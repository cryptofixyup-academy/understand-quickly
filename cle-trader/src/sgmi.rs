/// Synthetic General Market Intelligence (SGMI) monitor.
///
/// Aggregates detector scores into a single [0,1] threat index.
/// A warm-up grace period lets the system stabilize before the gate becomes active.
use crate::ingestion::now_ms;
use crate::state::{SymbolState, STATE_SNAPSHOT};
use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const SGMI_PERIOD_MS: u64 = 1_000;
const SGMI_MAX_AGE_MS: u64 = 2_000;
const SGMI_WARMUP_MS: u64 = 10 * 60 * 1_000; // 10 min
const SGMI_HARD_THRESHOLD: f64 = 0.8;
const SGMI_SOFT_THRESHOLD: f64 = 0.5;
/// Rolling window depth for spread and return histories (ticks ≈ seconds at 1 Hz).
const DETECTOR_WINDOW: usize = 60;
/// Symbols not updated within this window are counted as stale.
const STALE_SYMBOL_MS: u64 = 5_000;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct DetectorScore {
    pub name: &'static str,
    pub score: f64,
    pub weight: f64,
}

#[derive(Clone, Debug)]
pub struct SgmiScore {
    pub ts_ms: u64,
    pub score: f64,
    pub components: Vec<DetectorScore>,
    pub in_warmup: bool,
}

impl SgmiScore {
    pub fn zero(ts_ms: u64, in_warmup: bool) -> Self {
        Self {
            ts_ms,
            score: 0.0,
            components: vec![],
            in_warmup,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SgmiGateDecision {
    Pass,
    SoftBlock,
    HardBlock,
}

#[derive(Clone, Debug)]
pub struct SgmiGateResult {
    pub decision: SgmiGateDecision,
    pub score: f64,
    pub age_ms: u64,
    pub override_active: bool,
}

// ---------------------------------------------------------------------------
// Global cache
// ---------------------------------------------------------------------------

pub static SGMI_CACHE: Lazy<ArcSwap<SgmiScore>> =
    Lazy::new(|| ArcSwap::from_pointee(SgmiScore::zero(0, true)));

pub fn update_sgmi_cache(score: SgmiScore) {
    SGMI_CACHE.store(Arc::new(score));
}

// ---------------------------------------------------------------------------
// Core logic
// ---------------------------------------------------------------------------

/// Weighted max: max_i(w_i * c_i) / max_i(w_i).
pub fn aggregate_score(components: &[DetectorScore]) -> f64 {
    if components.is_empty() {
        return 0.0;
    }
    let max_weighted = components
        .iter()
        .map(|d| d.weight * d.score)
        .fold(f64::NEG_INFINITY, f64::max);
    let max_weight = components
        .iter()
        .map(|d| d.weight)
        .fold(f64::NEG_INFINITY, f64::max);
    if max_weight <= 0.0 {
        return 0.0;
    }
    (max_weighted / max_weight).clamp(0.0, 1.0)
}

pub fn evaluate_sgmi_gate(now_ms: u64, override_active: bool) -> SgmiGateResult {
    let cached = SGMI_CACHE.load();
    let age_ms = now_ms.saturating_sub(cached.ts_ms);

    // During warm-up always pass.
    if cached.in_warmup {
        return SgmiGateResult {
            decision: SgmiGateDecision::Pass,
            score: cached.score,
            age_ms,
            override_active,
        };
    }

    // Stale data → hard block.
    if age_ms > SGMI_MAX_AGE_MS {
        return SgmiGateResult {
            decision: SgmiGateDecision::HardBlock,
            score: cached.score,
            age_ms,
            override_active,
        };
    }

    let score = cached.score;
    let decision = if override_active {
        SgmiGateDecision::Pass
    } else if score >= SGMI_HARD_THRESHOLD {
        SgmiGateDecision::HardBlock
    } else if score >= SGMI_SOFT_THRESHOLD {
        SgmiGateDecision::SoftBlock
    } else {
        SgmiGateDecision::Pass
    };

    SgmiGateResult { decision, score, age_ms, override_active }
}

/// Returns true when deltas, if applied, would decrease gross USD exposure
/// (sum of |position_usd|) by more than $1 epsilon.
pub fn reduces_gross_exposure(
    deltas: &[(String, f64)],
    current: &HashMap<String, f64>,
) -> bool {
    const EPSILON: f64 = 1.0;
    let delta_map: HashMap<&str, f64> = deltas.iter().map(|(s, d)| (s.as_str(), *d)).collect();
    let symbols: std::collections::HashSet<&str> = current
        .keys()
        .map(|s| s.as_str())
        .chain(delta_map.keys().copied())
        .collect();

    let before: f64 = current.values().map(|v| v.abs()).sum();
    let after: f64 = symbols
        .iter()
        .map(|sym| {
            let cur = current.get(*sym).copied().unwrap_or(0.0);
            let d = delta_map.get(*sym).copied().unwrap_or(0.0);
            (cur + d).abs()
        })
        .sum();

    before - after > EPSILON
}

// ---------------------------------------------------------------------------
// Detector state — rolling windows maintained across ticks.
// ---------------------------------------------------------------------------

struct SgmiDetectorState {
    /// Rolling mean relative bid-ask spreads (per-tick cross-symbol mean).
    spread_history: VecDeque<f64>,
    spread_sum: f64,
    spread_sum_sq: f64,
    /// Rolling market-wide mean price returns (tick-over-tick).
    return_history: VecDeque<f64>,
    prev_mean_mid: Option<f64>,
}

impl SgmiDetectorState {
    fn new() -> Self {
        Self {
            spread_history: VecDeque::with_capacity(DETECTOR_WINDOW + 1),
            spread_sum: 0.0,
            spread_sum_sq: 0.0,
            return_history: VecDeque::with_capacity(DETECTOR_WINDOW + 1),
            prev_mean_mid: None,
        }
    }

    fn update(&mut self, symbols: &[SymbolState]) {
        let valid: Vec<&SymbolState> = symbols.iter().filter(|s| s.mid_price > 0.0).collect();
        if valid.is_empty() {
            return;
        }
        let n = valid.len() as f64;
        let mean_spread =
            valid.iter().map(|s| (s.best_ask - s.best_bid) / s.mid_price).sum::<f64>() / n;
        let mean_mid = valid.iter().map(|s| s.mid_price).sum::<f64>() / n;

        // Evict oldest sample before pushing the new one.
        if self.spread_history.len() >= DETECTOR_WINDOW {
            let old = self.spread_history.pop_front().unwrap();
            self.spread_sum -= old;
            self.spread_sum_sq -= old * old;
        }
        self.spread_history.push_back(mean_spread);
        self.spread_sum += mean_spread;
        self.spread_sum_sq += mean_spread * mean_spread;

        if let Some(prev) = self.prev_mean_mid {
            if prev > 0.0 {
                if self.return_history.len() >= DETECTOR_WINDOW {
                    self.return_history.pop_front();
                }
                self.return_history.push_back((mean_mid - prev) / prev);
            }
        }
        self.prev_mean_mid = Some(mean_mid);
    }
}

// ---------------------------------------------------------------------------
// Detectors
// ---------------------------------------------------------------------------

/// Approximates an eigenvalue spike via z-scored aggregate bid-ask spread.
/// A spread anomaly > 3σ above baseline is the primary observable proxy for
/// covariance matrix eigenvalue dominance that occurs during market stress.
fn run_eigenvalue_spike_detector(state: &SgmiDetectorState) -> DetectorScore {
    let score = if state.spread_history.len() < 10 {
        0.0
    } else {
        let n = state.spread_history.len() as f64;
        let mean = state.spread_sum / n;
        let variance = (state.spread_sum_sq / n) - mean * mean;
        if variance < 1e-20 {
            0.0
        } else {
            let current = *state.spread_history.back().unwrap();
            let z = (current - mean) / variance.sqrt();
            // Score saturates toward 1 at z = 3σ.
            (z.max(0.0) / 3.0).min(1.0)
        }
    };
    DetectorScore { name: "eigenvalue_spike", score, weight: 0.3 }
}

/// Scores pairwise cosine similarity of agent proposal target vectors.
/// Zero until cognition agents register active proposals — herding risk
/// (mean off-diagonal cosine similarity) would then be scored here.
fn run_cross_agent_cosine_detector() -> DetectorScore {
    DetectorScore { name: "cross_agent_cosine", score: 0.0, weight: 0.3 }
}

/// Lag-1 autocorrelation of the market-wide mean price return.
/// Sustained directional momentum indicates a feedback loop or
/// microstructure anomaly that warrants position de-risking.
fn run_temporal_autocorr_detector(state: &SgmiDetectorState) -> DetectorScore {
    let score = if state.return_history.len() < 10 {
        0.0
    } else {
        let rets: Vec<f64> = state.return_history.iter().copied().collect();
        let n = rets.len() as f64;
        let mean = rets.iter().sum::<f64>() / n;
        let variance = rets.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n;
        if variance < 1e-20 {
            0.0
        } else {
            let autocov: f64 = rets
                .windows(2)
                .map(|w| (w[0] - mean) * (w[1] - mean))
                .sum::<f64>()
                / (rets.len() - 1) as f64;
            (autocov / variance).clamp(-1.0, 1.0).abs()
        }
    };
    DetectorScore { name: "temporal_autocorr", score, weight: 0.2 }
}

/// Fraction of tracked symbols whose last price event is older than
/// STALE_SYMBOL_MS, used as a proxy for ingestion or parse failure rate.
fn run_ontology_failure_rate_detector(now: u64, symbols: &[SymbolState]) -> DetectorScore {
    let score = if symbols.is_empty() {
        0.0
    } else {
        let stale =
            symbols.iter().filter(|s| now.saturating_sub(s.ts_ms) > STALE_SYMBOL_MS).count();
        stale as f64 / symbols.len() as f64
    };
    DetectorScore { name: "ontology_failure_rate", score, weight: 0.2 }
}

// ---------------------------------------------------------------------------
// Background monitor task
// ---------------------------------------------------------------------------

pub async fn run_sgmi_monitor(process_start_ms: u64) {
    info!("SGMI monitor starting (warmup {}s)", SGMI_WARMUP_MS / 1_000);
    let mut tick = interval(Duration::from_millis(SGMI_PERIOD_MS));
    let mut detector_state = SgmiDetectorState::new();

    loop {
        tick.tick().await;
        let now = now_ms();
        let in_warmup = now.saturating_sub(process_start_ms) < SGMI_WARMUP_MS;

        let snapshot = STATE_SNAPSHOT.load();
        detector_state.update(&snapshot.symbols);

        let components = vec![
            run_eigenvalue_spike_detector(&detector_state),
            run_cross_agent_cosine_detector(),
            run_temporal_autocorr_detector(&detector_state),
            run_ontology_failure_rate_detector(now, &snapshot.symbols),
        ];

        let score = aggregate_score(&components);
        if score >= SGMI_SOFT_THRESHOLD {
            warn!(score, in_warmup, "SGMI score elevated");
        }

        update_sgmi_cache(SgmiScore { ts_ms: now, score, components, in_warmup });
    }
}
