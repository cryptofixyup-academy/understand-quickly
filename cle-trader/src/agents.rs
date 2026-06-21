/// Built-in cognition agents.
///
/// Each agent reads the current market snapshot and returns an AgentProposal
/// for the coordinator. Agents are stateless functions; per-agent rolling state
/// (e.g. for decay or performance tracking) can be added alongside as structs.
use crate::coordinator::{AgentProposal, InstrumentAction};
use crate::state::StateSnapshot;

/// Order-flow imbalance (OFI) agent.
///
/// Signal: `imbalance = (bid_qty - ask_qty) / (bid_qty + ask_qty)` ∈ [-1, 1].
/// Positive imbalance → buying pressure → long target.
/// Negative imbalance → selling pressure → short target.
///
/// `target_usd  = imbalance * scale_usd`
/// `conviction  = |imbalance|`
///
/// Symbols with no depth (total qty ≈ 0) are omitted.
pub fn ofi_agent(state: &StateSnapshot, scale_usd: f64) -> AgentProposal {
    let actions: Vec<InstrumentAction> = state
        .symbols
        .iter()
        .filter_map(|s| {
            let total_qty = s.best_bid_qty + s.best_ask_qty;
            if total_qty < 1e-9 {
                return None;
            }
            let imbalance = (s.best_bid_qty - s.best_ask_qty) / total_qty;
            Some(InstrumentAction {
                symbol: s.symbol.clone(),
                target_usd: imbalance * scale_usd,
                conviction: imbalance.abs(),
            })
        })
        .collect();

    AgentProposal {
        agent_id: "ofi-v1".to_string(),
        species: "order-flow".to_string(),
        actions,
        performance_score: 0.5,
        stability_score: 0.5,
        confidence: 0.5,
        risk_penalty: 0.0,
    }
}
