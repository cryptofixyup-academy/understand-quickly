mod allocator;
mod coordinator;
mod execution;
mod ingestion;
mod risk;
mod sgmi;
mod state;

use crate::allocator::{allocate, RiskLimits};
use crate::coordinator::{coordinate, AgentProposal, CoordinatorConfig};
use crate::execution::{spawn_execution, ExecutionCommand, OrderSide};
use crate::ingestion::{now_ms, run_ingestion};
use crate::risk::{evaluate_risk_gate, GateResult};
use crate::sgmi::run_sgmi_monitor;
use crate::state::{POSITIONS_SNAPSHOT, STATE_SNAPSHOT};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration, MissedTickBehavior};
use tracing::info;

/// How often the cognition loop ticks. Matches SNAPSHOT_INTERVAL_MS in
/// ingestion so every tick sees a fresh StateSnapshot.
const COGNITION_TICK_MS: u64 = 100;
/// GTC TTL for orders emitted by the cognition loop.
const ORDER_TTL_MS: u64 = 5_000;

/// Cognition loop: runs every COGNITION_TICK_MS, reads market and position
/// state, runs the full coordinator → allocator → risk-gate pipeline, and
/// sends approved orders to the execution actor.
async fn run_cognition(exec_tx: mpsc::Sender<ExecutionCommand>) {
    let mut tick = interval(Duration::from_millis(COGNITION_TICK_MS));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let coordinator_cfg = CoordinatorConfig::default();
    let risk_limits = RiskLimits::default();

    loop {
        tick.tick().await;

        let state = STATE_SNAPSHOT.load();
        let positions = POSITIONS_SNAPSHOT.load();

        // Collect proposals from registered agents.  No agents are wired yet;
        // insert agent calls here once cognition agents are implemented.
        let proposals: Vec<AgentProposal> = vec![];

        let Some(unified) = coordinate(proposals, state.version, &positions, &coordinator_cfg) else {
            continue;
        };

        // Working notional stubs to zero — full accounting requires the
        // ExecutionActor's live order book exposed as shared state.
        let deltas = allocate(&unified, &positions, &risk_limits, &|_: &str| 0.0);
        if deltas.is_empty() {
            continue;
        }

        let approved = match evaluate_risk_gate(deltas, &positions, &risk_limits, false) {
            GateResult::Approved(d) | GateResult::PartialApproved(d, _) => d,
            GateResult::Rejected(_) => continue,
        };

        for delta in approved {
            let Some(sym_state) = state.get(&delta.symbol) else {
                continue;
            };
            let price = sym_state.mid_price;
            if price <= 0.0 {
                continue;
            }
            let side = if delta.delta_net_usd > 0.0 {
                OrderSide::Buy
            } else {
                OrderSide::Sell
            };
            let qty = delta.delta_net_usd.abs() / price;
            if qty <= 0.0 {
                continue;
            }
            // try_send: drop the order silently if the execution actor's
            // channel is full — the next tick will re-evaluate and resubmit.
            let _ = exec_tx.try_send(ExecutionCommand::SendOrder {
                symbol: delta.symbol,
                side,
                price,
                qty,
                ttl_ms: ORDER_TTL_MS,
            });
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!("cle-trader starting");

    let process_start_ms = now_ms();

    // Read credentials from environment (never hardcode)
    let api_key = std::env::var("BINANCE_API_KEY").unwrap_or_default();
    let secret = std::env::var("BINANCE_SECRET").unwrap_or_default();

    // Spawn ingestion: Binance WS -> StateBus
    tokio::spawn(async move {
        run_ingestion().await;
    });

    // Spawn SGMI monitor (background, not in hot path)
    tokio::spawn(async move {
        run_sgmi_monitor(process_start_ms).await;
    });

    // Spawn execution actor: owns OrderBook, PositionState, UDS
    let exec_tx = spawn_execution(api_key, secret);

    // Spawn cognition loop: reads STATE_SNAPSHOT each tick, collects agent
    // proposals, runs coordinator → allocator → risk gate, and sends approved
    // ExecutionCommands to the execution actor.
    tokio::spawn(run_cognition(exec_tx));

    info!("All tasks spawned. Running.");

    // Keep main alive
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl-c");

    info!("Shutdown signal received");
}
