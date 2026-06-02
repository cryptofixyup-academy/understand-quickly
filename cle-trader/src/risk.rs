/// Risk gate: 4-stage filter applied after allocation, before execution.
///
/// Stage 1: SGMI gate (hard/soft block)
/// Stage 2: Drawdown limit (cumulative realized PnL floor)
/// Stage 3: Per-symbol exposure cap
/// Stage 4: Portfolio leverage cap
use crate::allocator::{AllocatedDelta, RiskLimits};
use crate::sgmi::{evaluate_sgmi_gate, reduces_gross_exposure, SgmiGateDecision};
use crate::state::PositionsSnapshot;
use crate::ingestion::now_ms;
use std::collections::HashMap;
use tracing::warn;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum RejectionReason {
    SgmiHardBlock { score: f64, age_ms: u64 },
    SgmiStale { age_ms: u64 },
    SgmiSoftBlockIncreasesExposure { score: f64 },
    ExposureLimitBreached { symbol: String, would_be: f64, limit: f64 },
    LeverageLimitBreached { gross: f64, limit: f64 },
    DrawdownLimitBreached,
}

#[derive(Clone, Debug)]
pub enum GateResult {
    Approved(Vec<AllocatedDelta>),
    Rejected(RejectionReason),
    PartialApproved(Vec<AllocatedDelta>, Vec<(String, RejectionReason)>),
}

// ---------------------------------------------------------------------------
// Gate logic
// ---------------------------------------------------------------------------

pub fn evaluate_risk_gate(
    deltas: Vec<AllocatedDelta>,
    positions: &PositionsSnapshot,
    limits: &RiskLimits,
    sgmi_override_active: bool,
) -> GateResult {
    let now = now_ms();
    let gate = evaluate_sgmi_gate(now, sgmi_override_active);

    // Stage 1: SGMI
    match gate.decision {
        SgmiGateDecision::HardBlock => {
            warn!(score = gate.score, age_ms = gate.age_ms, "SGMI hard block — rejecting all");
            if gate.age_ms > 2_000 {
                return GateResult::Rejected(RejectionReason::SgmiStale { age_ms: gate.age_ms });
            }
            return GateResult::Rejected(RejectionReason::SgmiHardBlock {
                score: gate.score,
                age_ms: gate.age_ms,
            });
        }
        SgmiGateDecision::SoftBlock => {
            // Allow only if the batch reduces gross exposure.
            let current_pos: HashMap<String, f64> = positions
                .positions
                .iter()
                .map(|p| (p.symbol.clone(), p.position_usd))
                .collect();
            let delta_pairs: Vec<(String, f64)> = deltas
                .iter()
                .map(|d| (d.symbol.clone(), d.delta_net_usd))
                .collect();
            if !reduces_gross_exposure(&delta_pairs, &current_pos) {
                warn!(score = gate.score, "SGMI soft block increases exposure — rejecting");
                return GateResult::Rejected(RejectionReason::SgmiSoftBlockIncreasesExposure {
                    score: gate.score,
                });
            }
        }
        SgmiGateDecision::Pass => {}
    }

    // Stage 2: drawdown check — block all new activity if cumulative realized
    // loss exceeds the configured budget.
    let total_realized_pnl: f64 =
        positions.positions.iter().map(|p| p.realized_pnl).sum();
    if total_realized_pnl < -limits.max_drawdown_usd {
        warn!(
            realized_pnl = total_realized_pnl,
            limit = limits.max_drawdown_usd,
            "drawdown limit breached — rejecting all"
        );
        return GateResult::Rejected(RejectionReason::DrawdownLimitBreached);
    }

    // Stage 3 + 4: per-symbol exposure + leverage.
    let equity = if positions.equity_usd > 0.0 {
        positions.equity_usd
    } else {
        limits.c_tot
    };

    let mut approved: Vec<AllocatedDelta> = Vec::with_capacity(deltas.len());
    let mut rejected_pairs: Vec<(String, RejectionReason)> = vec![];

    for d in deltas {
        let cur = positions.position_usd(&d.symbol);
        let would_be = (cur + d.delta_net_usd).abs();
        let cap = limits
            .symbol_caps
            .get(&d.symbol)
            .copied()
            .unwrap_or(limits.default_symbol_cap);

        if would_be > cap + 1.0 {
            warn!(
                symbol = %d.symbol,
                would_be,
                cap,
                "per-symbol exposure cap breached — rejecting delta"
            );
            rejected_pairs.push((
                d.symbol.clone(),
                RejectionReason::ExposureLimitBreached {
                    symbol: d.symbol,
                    would_be,
                    limit: cap,
                },
            ));
            continue;
        }
        approved.push(d);
    }

    // Portfolio leverage check on approved set.
    let gross_after: f64 = approved
        .iter()
        .map(|d| {
            let cur = positions.position_usd(&d.symbol);
            (cur + d.delta_net_usd).abs()
        })
        .sum();
    let leverage_limit = limits.l_max * equity;
    if gross_after > leverage_limit + 1.0 {
        warn!(gross = gross_after, limit = leverage_limit, "leverage limit breached — rejecting all approved");
        return GateResult::Rejected(RejectionReason::LeverageLimitBreached {
            gross: gross_after,
            limit: leverage_limit,
        });
    }

    if rejected_pairs.is_empty() {
        GateResult::Approved(approved)
    } else if approved.is_empty() {
        GateResult::Rejected(rejected_pairs.into_iter().next().unwrap().1)
    } else {
        GateResult::PartialApproved(approved, rejected_pairs)
    }
}
