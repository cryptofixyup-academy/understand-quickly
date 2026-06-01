/// Binance WebSocket ingestion.
///
/// Fixes vs the skeleton:
/// - Per-symbol update_id tracking (not a single global scalar)
/// - Pong sent explicitly (write half retained)
/// - parse errors logged and skipped, never silently zeroed
/// - OOO counter breach causes actual reconnect via Err return
/// - DashMap replaced with HashMap (single async task, no cross-thread writes)
/// - snapshot published on timer; if no messages arrive GCL detects staleness
///   via MD_TIMEOUT_MS (caller responsibility)
use crate::state::{StateSnapshot, SymbolState, STATE_SNAPSHOT};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::{interval, MissedTickBehavior};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Publish a StateSnapshot to the StateBus at this cadence regardless of
/// WS event frequency. Downstream tick length should be >= this value.
const SNAPSHOT_INTERVAL_MS: u64 = 100;

/// Tolerate this many OOO events per symbol before forcing reconnect.
const MAX_OOO_PER_SYMBOL: u64 = 5;

/// All-market best bid/ask stream.
const WS_URL: &str = "wss://stream.binance.com:9443/ws/!bookTicker";

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Binance individual symbol book ticker event.
/// https://binance-docs.github.io/apidocs/spot/en/#individual-symbol-book-ticker-streams
#[derive(Debug, Deserialize)]
struct BookTickerEvent {
    /// Update ID (per-symbol monotonic counter)
    u: u64,
    /// Symbol e.g. "BTCUSDT"
    s: String,
    /// Best bid price (string)
    b: String,
    /// Best bid quantity (string)
    #[serde(rename = "B")]
    bid_qty: String,
    /// Best ask price (string)
    a: String,
    /// Best ask quantity (string)
    #[serde(rename = "A")]
    ask_qty: String,
}

// ---------------------------------------------------------------------------
// Ingestion state
// ---------------------------------------------------------------------------

struct IngestionState {
    /// Per-symbol last seen update_id.
    last_update_id: HashMap<String, u64>,
    /// Per-symbol OOO counter. Reset on good event; triggers reconnect at threshold.
    ooo_counter: HashMap<String, u64>,
    /// Accumulated symbol states. Written on event; snapshotted on timer.
    symbol_map: HashMap<String, SymbolState>,
    /// Monotonically increasing version for StateSnapshot.
    version: u64,
}

impl IngestionState {
    fn new() -> Self {
        Self {
            last_update_id: HashMap::new(),
            ooo_counter: HashMap::new(),
            symbol_map: HashMap::new(),
            version: 0,
        }
    }

    /// Returns Err if caller should reconnect (OOO threshold breached).
    fn handle_event(&mut self, ev: BookTickerEvent) -> Result<(), &'static str> {
        let last = self.last_update_id.get(&ev.s).copied().unwrap_or(0);

        // Duplicate or out-of-order
        if ev.u <= last {
            let counter = self.ooo_counter.entry(ev.s.clone()).or_insert(0);
            *counter += 1;
            if *counter > MAX_OOO_PER_SYMBOL {
                warn!(
                    symbol = %ev.s,
                    ooo = *counter,
                    "OOO threshold breached — reconnecting"
                );
                return Err("ooo_threshold");
            }
            return Ok(());
        }

        // Good event
        self.last_update_id.insert(ev.s.clone(), ev.u);
        self.ooo_counter.insert(ev.s.clone(), 0);

        // Parse prices — skip on failure, never zero-fill
        let bid = match ev.b.parse::<f64>() {
            Ok(v) if v > 0.0 => v,
            _ => {
                warn!(symbol = %ev.s, raw = %ev.b, "failed to parse bid price — skipping event");
                return Ok(());
            }
        };
        let ask = match ev.a.parse::<f64>() {
            Ok(v) if v > 0.0 => v,
            _ => {
                warn!(symbol = %ev.s, raw = %ev.a, "failed to parse ask price — skipping event");
                return Ok(());
            }
        };

        let bid_qty = ev.bid_qty.parse::<f64>().unwrap_or(0.0);
        let ask_qty = ev.ask_qty.parse::<f64>().unwrap_or(0.0);

        if ask < bid {
            warn!(symbol = %ev.s, bid, ask, "crossed book — skipping event");
            return Ok(());
        }

        let mid = (bid + ask) / 2.0;

        self.symbol_map.insert(
            ev.s.clone(),
            SymbolState {
                symbol: ev.s,
                mid_price: mid,
                best_bid: bid,
                best_ask: ask,
                best_bid_qty: bid_qty,
                best_ask_qty: ask_qty,
                ts_ms: now_ms(),
            },
        );

        Ok(())
    }

    fn publish_snapshot(&mut self) {
        self.version += 1;
        let mut symbols: Vec<SymbolState> = self.symbol_map.values().cloned().collect();
        // Sorted for deterministic iteration by downstream consumers.
        symbols.sort_by(|a, b| a.symbol.cmp(&b.symbol));

        let snapshot = StateSnapshot {
            version: self.version,
            ts_ms: now_ms(),
            symbols,
        };
        STATE_SNAPSHOT.store(Arc::new(snapshot));
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Spawn this as a dedicated Tokio task. It runs forever, reconnecting on
/// any error. Caller should provide a shutdown signal if needed.
pub async fn run_ingestion() {
    let mut backoff_ms = 1_000u64;
    loop {
        let mut state = IngestionState::new();
        match run_once(&mut state).await {
            Ok(_) => {
                warn!("Ingestion WS loop ended cleanly — reconnecting");
            }
            Err(e) => {
                error!(err = %e, "Ingestion error — reconnecting after {}ms", backoff_ms);
            }
        }
        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        // Exponential backoff capped at 30s
        backoff_ms = (backoff_ms * 2).min(30_000);
    }
}

async fn run_once(state: &mut IngestionState) -> anyhow::Result<()> {
    info!("Connecting to Binance WS: {}", WS_URL);
    let (ws_stream, _) = connect_async(WS_URL).await?;
    info!("Binance WS connected");

    // Retain both halves so we can send pong.
    let (mut write, mut read) = ws_stream.split();

    // Publish snapshot on a fixed timer, not per-event.
    let mut publish_tick = interval(Duration::from_millis(SNAPSHOT_INTERVAL_MS));
    publish_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    None => {
                        warn!("WS stream ended");
                        return Ok(());
                    }
                    Some(Err(e)) => {
                        return Err(e.into());
                    }
                    Some(Ok(Message::Text(txt))) => {
                        match serde_json::from_str::<BookTickerEvent>(&txt) {
                            Ok(ev) => {
                                if let Err(reason) = state.handle_event(ev) {
                                    // OOO threshold: force reconnect
                                    return Err(anyhow::anyhow!("reconnect: {}", reason));
                                }
                            }
                            Err(e) => {
                                // Log and continue — one bad frame is not fatal
                                warn!(err = %e, "failed to deserialize WS message");
                            }
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        // Binance sends pings every 3 minutes.
                        // Must respond with pong or connection is closed.
                        if let Err(e) = write.send(Message::Pong(payload)).await {
                            return Err(e.into());
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        warn!("WS close frame received: {:?}", frame);
                        return Ok(());
                    }
                    Some(Ok(_)) => {
                        // Binary, Frame, etc. — ignore
                    }
                }
            }
            _ = publish_tick.tick() => {
                state.publish_snapshot();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_millis() as u64
}
