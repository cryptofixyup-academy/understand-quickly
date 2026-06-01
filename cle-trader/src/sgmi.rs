/// Synthetic General Market Intelligence (SGMI) monitor.
///
/// Aggregates detector scores into a single [0,1] threat index.
/// A warm-up grace period lets the system stabilize before the gate becomes active.
use crate::ingestion::now_ms;
use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use std::collections::HashMap;
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

pub static SGMI_CACHE: Lazy<ArcSwap<SgmiScore>> = Lazy::new(|| {
    ArcSwap::from_pointee(SgmiScore::zero(0, true))
});

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

    SgmiGateResult {
        decision,
        score,
        age_ms,
        override_active,
    }
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
// Background monitor task
// ---------------------------------------------------------------------------

/// Stub detectors — replace with real signal extraction once data sources are wired.
fn run_eigenvalue_spike_detector() -> DetectorScore {
    DetectorScore { name: "eigenvalue_spike", score: 0.0, weight: 0.3 }
}

fn run_cross_agent_cosine_detector() -> DetectorScore {
    DetectorScore { name: "cross_agent_cosine", score: 0.0, weight: 0.3 }
}

fn run_temporal_autocorr_detector() -> DetectorScore {
    DetectorScore { name: "temporal_autocorr", score: 0.0, weight: 0.2 }
}

fn run_ontology_failure_rate_detector() -> DetectorScore {
    DetectorScore { name: "ontology_failure_rate", score: 0.0, weight: 0.2 }
}

pub async fn run_sgmi_monitor(process_start_ms: u64) {
    info!("SGMI monitor starting (warmup {}s)", SGMI_WARMUP_MS / 1_000);
    let mut tick = interval(Duration::from_millis(SGMI_PERIOD_MS));

    loop {
        tick.tick().await;
        let now = now_ms();
        let in_warmup = now.saturating_sub(process_start_ms) < SGMI_WARMUP_MS;

        let components = vec![
            run_eigenvalue_spike_detector(),
            run_cross_agent_cosine_detector(),
            run_temporal_autocorr_detector(),
            run_ontology_failure_rate_detector(),
        ];

        let score = aggregate_score(&components);
        if score >= SGMI_SOFT_THRESHOLD {
            warn!(score, in_warmup, "SGMI score elevated");
        }

        update_sgmi_cache(SgmiScore { ts_ms: now, score, components, in_warmup });
    }
}
