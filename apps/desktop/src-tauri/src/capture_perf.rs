//! Rolling per-phase capture performance stats for the Diagnostics UI.
//!
//! Tracks the last ~120 samples per monitor (≈ 2 minutes at 1 s interval) for
//! each phase: screen capture (`capture_image`), downscale, save (JPEG encode +
//! write), and total tick time.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use serde::Serialize;

use crate::worker_activity::StageTimingStats;

const PER_MONITOR_WINDOW: usize = 120;
const OVERALL_WINDOW: usize = 240;

#[derive(Clone, Debug, Default)]
struct PhaseSamples {
    capture: VecDeque<u64>,
    downscale: VecDeque<u64>,
    save: VecDeque<u64>,
    total: VecDeque<u64>,
}

impl PhaseSamples {
    fn push(
        &mut self,
        capture: u64,
        downscale: u64,
        save: u64,
        total: u64,
    ) {
        push_capped(&mut self.capture, capture, PER_MONITOR_WINDOW);
        push_capped(&mut self.downscale, downscale, PER_MONITOR_WINDOW);
        push_capped(&mut self.save, save, PER_MONITOR_WINDOW);
        push_capped(&mut self.total, total, PER_MONITOR_WINDOW);
    }

    fn summarize(&self) -> PerPhaseStats {
        PerPhaseStats {
            sample_count: self.total.len(),
            capture: summarize_ms(&self.capture),
            downscale: summarize_ms(&self.downscale),
            save: summarize_ms(&self.save),
            total: summarize_ms(&self.total),
        }
    }
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PerPhaseStats {
    pub sample_count: usize,
    pub capture: StageTimingStats,
    pub downscale: StageTimingStats,
    pub save: StageTimingStats,
    pub total: StageTimingStats,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MonitorCapturePerf {
    pub monitor_id: i32,
    pub monitor_name: String,
    pub stats: PerPhaseStats,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CapturePerfSnapshot {
    pub overall: PerPhaseStats,
    pub by_monitor: Vec<MonitorCapturePerf>,
    /// Stats for frames that were actually saved to disk (excludes skipped/unchanged).
    pub saved_overall: PerPhaseStats,
    pub saved_by_monitor: Vec<MonitorCapturePerf>,
    /// Duration of the last full tick (enum + all monitors), ms.
    pub last_tick_ms: u64,
    /// Duration of monitor enumeration in the last tick, ms.
    pub last_tick_enum_ms: u64,
}

pub struct CapturePerf {
    by_monitor: Mutex<HashMap<i32, PhaseSamples>>,
    monitor_names: Mutex<HashMap<i32, String>>,
    overall: Mutex<PhaseSamples>,
    saved_by_monitor: Mutex<HashMap<i32, PhaseSamples>>,
    saved_overall: Mutex<PhaseSamples>,
    last_tick: Mutex<(u64, u64)>, // (total_ms, enum_ms)
}

impl Default for CapturePerf {
    fn default() -> Self {
        Self {
            by_monitor: Mutex::new(HashMap::new()),
            monitor_names: Mutex::new(HashMap::new()),
            overall: Mutex::new(PhaseSamples::default()),
            saved_by_monitor: Mutex::new(HashMap::new()),
            saved_overall: Mutex::new(PhaseSamples::default()),
            last_tick: Mutex::new((0, 0)),
        }
    }
}

impl CapturePerf {
    pub fn record(
        &self,
        monitor_id: i32,
        monitor_name: &str,
        capture_ms: u64,
        _hash_ms: u64,
        downscale_ms: u64,
        save_ms: u64,
        total_ms: u64,
    ) {
        if let Ok(mut g) = self.by_monitor.lock() {
            g.entry(monitor_id)
                .or_default()
                .push(capture_ms, downscale_ms, save_ms, total_ms);
        }
        if let Ok(mut g) = self.monitor_names.lock() {
            g.insert(monitor_id, monitor_name.to_string());
        }
        if let Ok(mut g) = self.overall.lock() {
            push_capped(&mut g.capture, capture_ms, OVERALL_WINDOW);
            push_capped(&mut g.downscale, downscale_ms, OVERALL_WINDOW);
            push_capped(&mut g.save, save_ms, OVERALL_WINDOW);
            push_capped(&mut g.total, total_ms, OVERALL_WINDOW);
        }
    }

    pub fn record_saved(
        &self,
        monitor_id: i32,
        capture_ms: u64,
        _hash_ms: u64,
        downscale_ms: u64,
        save_ms: u64,
        total_ms: u64,
    ) {
        if let Ok(mut g) = self.saved_by_monitor.lock() {
            g.entry(monitor_id)
                .or_default()
                .push(capture_ms, downscale_ms, save_ms, total_ms);
        }
        if let Ok(mut g) = self.saved_overall.lock() {
            push_capped(&mut g.capture, capture_ms, OVERALL_WINDOW);
            push_capped(&mut g.downscale, downscale_ms, OVERALL_WINDOW);
            push_capped(&mut g.save, save_ms, OVERALL_WINDOW);
            push_capped(&mut g.total, total_ms, OVERALL_WINDOW);
        }
    }

    pub fn set_last_tick(&self, total_ms: u64, enum_ms: u64) {
        if let Ok(mut g) = self.last_tick.lock() {
            *g = (total_ms, enum_ms);
        }
    }

    pub fn snapshot(&self) -> CapturePerfSnapshot {
        let names = match self.monitor_names.lock() {
            Ok(guard) => guard,
            Err(_) => return CapturePerfSnapshot::default(),
        };

        let mut monitors: Vec<MonitorCapturePerf> = match self.by_monitor.lock() {
            Ok(guard) => guard
                .iter()
                .map(|(&id, samples)| MonitorCapturePerf {
                    monitor_id: id,
                    monitor_name: names.get(&id).cloned().unwrap_or_else(|| format!("Monitor {id}")),
                    stats: samples.summarize(),
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        monitors.sort_by_key(|m| m.monitor_id);

        let saved_guard = self.saved_by_monitor.lock().ok();
        let mut saved_monitors: Vec<MonitorCapturePerf> = Vec::new();
        // Include all known monitors even if they have 0 saved samples.
        for &id in names.keys() {
            let stats = saved_guard
                .as_ref()
                .and_then(|g| g.get(&id))
                .map(|s| s.summarize())
                .unwrap_or_default();
            saved_monitors.push(MonitorCapturePerf {
                monitor_id: id,
                monitor_name: names.get(&id).cloned().unwrap_or_else(|| format!("Monitor {id}")),
                stats,
            });
        }
        drop(saved_guard);
        drop(names);
        saved_monitors.sort_by_key(|m| m.monitor_id);

        let overall = self
            .overall
            .lock()
            .ok()
            .map(|g| g.summarize())
            .unwrap_or_default();
        let saved_overall = self
            .saved_overall
            .lock()
            .ok()
            .map(|g| g.summarize())
            .unwrap_or_default();
        let (last_tick_ms, last_tick_enum_ms) =
            self.last_tick.lock().ok().map(|g| *g).unwrap_or((0, 0));

        CapturePerfSnapshot {
            overall,
            by_monitor: monitors,
            saved_overall,
            saved_by_monitor: saved_monitors,
            last_tick_ms,
            last_tick_enum_ms,
        }
    }
}

fn push_capped(deque: &mut VecDeque<u64>, value: u64, cap: usize) {
    if deque.len() >= cap {
        deque.pop_front();
    }
    deque.push_back(value);
}

fn summarize_ms(samples: &VecDeque<u64>) -> StageTimingStats {
    if samples.is_empty() {
        return StageTimingStats::default();
    }
    let mut s: Vec<u64> = samples.iter().copied().collect();
    s.sort_unstable();
    let n = s.len();
    let sum: u64 = s.iter().sum();
    let mean = sum as f64 / n as f64;
    let p50 = if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2
    };
    let p95_i = ((n as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(n - 1);
    StageTimingStats {
        sample_count: n,
        min_ms: Some(s[0]),
        max_ms: Some(s[n - 1]),
        mean_ms: Some(mean),
        p50_ms: Some(p50),
        p95_ms: Some(s[p95_i]),
    }
}
