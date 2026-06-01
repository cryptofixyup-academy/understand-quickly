mod allocator;
mod coordinator;
mod execution;
mod ingestion;
mod risk;
mod sgmi;
mod state;

use crate::execution::spawn_execution;
use crate::ingestion::{now_ms, run_ingestion};
use crate::sgmi::run_sgmi_monitor;
use tracing::info;

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
    let _exec_tx = spawn_execution(api_key, secret);
    // exec_tx is the channel for sending ExecutionCommand from Cognition/Risk

    // TODO: spawn cognition loop:
    //   - per tick: read STATE_SNAPSHOT, collect proposals, coordinate, allocate
    //   - pass allocated deltas through risk gate
    //   - send approved ExecutionCommands via exec_tx

    info!("All tasks spawned. Running.");

    // Keep main alive
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl-c");

    info!("Shutdown signal received");
}
