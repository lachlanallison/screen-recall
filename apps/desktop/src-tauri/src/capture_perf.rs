//! Rolling per-phase capture performance stats for the Diagnostics UI.
//!
//! Tracks the last ~120 samples per monitor for each phase. Overall stats are
//! computed on-the-fly by aggregating all per-monitor samples, so the overall
//! max/min never loses a sample just because another monitor is active.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use serde::Serialize;

use crate::worker_activity::StageTimingStats;

fn summarize_deque(samples: &VecDeque<u64>) -> StageTimingStats {
    let v: Vec<u64> = samples.iter().copied().collect();
    crate::worker_activity::summarize_ms(&v)
}

fn summarize_merged(samples: &[Vec<u64>]) -> StageTimingStats {
    let total_len: usize = samples.iter().map(|s| s.len()).sum();
    if total_len == 0 {
        return StageTimingStats::default();
    }
    let mut flat: Vec<u64> = Vec::with_capacity(total_len);
    for s in samples {
        flat.extend_from_slice(s);
    }
    crate::worker_activity::summarize_ms(&flat)
}

const PER_MONITOR_WINDOW: usize = 120;

#[derive(Clone, Debug, Default)]
struct PhaseSamples {
    capture: VecDeque<u64>,
    dhash: VecDeque<u64>,
    downscale: VecDeque<u64>,
    save: VecDeque<u64>,
    db: VecDeque<u64>,
    total: VecDeque<u64>,
}

impl PhaseSamples {
    fn push(&mut self, capture: u64, dhash: u64, downscale: u64, save: u64, db: u64, total: u64) {
        push_capped(&mut self.capture, capture, PER_MONITOR_WINDOW);
        push_capped(&mut self.dhash, dhash, PER_MONITOR_WINDOW);
        push_capped(&mut self.downscale, downscale, PER_MONITOR_WINDOW);
        push_capped(&mut self.save, save, PER_MONITOR_WINDOW);
        push_capped(&mut self.db, db, PER_MONITOR_WINDOW);
        push_capped(&mut self.total, total, PER_MONITOR_WINDOW);
    }

    fn summarize(&self) -> PerPhaseStats {
        PerPhaseStats {
            sample_count: self.total.len(),
            capture: summarize_deque(&self.capture),
            dhash: summarize_deque(&self.dhash),
            downscale: summarize_deque(&self.downscale),
            save: summarize_deque(&self.save),
            db: summarize_deque(&self.db),
            total: summarize_deque(&self.total),
        }
    }
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PerPhaseStats {
    pub sample_count: usize,
    pub capture: StageTimingStats,
    pub dhash: StageTimingStats,
    pub downscale: StageTimingStats,
    pub save: StageTimingStats,
    pub db: StageTimingStats,
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
    saved_by_monitor: Mutex<HashMap<i32, PhaseSamples>>,
    last_tick: Mutex<(u64, u64)>, // (total_ms, enum_ms)
}

impl Default for CapturePerf {
    fn default() -> Self {
        Self {
            by_monitor: Mutex::new(HashMap::new()),
            monitor_names: Mutex::new(HashMap::new()),
            saved_by_monitor: Mutex::new(HashMap::new()),
            last_tick: Mutex::new((0, 0)),
        }
    }
}

impl CapturePerf {
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        monitor_id: i32,
        monitor_name: &str,
        capture_ms: u64,
        dhash_ms: u64,
        downscale_ms: u64,
        save_ms: u64,
        db_ms: u64,
        total_ms: u64,
    ) {
        if let Ok(mut g) = self.by_monitor.lock() {
            g.entry(monitor_id).or_default().push(
                capture_ms,
                dhash_ms,
                downscale_ms,
                save_ms,
                db_ms,
                total_ms,
            );
        }
        if !monitor_name.is_empty() {
            if let Ok(mut g) = self.monitor_names.lock() {
                g.insert(monitor_id, monitor_name.to_string());
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_saved(
        &self,
        monitor_id: i32,
        capture_ms: u64,
        dhash_ms: u64,
        downscale_ms: u64,
        save_ms: u64,
        db_ms: u64,
        total_ms: u64,
    ) {
        if let Ok(mut g) = self.saved_by_monitor.lock() {
            g.entry(monitor_id).or_default().push(
                capture_ms,
                dhash_ms,
                downscale_ms,
                save_ms,
                db_ms,
                total_ms,
            );
        }
        if let Ok(mut g) = self.monitor_names.lock() {
            g.entry(monitor_id)
                .or_insert_with(|| format!("Monitor {monitor_id}"));
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

        let mut by_monitor_guard = self.by_monitor.lock().ok();

        let mut monitors: Vec<MonitorCapturePerf> = by_monitor_guard
            .as_ref()
            .map(|g| {
                g.iter()
                    .map(|(&id, samples)| MonitorCapturePerf {
                        monitor_id: id,
                        monitor_name: names
                            .get(&id)
                            .cloned()
                            .unwrap_or_else(|| format!("Monitor {id}")),
                        stats: samples.summarize(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        monitors.sort_by_key(|m| m.monitor_id);

        let overall = by_monitor_guard
            .as_mut()
            .map(|g| aggregate_overall(g))
            .unwrap_or_default();
        drop(by_monitor_guard);

        let mut saved_guard = self.saved_by_monitor.lock().ok();
        let mut saved_monitors: Vec<MonitorCapturePerf> = Vec::new();
        for &id in names.keys() {
            let stats = saved_guard
                .as_ref()
                .and_then(|g| g.get(&id))
                .map(|s| s.summarize())
                .unwrap_or_default();
            saved_monitors.push(MonitorCapturePerf {
                monitor_id: id,
                monitor_name: names
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| format!("Monitor {id}")),
                stats,
            });
        }
        let saved_overall = saved_guard
            .as_mut()
            .map(|g| aggregate_overall(g))
            .unwrap_or_default();
        drop(saved_guard);
        drop(names);
        saved_monitors.sort_by_key(|m| m.monitor_id);

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

fn aggregate_overall(by_monitor: &mut HashMap<i32, PhaseSamples>) -> PerPhaseStats {
    let mut capture_samples: Vec<Vec<u64>> = Vec::new();
    let mut dhash_samples: Vec<Vec<u64>> = Vec::new();
    let mut downscale_samples: Vec<Vec<u64>> = Vec::new();
    let mut save_samples: Vec<Vec<u64>> = Vec::new();
    let mut db_samples: Vec<Vec<u64>> = Vec::new();
    let mut total_samples: Vec<Vec<u64>> = Vec::new();

    for samples in by_monitor.values_mut() {
        if !samples.capture.is_empty() {
            capture_samples.push(samples.capture.iter().copied().collect());
        }
        if !samples.dhash.is_empty() {
            dhash_samples.push(samples.dhash.iter().copied().collect());
        }
        if !samples.downscale.is_empty() {
            downscale_samples.push(samples.downscale.iter().copied().collect());
        }
        if !samples.save.is_empty() {
            save_samples.push(samples.save.iter().copied().collect());
        }
        if !samples.db.is_empty() {
            db_samples.push(samples.db.iter().copied().collect());
        }
        if !samples.total.is_empty() {
            total_samples.push(samples.total.iter().copied().collect());
        }
    }

    let total_count: usize = total_samples.iter().map(|s| s.len()).sum();

    PerPhaseStats {
        sample_count: total_count,
        capture: summarize_merged(&capture_samples),
        dhash: summarize_merged(&dhash_samples),
        downscale: summarize_merged(&downscale_samples),
        save: summarize_merged(&save_samples),
        db: summarize_merged(&db_samples),
        total: summarize_merged(&total_samples),
    }
}

fn push_capped(deque: &mut VecDeque<u64>, value: u64, cap: usize) {
    if deque.len() >= cap {
        deque.pop_front();
    }
    deque.push_back(value);
}
