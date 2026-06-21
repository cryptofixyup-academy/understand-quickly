/// Execution actor: owns the live OrderBook and drives Binance REST + UDS.
///
/// On startup: reconcile_from_rest → flatten_all (safety) → create_listen_key → UDS loop.
/// Reconnects UDS on any error, keeps listen key alive every 20 min.
/// Shuts down gracefully on CancelAll / FlattenAll commands.
use crate::ingestion::now_ms;
use crate::state::{PositionState, PositionsSnapshot, POSITIONS_SNAPSHOT, WorkingNotionalSnapshot, WORKING_NOTIONAL};
use anyhow::Result;
use futures::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration, MissedTickBehavior};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const BINANCE_REST_URL: &str = "https://api.binance.com";
const BINANCE_WS_BASE: &str = "wss://stream.binance.com:9443/ws";
const UDS_KEEPALIVE_MS: u64 = 20 * 60 * 1_000; // 20 min
const UDS_TIMEOUT_MS: u64 = 1_000;
const DELTA_EPSILON_USD: f64 = 1.0;

// ---------------------------------------------------------------------------
// Commands (sent from Cognition / Risk)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ExecutionCommand {
    SendOrder {
        symbol: String,
        side: OrderSide,
        price: f64,
        qty: f64,
        ttl_ms: u64,
    },
    CancelOrder {
        client_order_id: String,
    },
    CancelAll,
    FlattenAll,
}

// ---------------------------------------------------------------------------
// Order model
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OrderStatus {
    New,
    PartiallyFilled,
    Filled,
    CancelPending,
    Canceled,
    Expired,
}

#[derive(Clone, Debug)]
pub struct Order {
    pub client_order_id: String,
    pub symbol: String,
    pub side: OrderSide,
    pub price: f64,
    pub qty: f64,
    pub filled_qty: f64,
    pub status: OrderStatus,
    pub created_at_ms: u64,
    pub ttl_ms: u64,
}

impl Order {
    pub fn working_notional(&self) -> f64 {
        let remaining = self.qty - self.filled_qty;
        let signed = match self.side {
            OrderSide::Buy => remaining,
            OrderSide::Sell => -remaining,
        };
        signed * self.price
    }
}

// ---------------------------------------------------------------------------
// OrderBook
// ---------------------------------------------------------------------------

pub struct OrderBook {
    orders: HashMap<String, Order>,
}

impl OrderBook {
    pub fn new() -> Self {
        Self { orders: HashMap::new() }
    }

    pub fn insert(&mut self, order: Order) {
        self.orders.insert(order.client_order_id.clone(), order);
    }

    pub fn working_notional_for(&self, symbol: &str) -> f64 {
        self.orders
            .values()
            .filter(|o| o.symbol == symbol && matches!(o.status, OrderStatus::New | OrderStatus::PartiallyFilled))
            .map(|o| o.working_notional())
            .sum()
    }

    /// Idempotent fill application.
    pub fn apply_fill(&mut self, coid: &str, filled_qty: f64) {
        if let Some(o) = self.orders.get_mut(coid) {
            o.filled_qty = filled_qty.max(o.filled_qty);
            if (o.filled_qty - o.qty).abs() < 1e-9 {
                o.status = OrderStatus::Filled;
            } else {
                o.status = OrderStatus::PartiallyFilled;
            }
        }
    }

    pub fn apply_cancel_ack(&mut self, coid: &str) {
        if let Some(o) = self.orders.get_mut(coid) {
            o.status = OrderStatus::Canceled;
        }
    }

    pub fn apply_expired(&mut self, coid: &str) {
        if let Some(o) = self.orders.get_mut(coid) {
            o.status = OrderStatus::Expired;
        }
    }

    pub fn set_cancel_pending(&mut self, coid: &str) {
        if let Some(o) = self.orders.get_mut(coid) {
            if matches!(o.status, OrderStatus::New | OrderStatus::PartiallyFilled) {
                o.status = OrderStatus::CancelPending;
            }
        }
    }

    pub fn cancel_all_working(&mut self) -> Vec<String> {
        self.orders
            .values_mut()
            .filter(|o| matches!(o.status, OrderStatus::New | OrderStatus::PartiallyFilled))
            .map(|o| {
                o.status = OrderStatus::CancelPending;
                o.client_order_id.clone()
            })
            .collect()
    }

    pub fn clear_terminals(&mut self) {
        self.orders.retain(|_, o| {
            !matches!(
                o.status,
                OrderStatus::Filled | OrderStatus::Canceled | OrderStatus::Expired
            )
        });
    }
}

// ---------------------------------------------------------------------------
// Binance REST client
// ---------------------------------------------------------------------------

pub struct BinanceRest {
    client: reqwest::Client,
    api_key: String,
    secret: String,
}

impl BinanceRest {
    pub fn new(api_key: String, secret: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            secret,
        }
    }

    fn sign(&self, query: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.secret.as_bytes())
            .expect("HMAC can accept any key length");
        mac.update(query.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    pub async fn get_account(&self) -> Result<AccountInfo> {
        let ts = now_ms();
        let query = format!("timestamp={}", ts);
        let sig = self.sign(&query);
        let url = format!("{}/api/v3/account?{}&signature={}", BINANCE_REST_URL, query, sig);
        let resp = self
            .client
            .get(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?
            .json::<AccountInfo>()
            .await?;
        Ok(resp)
    }

    pub async fn send_limit_order(
        &self,
        symbol: &str,
        side: &str,
        price: f64,
        qty: f64,
        client_order_id: &str,
    ) -> Result<()> {
        let ts = now_ms();
        let body = format!(
            "symbol={}&side={}&type=LIMIT&timeInForce=GTC&quantity={:.8}&price={:.8}\
             &newClientOrderId={}&timestamp={}",
            symbol, side, qty, price, client_order_id, ts
        );
        let sig = self.sign(&body);
        let full = format!("{}&signature={}", body, sig);
        let resp = self
            .client
            .post(format!("{}/api/v3/order", BINANCE_REST_URL))
            .header("X-MBX-APIKEY", &self.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(full)
            .send()
            .await?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("send_limit_order failed: {}", text));
        }
        Ok(())
    }

    pub async fn send_market_order(
        &self,
        symbol: &str,
        side: &str,
        qty: f64,
        client_order_id: &str,
    ) -> Result<()> {
        let ts = now_ms();
        let body = format!(
            "symbol={}&side={}&type=MARKET&quantity={:.8}&newClientOrderId={}&timestamp={}",
            symbol, side, qty, client_order_id, ts
        );
        let sig = self.sign(&body);
        let full = format!("{}&signature={}", body, sig);
        let resp = self
            .client
            .post(format!("{}/api/v3/order", BINANCE_REST_URL))
            .header("X-MBX-APIKEY", &self.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(full)
            .send()
            .await?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("send_market_order failed: {}", text));
        }
        Ok(())
    }

    pub async fn cancel_order(&self, symbol: &str, client_order_id: &str) -> Result<()> {
        let ts = now_ms();
        let body = format!(
            "symbol={}&origClientOrderId={}&timestamp={}",
            symbol, client_order_id, ts
        );
        let sig = self.sign(&body);
        let full = format!("{}&signature={}", body, sig);
        let resp = self
            .client
            .delete(format!("{}/api/v3/order", BINANCE_REST_URL))
            .header("X-MBX-APIKEY", &self.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(full)
            .send()
            .await?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("cancel_order failed: {}", text));
        }
        Ok(())
    }

    pub async fn create_listen_key(&self) -> Result<String> {
        let resp: serde_json::Value = self
            .client
            .post(format!("{}/api/v3/userDataStream", BINANCE_REST_URL))
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await?
            .json()
            .await?;
        let key = resp["listenKey"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no listenKey in response"))?
            .to_string();
        Ok(key)
    }

    pub async fn keepalive_listen_key(&self, listen_key: &str) -> Result<()> {
        let resp = self
            .client
            .put(format!("{}/api/v3/userDataStream", BINANCE_REST_URL))
            .header("X-MBX-APIKEY", &self.api_key)
            .query(&[("listenKey", listen_key)])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(anyhow::anyhow!("keepalive_listen_key failed: {}", resp.status()));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Binance REST response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AccountBalance {
    pub asset: String,
    pub free: String,
    pub locked: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub balances: Vec<AccountBalance>,
    pub total_net_asset_of_btc: Option<String>,
}

// ---------------------------------------------------------------------------
// UDS event types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UdsEvent {
    #[serde(rename = "e")]
    event_type: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionReport {
    #[serde(rename = "i")]
    order_id: u64,
    #[serde(rename = "c")]
    client_order_id: String,
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "X")]
    order_status: String,
    #[serde(rename = "z")]
    cumulative_qty: String,
}

#[derive(Debug, Deserialize)]
struct AccountUpdateBalance {
    #[serde(rename = "a")]
    asset: String,
    #[serde(rename = "wb")]
    wallet_balance: String,
}

#[derive(Debug, Deserialize)]
struct AccountUpdate {
    #[serde(rename = "B")]
    balances: Vec<AccountUpdateBalance>,
}

// ---------------------------------------------------------------------------
// Execution actor
// ---------------------------------------------------------------------------

struct ExecutionActor {
    rest: BinanceRest,
    order_book: OrderBook,
    positions: PositionsSnapshot,
    cmd_rx: mpsc::Receiver<ExecutionCommand>,
    trading_enabled: bool,
    last_uds_update_ms: u64,
}

impl ExecutionActor {
    fn new(api_key: String, secret: String, cmd_rx: mpsc::Receiver<ExecutionCommand>) -> Self {
        Self {
            rest: BinanceRest::new(api_key, secret),
            order_book: OrderBook::new(),
            positions: PositionsSnapshot::default(),
            cmd_rx,
            trading_enabled: false,
            last_uds_update_ms: 0,
        }
    }

    async fn reconcile_from_rest(&mut self) -> Result<()> {
        let account = self.rest.get_account().await?;
        let mut equity = 0.0f64;
        for bal in &account.balances {
            if bal.asset == "USDT" {
                let free: f64 = bal.free.parse().unwrap_or(0.0);
                let locked: f64 = bal.locked.parse().unwrap_or(0.0);
                equity += free + locked;
            }
        }
        self.positions.equity_usd = equity;
        self.positions.ts_ms = now_ms();
        POSITIONS_SNAPSHOT.store(Arc::new(self.positions.clone()));
        info!(equity, "Reconciled from REST");
        Ok(())
    }

    async fn flatten_all(&mut self) -> Result<()> {
        info!("Flattening all positions on startup");
        let coids = self.order_book.cancel_all_working();
        for coid in &coids {
            if let Some(o) = self.order_book.orders.get(coid) {
                let _ = self.rest.cancel_order(&o.symbol.clone(), coid).await;
            }
        }
        Ok(())
    }

    async fn run(mut self) {
        if let Err(e) = self.reconcile_from_rest().await {
            error!(err = %e, "reconcile_from_rest failed; proceeding cautiously");
        }
        if let Err(e) = self.flatten_all().await {
            error!(err = %e, "flatten_all failed");
        }

        let mut backoff_ms = 1_000u64;
        loop {
            match self.rest.create_listen_key().await {
                Ok(key) => {
                    self.trading_enabled = true;
                    match self.run_uds_loop(&key).await {
                        Ok(_) => {
                            warn!("UDS loop ended — reconnecting");
                        }
                        Err(e) => {
                            error!(err = %e, "UDS loop error — reconnecting after {}ms", backoff_ms);
                            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                            backoff_ms = (backoff_ms * 2).min(30_000);
                        }
                    }
                }
                Err(e) => {
                    error!(err = %e, "create_listen_key failed");
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(30_000);
                }
            }
            self.trading_enabled = false;
        }
    }

    async fn run_uds_loop(&mut self, listen_key: &str) -> Result<()> {
        let ws_url = format!("{}/{}", BINANCE_WS_BASE, listen_key);
        info!("Connecting to UDS: {}", ws_url);
        let (ws_stream, _) = connect_async(&ws_url).await?;
        let (mut write, mut read) = ws_stream.split();
        info!("UDS connected");

        self.last_uds_update_ms = now_ms();
        let mut keepalive = interval(Duration::from_millis(UDS_KEEPALIVE_MS));
        keepalive.set_missed_tick_behavior(MissedTickBehavior::Skip);
        // Skip the first immediate tick.
        keepalive.tick().await;

        let mut staleness_check = interval(Duration::from_millis(UDS_TIMEOUT_MS * 3));
        staleness_check.set_missed_tick_behavior(MissedTickBehavior::Skip);
        staleness_check.tick().await;

        loop {
            tokio::select! {
                msg = read.next() => {
                    match msg {
                        None => {
                            warn!("UDS stream ended");
                            return Ok(());
                        }
                        Some(Err(e)) => return Err(e.into()),
                        Some(Ok(Message::Text(txt))) => {
                            self.last_uds_update_ms = now_ms();
                            self.handle_uds_message(&txt);
                        }
                        Some(Ok(Message::Ping(p))) => {
                            if let Err(e) = write.send(Message::Pong(p)).await {
                                return Err(e.into());
                            }
                        }
                        Some(Ok(Message::Close(_))) => {
                            warn!("UDS close frame");
                            return Ok(());
                        }
                        Some(Ok(_)) => {}
                    }
                }
                _ = keepalive.tick() => {
                    if let Err(e) = self.rest.keepalive_listen_key(listen_key).await {
                        warn!(err = %e, "keepalive_listen_key failed");
                    }
                }
                _ = staleness_check.tick() => {
                    let age = now_ms().saturating_sub(self.last_uds_update_ms);
                    if age > UDS_TIMEOUT_MS * 3 {
                        warn!(age_ms = age, "UDS stale — reconnecting");
                        return Ok(());
                    }
                }
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        None => {
                            info!("cmd_rx closed — shutting down execution actor");
                            return Ok(());
                        }
                        Some(c) => {
                            if let Err(e) = self.handle_command(c).await {
                                warn!(err = %e, "command handler error");
                            }
                        }
                    }
                }
            }
        }
    }

    fn handle_uds_message(&mut self, txt: &str) {
        let event: UdsEvent = match serde_json::from_str(txt) {
            Ok(e) => e,
            Err(err) => {
                warn!(%err, "failed to parse UDS event type");
                return;
            }
        };

        match event.event_type.as_str() {
            "executionReport" | "ORDER_TRADE_UPDATE" => {
                if let Ok(report) = serde_json::from_str::<ExecutionReport>(txt) {
                    match report.order_status.as_str() {
                        "FILLED" | "PARTIALLY_FILLED" => {
                            let cum_qty: f64 = report.cumulative_qty.parse().unwrap_or(0.0);
                            // Snapshot fill delta and order metadata before mutating the book.
                            let fill_info = self.order_book.orders.get(&report.client_order_id).map(|o| {
                                let delta = (cum_qty - o.filled_qty).max(0.0);
                                (o.side.clone(), o.price, o.symbol.clone(), delta)
                            });
                            self.order_book.apply_fill(&report.client_order_id, cum_qty);
                            // Update position tracking for each incremental fill.
                            if let Some((side, price, symbol, delta)) = fill_info {
                                if delta > 1e-9 {
                                    let sign = match side {
                                        OrderSide::Buy => 1.0,
                                        OrderSide::Sell => -1.0,
                                    };
                                    self.update_position(&symbol, sign * delta, price);
                                }
                            }
                        }
                        "CANCELED" => {
                            self.order_book.apply_cancel_ack(&report.client_order_id);
                        }
                        "EXPIRED" => {
                            self.order_book.apply_expired(&report.client_order_id);
                        }
                        _ => {}
                    }
                    self.order_book.clear_terminals();
                    self.publish_working_notional();
                }
            }
            "outboundAccountPosition" | "ACCOUNT_UPDATE" => {
                if let Ok(update) = serde_json::from_str::<serde_json::Value>(txt) {
                    if let Some(balances) = update.get("B").and_then(|b| b.as_array()) {
                        for bal in balances {
                            let asset = bal.get("a").and_then(|a| a.as_str()).unwrap_or("");
                            let wb: f64 = bal
                                .get("wb")
                                .and_then(|v| v.as_str())
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0.0);
                            if asset == "USDT" {
                                self.positions.equity_usd = wb;
                            }
                        }
                    }
                    self.positions.ts_ms = now_ms();
                    POSITIONS_SNAPSHOT.store(Arc::new(self.positions.clone()));
                }
            }
            _ => {}
        }
    }

    /// Update the in-memory position for `symbol` using a fill of `delta_qty` lots
    /// at `fill_px`, track cost basis via weighted-average entry price, compute
    /// realized PnL for any quantity that closes or flips the position, and
    /// publish the updated snapshot to `POSITIONS_SNAPSHOT`.
    fn update_position(&mut self, symbol: &str, delta_qty: f64, fill_px: f64) {
        if let Some(pos) = self.positions.positions.iter_mut().find(|p| p.symbol == symbol) {
            let cur_qty = pos.position_qty;
            let cur_px = pos.avg_entry_px;
            let new_qty = cur_qty + delta_qty;

            // Realized PnL: only when the fill reduces, closes, or flips the position.
            if cur_qty.abs() > 1e-9 && cur_qty * delta_qty < 0.0 {
                let closed = delta_qty.abs().min(cur_qty.abs());
                // Long: profit when fill_px > entry; short: profit when fill_px < entry.
                pos.realized_pnl += (fill_px - cur_px) * closed * cur_qty.signum();
            }

            if new_qty.abs() < 1e-9 {
                // Fully closed.
                pos.position_qty = 0.0;
                pos.position_usd = 0.0;
                pos.avg_entry_px = 0.0;
            } else if cur_qty.abs() < 1e-9 || new_qty * cur_qty < 0.0 {
                // New position (zero existing) or flipped direction.
                pos.avg_entry_px = fill_px;
                pos.position_qty = new_qty;
                pos.position_usd = new_qty * fill_px;
            } else if delta_qty * cur_qty > 0.0 {
                // Adding to existing position: weighted-average entry.
                let total = cur_qty.abs() + delta_qty.abs();
                pos.avg_entry_px =
                    (cur_qty.abs() * cur_px + delta_qty.abs() * fill_px) / total;
                pos.position_qty = new_qty;
                pos.position_usd = new_qty * pos.avg_entry_px;
            } else {
                // Partially reducing: entry price is unchanged for remaining qty.
                pos.position_qty = new_qty;
                pos.position_usd = new_qty * cur_px;
            }
        } else {
            self.positions.positions.push(PositionState {
                symbol: symbol.to_string(),
                position_qty: delta_qty,
                position_usd: delta_qty * fill_px,
                realized_pnl: 0.0,
                avg_entry_px: fill_px,
            });
        }
        self.positions.ts_ms = now_ms();
        POSITIONS_SNAPSHOT.store(Arc::new(self.positions.clone()));
    }

    fn publish_working_notional(&self) {
        let mut by_symbol: HashMap<String, f64> = HashMap::new();
        for o in self.order_book.orders.values() {
            if matches!(o.status, OrderStatus::New | OrderStatus::PartiallyFilled) {
                *by_symbol.entry(o.symbol.clone()).or_insert(0.0) += o.working_notional();
            }
        }
        WORKING_NOTIONAL.store(Arc::new(WorkingNotionalSnapshot { by_symbol }));
    }

    async fn handle_command(&mut self, cmd: ExecutionCommand) -> Result<()> {
        if !self.trading_enabled {
            warn!("command received but trading not enabled yet");
            return Ok(());
        }
        match cmd {
            ExecutionCommand::SendOrder { symbol, side, price, qty, ttl_ms } => {
                let cur = self.positions.position_usd(&symbol);
                let signed_delta = match side {
                    OrderSide::Buy => qty * price,
                    OrderSide::Sell => -(qty * price),
                };
                if signed_delta.abs() < DELTA_EPSILON_USD {
                    return Ok(());
                }
                let coid = uuid::Uuid::new_v4().to_string();
                let side_str = match side {
                    OrderSide::Buy => "BUY",
                    OrderSide::Sell => "SELL",
                };
                self.rest.send_limit_order(&symbol, side_str, price, qty, &coid).await?;
                self.order_book.insert(Order {
                    client_order_id: coid,
                    symbol,
                    side,
                    price,
                    qty,
                    filled_qty: 0.0,
                    status: OrderStatus::New,
                    created_at_ms: now_ms(),
                    ttl_ms,
                });
                let _ = cur;
            }
            ExecutionCommand::CancelOrder { client_order_id } => {
                if let Some(o) = self.order_book.orders.get(&client_order_id) {
                    let sym = o.symbol.clone();
                    self.rest.cancel_order(&sym, &client_order_id).await?;
                    self.order_book.set_cancel_pending(&client_order_id);
                }
            }
            ExecutionCommand::CancelAll => {
                let coids = self.order_book.cancel_all_working();
                for coid in coids {
                    if let Some(o) = self.order_book.orders.get(&coid) {
                        let sym = o.symbol.clone();
                        if let Err(e) = self.rest.cancel_order(&sym, &coid).await {
                            warn!(err = %e, coid, "cancel_order failed");
                        }
                    }
                }
            }
            ExecutionCommand::FlattenAll => {
                let coids = self.order_book.cancel_all_working();
                for coid in coids {
                    if let Some(o) = self.order_book.orders.get(&coid) {
                        let sym = o.symbol.clone();
                        if let Err(e) = self.rest.cancel_order(&sym, &coid).await {
                            warn!(err = %e, coid, "cancel during FlattenAll failed");
                        }
                    }
                }
                // Send market orders to close any open positions.
                let positions_to_close: Vec<(String, f64)> = self.positions.positions
                    .iter()
                    .filter(|p| p.position_qty.abs() > 1e-9)
                    .map(|p| (p.symbol.clone(), p.position_qty))
                    .collect();
                for (symbol, qty) in positions_to_close {
                    let (side, close_qty) = if qty > 0.0 {
                        ("SELL", qty)
                    } else {
                        ("BUY", -qty)
                    };
                    let coid = uuid::Uuid::new_v4().to_string();
                    if let Err(e) = self.rest.send_market_order(&symbol, side, close_qty, &coid).await {
                        warn!(err = %e, %symbol, "send_market_order during FlattenAll failed");
                    }
                }
            }
        }
        self.publish_working_notional();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public spawn function
// ---------------------------------------------------------------------------

pub fn spawn_execution(api_key: String, secret: String) -> mpsc::Sender<ExecutionCommand> {
    let (tx, rx) = mpsc::channel::<ExecutionCommand>(256);
    let actor = ExecutionActor::new(api_key, secret, rx);
    tokio::spawn(async move {
        actor.run().await;
    });
    tx
}
