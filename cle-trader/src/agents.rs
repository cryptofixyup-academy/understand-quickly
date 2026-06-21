/// Built-in cognition agents.
///
/// Each agent reads the current market snapshot and returns an AgentProposal
/// for the coordinator. Per-agent rolling state lives in companion structs so
/// performance_score and stability_score are data-driven rather than hardcoded.
use crate::coordinator::{AgentProposal, InstrumentAction};
use crate::state::{PositionsSnapshot, StateSnapshot};
use std::collections::VecDeque;

const AGENT_WINDOW: usize = 60;

// ---------------------------------------------------------------------------
// OFI agent state
// ---------------------------------------------------------------------------

/// Rolling state for the OFI agent, updated on every cognition tick.
pub struct OfiAgentState {
    /// Total realized PnL snapshots (window AGENT_WINDOW).
    pnl_history: VecDeque<f64>,
    /// Mean signed imbalance across all symbols per tick (window AGENT_WINDOW).
    signal_history: VecDeque<f64>,
}

impl OfiAgentState {
    pub fn new() -> Self {
        Self {
            pnl_history: VecDeque::with_capacity(AGENT_WINDOW + 1),
            signal_history: VecDeque::with_capacity(AGENT_WINDOW + 1),
        }
    }

    /// Rolling Sharpe of tick-to-tick realized PnL increments, mapped to [0,1].
    /// Sharpe ±2 saturates the score at 1.0 / 0.0; Sharpe 0 → 0.5.
    fn performance_score(&self) -> f64 {
        if self.pnl_history.len() < 2 {
            return 0.5;
        }
        let incs: Vec<f64> = self.pnl_history
            .iter()
            .zip(self.pnl_history.iter().skip(1))
            .map(|(a, b)| b - a)
            .collect();
        let n = incs.len() as f64;
        let mean = incs.iter().sum::<f64>() / n;
        let variance = incs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
        let sharpe = mean / (variance.sqrt() + 1e-9);
        (0.5 + sharpe * 0.25).clamp(0.0, 1.0)
    }

    /// Stability: how consistent the mean-imbalance signal has been.
    /// std(signal) = 0 → 1.0 (rock-steady); std ≥ 0.5 → 0.0 (noisy).
    fn stability_score(&self) -> f64 {
        if self.signal_history.len() < 2 {
            return 0.5;
        }
        let n = self.signal_history.len() as f64;
        let mean = self.signal_history.iter().sum::<f64>() / n;
        let variance =
            self.signal_history.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
        (1.0 - variance.sqrt() * 2.0).clamp(0.0, 1.0)
    }

    fn push_pnl(&mut self, total_pnl: f64) {
        self.pnl_history.push_back(total_pnl);
        if self.pnl_history.len() > AGENT_WINDOW {
            self.pnl_history.pop_front();
        }
    }

    fn push_signal(&mut self, mean_imbalance: f64) {
        self.signal_history.push_back(mean_imbalance);
        if self.signal_history.len() > AGENT_WINDOW {
            self.signal_history.pop_front();
        }
    }
}

// ---------------------------------------------------------------------------
// OFI agent
// ---------------------------------------------------------------------------

/// Order-flow imbalance (OFI) agent.
///
/// Signal: `imbalance = (bid_qty - ask_qty) / (bid_qty + ask_qty)` ∈ [-1, 1].
/// Positive imbalance → buying pressure → long target.
/// Negative imbalance → selling pressure → short target.
///
/// `target_usd  = imbalance * scale_usd`
/// `conviction  = |imbalance|`
///
/// `performance_score` and `stability_score` are derived from rolling histories
/// of realized PnL and mean-imbalance signal respectively.
/// Symbols with no depth (total qty ≈ 0) are omitted.
pub fn ofi_agent(
    state: &StateSnapshot,
    positions: &PositionsSnapshot,
    agent_state: &mut OfiAgentState,
    scale_usd: f64,
) -> AgentProposal {
    let mut total_imbalance = 0.0f64;
    let mut total_abs_imbalance = 0.0f64;
    let mut total_spread_pct = 0.0f64;
    let mut count = 0usize;

    let actions: Vec<InstrumentAction> = state
        .symbols
        .iter()
        .filter_map(|s| {
            let total_qty = s.best_bid_qty + s.best_ask_qty;
            if total_qty < 1e-9 {
                return None;
            }
            let imbalance = (s.best_bid_qty - s.best_ask_qty) / total_qty;
            total_imbalance += imbalance;
            total_abs_imbalance += imbalance.abs();
            if s.mid_price > 1e-9 {
                total_spread_pct += (s.best_ask - s.best_bid) / s.mid_price;
            }
            count += 1;
            Some(InstrumentAction {
                symbol: s.symbol.clone(),
                target_usd: imbalance * scale_usd,
                conviction: imbalance.abs(),
            })
        })
        .collect();

    let n = count.max(1) as f64;
    let mean_imbalance = total_imbalance / n;

    // confidence: how strong is the signal? High mean |imbalance| → high confidence.
    let confidence = (total_abs_imbalance / n).clamp(0.0, 1.0);

    // risk_penalty: liquidity risk proxy — wide relative spread means poor fills.
    // Scaled so a 1% spread saturates at 1.0; typical liquid crypto = 0.001–0.05.
    let risk_penalty = (total_spread_pct / n * 100.0).clamp(0.0, 1.0);

    let total_pnl: f64 = positions.positions.iter().map(|p| p.realized_pnl).sum();

    agent_state.push_pnl(total_pnl);
    agent_state.push_signal(mean_imbalance);

    AgentProposal {
        agent_id: "ofi-v1".to_string(),
        species: "order-flow".to_string(),
        actions,
        performance_score: agent_state.performance_score(),
        stability_score: agent_state.stability_score(),
        confidence,
        risk_penalty,
    }
}
