use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Market state
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymbolState {
    pub symbol: String,
    pub mid_price: f64,
    pub best_bid: f64,
    pub best_ask: f64,
    pub best_bid_qty: f64,
    pub best_ask_qty: f64,
    pub ts_ms: u64, // time this symbol was last updated
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub version: u64,
    pub ts_ms: u64,
    /// Sorted by symbol for deterministic iteration.
    pub symbols: Vec<SymbolState>,
}

impl StateSnapshot {
    pub fn empty() -> Self {
        Self {
            version: 0,
            ts_ms: 0,
            symbols: vec![],
        }
    }

    pub fn get(&self, symbol: &str) -> Option<&SymbolState> {
        self.symbols.iter().find(|s| s.symbol == symbol)
    }
}

/// Global StateBus. Readers call `.load()` to get an `Arc<StateSnapshot>`.
/// Writer (Ingestion) calls `.store(Arc::new(...))`.
/// Uses ArcSwap for lock-free RCU semantics.
pub static STATE_SNAPSHOT: Lazy<ArcSwap<StateSnapshot>> =
    Lazy::new(|| ArcSwap::from_pointee(StateSnapshot::empty()));

// ---------------------------------------------------------------------------
// Position state — owned by Execution process, read via snapshot elsewhere
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PositionState {
    pub symbol: String,
    pub position_usd: f64,   // signed: >0 long, <0 short (valued at avg entry price)
    pub position_qty: f64,   // signed: >0 long, <0 short
    pub realized_pnl: f64,
    /// Weighted-average entry price for the current open position.
    /// Zero when there is no open position.
    #[serde(default)]
    pub avg_entry_px: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PositionsSnapshot {
    pub ts_ms: u64,
    pub positions: Vec<PositionState>,
    /// Account equity = balance + unrealized PnL (from Binance user data stream)
    pub equity_usd: f64,
}

impl PositionsSnapshot {
    pub fn position_usd(&self, symbol: &str) -> f64 {
        self.positions
            .iter()
            .find(|p| p.symbol == symbol)
            .map(|p| p.position_usd)
            .unwrap_or(0.0)
    }
}

pub static POSITIONS_SNAPSHOT: Lazy<ArcSwap<PositionsSnapshot>> =
    Lazy::new(|| ArcSwap::from_pointee(PositionsSnapshot::default()));

// ---------------------------------------------------------------------------
// Working notional — owned by Execution actor, read by cognition loop
// ---------------------------------------------------------------------------

/// Signed sum of live (New + PartiallyFilled) order notional per symbol.
/// Positive = working buy USD, negative = working sell USD.
#[derive(Clone, Debug, Default)]
pub struct WorkingNotionalSnapshot {
    pub by_symbol: HashMap<String, f64>,
}

impl WorkingNotionalSnapshot {
    pub fn get(&self, symbol: &str) -> f64 {
        self.by_symbol.get(symbol).copied().unwrap_or(0.0)
    }
}

pub static WORKING_NOTIONAL: Lazy<ArcSwap<WorkingNotionalSnapshot>> =
    Lazy::new(|| ArcSwap::from_pointee(WorkingNotionalSnapshot::default()));

// ---------------------------------------------------------------------------
// Agent proposals snapshot — written by cognition loop, read by SGMI detector
// ---------------------------------------------------------------------------

/// Minimal projection of an agent's proposal: only the signed USD targets per
/// symbol are needed to compute pairwise cosine similarity for herding detection.
#[derive(Clone, Debug, Default)]
pub struct AgentTargetVector {
    pub agent_id: String,
    /// (symbol, signed target_usd)
    pub targets: Vec<(String, f64)>,
}

#[derive(Clone, Debug, Default)]
pub struct ProposalsSnapshot {
    pub ts_ms: u64,
    pub agents: Vec<AgentTargetVector>,
}

pub static PROPOSALS_SNAPSHOT: Lazy<ArcSwap<ProposalsSnapshot>> =
    Lazy::new(|| ArcSwap::from_pointee(ProposalsSnapshot::default()));
