/// Multi-agent coordinator.
///
/// Aggregates proposals from N agents into a single UnifiedAction via
/// conviction-weighted targets, species water-filling caps, and diversity enforcement.
use crate::state::PositionsSnapshot;
use std::collections::HashMap;
use tracing::warn;

// ---------------------------------------------------------------------------
// Input types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct InstrumentAction {
    pub symbol: String,
    /// Desired signed USD position target (positive = long, negative = short).
    pub target_usd: f64,
    /// Agent's confidence [0,1] in this target.
    pub conviction: f64,
}

#[derive(Clone, Debug)]
pub struct AgentProposal {
    pub agent_id: String,
    pub species: String,
    pub actions: Vec<InstrumentAction>,
    /// Recent Sharpe or other performance proxy [0,1].
    pub performance_score: f64,
    /// Regime-stability proxy [0,1] — how consistent the agent has been.
    pub stability_score: f64,
    /// Agent's own confidence estimate [0,1].
    pub confidence: f64,
    /// Agent's self-reported risk penalty [0,1].
    pub risk_penalty: f64,
}

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SymbolTarget {
    pub symbol: String,
    pub target_usd: f64,
    pub conviction: f64,
    /// Inverse Herfindahl index: 1 / sum(w_i^2 / W^2).
    pub diversity: f64,
}

#[derive(Clone, Debug)]
pub struct AgentContribution {
    pub agent_id: String,
    pub weight: f64,
}

#[derive(Clone, Debug)]
pub struct UnifiedAction {
    pub state_version: u64,
    pub targets: Vec<SymbolTarget>,
    pub agent_contributions: Vec<AgentContribution>,
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct CoordinatorConfig {
    /// Weight blending coefficients — must sum to 1.
    pub alpha_performance: f64,
    pub alpha_stability: f64,
    pub alpha_confidence: f64,
    pub alpha_risk_penalty: f64,
    /// Per-species notional cap as fraction of total budget.
    pub species_cap: f64,
    /// Maximum notional change allowed per tick.
    pub max_tick_notional_change_usd: f64,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            alpha_performance: 0.35,
            alpha_stability: 0.35,
            alpha_confidence: 0.20,
            alpha_risk_penalty: 0.10,
            species_cap: 0.40,
            max_tick_notional_change_usd: 100_000.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

pub fn compute_base_weight(p: &AgentProposal, cfg: &CoordinatorConfig) -> f64 {
    let raw = cfg.alpha_performance * p.performance_score
        + cfg.alpha_stability * p.stability_score
        + cfg.alpha_confidence * p.confidence
        - cfg.alpha_risk_penalty * p.risk_penalty;
    raw.max(0.0)
}

/// Iterative species-cap enforcement: each round clamps the worst violator.
pub fn apply_species_caps(
    weights: &mut HashMap<String, f64>,
    proposals: &[AgentProposal],
    species_cap: f64,
) {
    let total_w: f64 = weights.values().sum();
    if total_w <= 0.0 {
        return;
    }
    let mut species_weight: HashMap<String, f64> = HashMap::new();
    for p in proposals {
        *species_weight.entry(p.species.clone()).or_insert(0.0) +=
            weights.get(&p.agent_id).copied().unwrap_or(0.0);
    }

    for _ in 0..proposals.len() {
        let total: f64 = weights.values().sum();
        if total <= 0.0 {
            break;
        }
        // Find species with highest fractional weight exceeding cap.
        let worst = species_weight
            .iter()
            .filter(|(_, &sw)| sw / total > species_cap)
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal));
        let worst_species: String = match worst {
            Some((k, _)) => k.clone(),
            None => break,
        };

        let excess = species_weight[&worst_species] / total - species_cap;
        if excess <= 0.0 {
            break;
        }
        // Trim heaviest agent in the species proportionally.
        let heaviest = proposals
            .iter()
            .filter(|p| p.species == worst_species)
            .max_by(|a, b| {
                weights
                    .get(&a.agent_id)
                    .partial_cmp(&weights.get(&b.agent_id))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some(agent) = heaviest {
            let w = weights.entry(agent.agent_id.clone()).or_insert(0.0);
            let trim = (*w * excess).min(*w);
            *w -= trim;
            *species_weight.get_mut(&worst_species).unwrap() -= trim;
        } else {
            break;
        }
    }
}

fn aggregate_symbol(
    symbol: &str,
    proposals: &[AgentProposal],
    weights: &HashMap<String, f64>,
) -> Option<SymbolTarget> {
    let total_w: f64 = weights.values().sum();
    if total_w <= 0.0 {
        return None;
    }

    let mut bull_w = 0.0f64;
    let mut bear_w = 0.0f64;
    let mut weighted_target = 0.0f64;
    let mut sum_w2 = 0.0f64;
    let mut any = false;

    for p in proposals {
        let w = weights.get(&p.agent_id).copied().unwrap_or(0.0);
        if w <= 0.0 {
            continue;
        }
        for a in &p.actions {
            if a.symbol != symbol {
                continue;
            }
            any = true;
            let wc = w * a.conviction;
            weighted_target += wc * a.target_usd;
            if a.target_usd >= 0.0 {
                bull_w += wc;
            } else {
                bear_w += wc;
            }
            sum_w2 += (w / total_w).powi(2);
        }
    }

    if !any {
        return None;
    }

    let norm = bull_w + bear_w;
    let target_usd = if norm > 0.0 { weighted_target / norm } else { 0.0 };
    let conviction = if norm > 0.0 {
        ((bull_w - bear_w) / norm).abs()
    } else {
        0.0
    };
    let diversity = if sum_w2 > 0.0 { 1.0 / sum_w2 } else { 1.0 };

    Some(SymbolTarget { symbol: symbol.to_string(), target_usd, conviction, diversity })
}

/// Clips aggregate delta to `max_tick_notional_change_usd`, ordered by conviction desc.
fn priority_clip(
    targets: Vec<SymbolTarget>,
    current_positions: &PositionsSnapshot,
    max_change: f64,
) -> Vec<SymbolTarget> {
    let mut sorted = targets;
    sorted.sort_by(|a, b| b.conviction.partial_cmp(&a.conviction).unwrap_or(std::cmp::Ordering::Equal));

    let mut remaining = max_change;
    let mut out = Vec::with_capacity(sorted.len());
    for t in sorted {
        let cur = current_positions.position_usd(&t.symbol);
        let delta = (t.target_usd - cur).abs();
        if remaining <= 0.0 {
            // Keep target equal to current — no change.
            out.push(SymbolTarget { target_usd: cur, ..t });
            continue;
        }
        if delta <= remaining {
            remaining -= delta;
            out.push(t);
        } else {
            let direction = if t.target_usd > cur { 1.0 } else { -1.0 };
            let clipped = cur + direction * remaining;
            remaining = 0.0;
            out.push(SymbolTarget { target_usd: clipped, ..t });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn coordinate(
    proposals: Vec<AgentProposal>,
    state_version: u64,
    current_positions: &PositionsSnapshot,
    cfg: &CoordinatorConfig,
) -> Option<UnifiedAction> {
    if proposals.is_empty() {
        return None;
    }

    // Compute raw weights.
    let mut weights: HashMap<String, f64> = proposals
        .iter()
        .map(|p| (p.agent_id.clone(), compute_base_weight(p, cfg)))
        .collect();

    let total_w: f64 = weights.values().sum();
    if total_w <= 0.0 {
        warn!("All agent weights are zero — no action");
        return None;
    }

    apply_species_caps(&mut weights, &proposals, cfg.species_cap);

    // Collect all symbols.
    let all_symbols: std::collections::HashSet<String> = proposals
        .iter()
        .flat_map(|p| p.actions.iter().map(|a| a.symbol.clone()))
        .collect();

    let mut targets: Vec<SymbolTarget> = all_symbols
        .iter()
        .filter_map(|sym| aggregate_symbol(sym, &proposals, &weights))
        .collect();

    targets = priority_clip(targets, current_positions, cfg.max_tick_notional_change_usd);

    let total_final: f64 = weights.values().sum();
    let agent_contributions: Vec<AgentContribution> = weights
        .into_iter()
        .filter(|(_, w)| *w > 0.0)
        .map(|(agent_id, w)| AgentContribution {
            agent_id,
            weight: if total_final > 0.0 { w / total_final } else { 0.0 },
        })
        .collect();

    Some(UnifiedAction { state_version, targets, agent_contributions })
}
