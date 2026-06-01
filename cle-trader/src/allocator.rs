/// Position allocator: converts a UnifiedAction into sized, delta-filtered orders.
///
/// 5-stage pipeline:
///   1. Per-symbol cap clamp
///   2. Conviction modulation with diversity scaling
///   3. Conviction-sorted budget water-fill
///   4. Leverage enforcement
///   5. Delta-net filtering (remove |delta| < $1)
use crate::coordinator::UnifiedAction;
use crate::state::PositionsSnapshot;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct RiskLimits {
    /// Total gross USD budget.
    pub c_tot: f64,
    /// Per-symbol maximum absolute exposure.
    pub symbol_caps: HashMap<String, f64>,
    /// Default cap for symbols not explicitly listed.
    pub default_symbol_cap: f64,
    /// Maximum portfolio leverage (gross / equity).
    pub l_max: f64,
    /// Target inverse Herfindahl diversity index.
    pub diversity_target: f64,
}

impl Default for RiskLimits {
    fn default() -> Self {
        Self {
            c_tot: 1_000_000.0,
            symbol_caps: HashMap::new(),
            default_symbol_cap: 200_000.0,
            l_max: 1.0,
            diversity_target: 2.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct AllocatedDelta {
    pub symbol: String,
    pub target_usd: f64,
    pub delta_net_usd: f64,
    pub conviction: f64,
}

// ---------------------------------------------------------------------------
// Allocator
// ---------------------------------------------------------------------------

pub fn allocate(
    unified: &UnifiedAction,
    positions: &PositionsSnapshot,
    limits: &RiskLimits,
    working_notional: &dyn Fn(&str) -> f64,
) -> Vec<AllocatedDelta> {
    let equity = if positions.equity_usd > 0.0 {
        positions.equity_usd
    } else {
        limits.c_tot
    };

    // Stage 1: clamp targets to per-symbol caps.
    let mut capped: Vec<(String, f64, f64)> = unified
        .targets
        .iter()
        .map(|t| {
            let cap = limits
                .symbol_caps
                .get(&t.symbol)
                .copied()
                .unwrap_or(limits.default_symbol_cap);
            let clamped = t.target_usd.clamp(-cap, cap);
            (t.symbol.clone(), clamped, t.conviction)
        })
        .collect();

    // Stage 2: conviction modulation with diversity scaling.
    for (sym, target, conviction) in &mut capped {
        let diversity = unified
            .targets
            .iter()
            .find(|t| &t.symbol == sym)
            .map(|t| t.diversity)
            .unwrap_or(1.0);
        let m_s = (diversity / limits.diversity_target).min(1.0) * *conviction;
        *target *= m_s;
    }

    // Stage 3: budget water-fill sorted by conviction desc.
    capped.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let mut budget_remaining = limits.c_tot;
    let mut filled: Vec<(String, f64, f64)> = Vec::with_capacity(capped.len());
    for (sym, target, conv) in capped {
        let abs_target = target.abs().min(budget_remaining);
        let final_target = target.signum() * abs_target;
        budget_remaining -= abs_target;
        filled.push((sym, final_target, conv));
    }

    // Stage 4: leverage enforcement — scale if gross > l_max * equity.
    let gross: f64 = filled.iter().map(|(_, t, _)| t.abs()).sum();
    let leverage_limit = limits.l_max * equity;
    let scale = if gross > leverage_limit + 1.0 {
        leverage_limit / gross
    } else {
        1.0
    };

    // Stage 5: delta-net filter — skip |delta| < $1.
    const DELTA_EPSILON: f64 = 1.0;
    filled
        .into_iter()
        .filter_map(|(sym, target, conviction)| {
            let scaled_target = target * scale;
            let cur = positions.position_usd(&sym);
            let working = working_notional(&sym);
            let delta_net = scaled_target - cur - working;
            if delta_net.abs() < DELTA_EPSILON {
                return None;
            }
            Some(AllocatedDelta {
                symbol: sym,
                target_usd: scaled_target,
                delta_net_usd: delta_net,
                conviction,
            })
        })
        .collect()
}
