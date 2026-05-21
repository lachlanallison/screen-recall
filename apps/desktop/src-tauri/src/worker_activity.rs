//! Best-effort view of which frame each indexer is processing (complements DB pending counts).
//! Tokio mpsc does not expose channel depth, so "current frame" + DB backlogs is the UI model.
//!
//! `ocr_durations` / `embed_durations` keep the last N completed per-frame timings (ms) for the
//! Diagnostics "recent samples" line.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;

const TIMING_WINDOW: usize = 400;

pub struct WorkerActivity {
    ocr: Mutex<Option<(i64, Instant)>>,
    embed: Mutex<Option<(i64, Instant)>>,
    ocr_durations: Mutex<VecDeque<u64>>,
    embed_durations: Mutex<VecDeque<u64>>,
}

impl Default for WorkerActivity {
    fn default() -> Self {
        Self {
            ocr: Mutex::new(None),
            embed: Mutex::new(None),
            ocr_durations: Mutex::new(VecDeque::new()),
            embed_durations: Mutex::new(VecDeque::new()),
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OneWorkerQueue {
    /// Frames in DB waiting for this stage (`ocr_done = 0` or embed pending, etc.).
    pub pending_in_db: i64,
    /// Frame id being processed, if any.
    pub active_frame_id: Option<i64>,
    /// How long the current job has been running (ms), if active.
    pub active_elapsed_ms: Option<u64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorkerQueuesSnapshot {
    pub ocr: OneWorkerQueue,
    pub embed: OneWorkerQueue,
    pub ocr_timing: StageTimingStats,
    pub embed_timing: StageTimingStats,
    pub frame_total: i64,
}

/// Rolling per-frame completion times (ms), from the most recent N finished jobs in-process.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct StageTimingStats {
    pub sample_count: usize,
    pub min_ms: Option<u64>,
    pub max_ms: Option<u64>,
    /// Arithmetic mean, milliseconds.
    pub mean_ms: Option<f64>,
    pub p50_ms: Option<u64>,
    pub p95_ms: Option<u64>,
}

pub fn summarize_ms(samples: &[u64]) -> StageTimingStats {
    if samples.is_empty() {
        return StageTimingStats::default();
    }
    let mut s = samples.to_vec();
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

impl WorkerActivity {
    pub fn set_ocr_active(&self, id: Option<i64>) {
        if let Ok(mut g) = self.ocr.lock() {
            *g = id.map(|i| (i, Instant::now()));
        }
    }

    pub fn set_embed_active(&self, id: Option<i64>) {
        if let Ok(mut g) = self.embed.lock() {
            *g = id.map(|i| (i, Instant::now()));
        }
    }

    pub fn snapshot_ocr(&self) -> (Option<i64>, Option<u64>) {
        let Ok(g) = self.ocr.lock() else {
            return (None, None);
        };
        match g.as_ref() {
            None => (None, None),
            Some((id, t)) => (Some(*id), Some(t.elapsed().as_millis() as u64)),
        }
    }

    pub fn snapshot_embed(&self) -> (Option<i64>, Option<u64>) {
        let Ok(g) = self.embed.lock() else {
            return (None, None);
        };
        match g.as_ref() {
            None => (None, None),
            Some((id, t)) => (Some(*id), Some(t.elapsed().as_millis() as u64)),
        }
    }

    pub fn record_ocr_ms(&self, ms: u64) {
        if let Ok(mut g) = self.ocr_durations.lock() {
            if g.len() >= TIMING_WINDOW {
                g.pop_front();
            }
            g.push_back(ms);
        }
    }

    pub fn record_embed_ms(&self, ms: u64) {
        if let Ok(mut g) = self.embed_durations.lock() {
            if g.len() >= TIMING_WINDOW {
                g.pop_front();
            }
            g.push_back(ms);
        }
    }

    pub fn timing_stats(&self) -> (StageTimingStats, StageTimingStats) {
        let ocr = self
            .ocr_durations
            .lock()
            .ok()
            .map(|g| g.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        let emb = self
            .embed_durations
            .lock()
            .ok()
            .map(|g| g.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        (summarize_ms(&ocr), summarize_ms(&emb))
    }
}

/// Clears `active_ocr` when dropped.
pub struct OcrActiveGuard {
    activity: Arc<WorkerActivity>,
}

impl OcrActiveGuard {
    pub fn new(activity: &Arc<WorkerActivity>, id: i64) -> Self {
        activity.set_ocr_active(Some(id));
        Self {
            activity: activity.clone(),
        }
    }
}

impl Drop for OcrActiveGuard {
    fn drop(&mut self) {
        self.activity.set_ocr_active(None);
    }
}

/// Clears `active_embed` when dropped.
pub struct EmbedActiveGuard {
    activity: Arc<WorkerActivity>,
}

impl EmbedActiveGuard {
    pub fn new(activity: &Arc<WorkerActivity>, id: i64) -> Self {
        activity.set_embed_active(Some(id));
        Self {
            activity: activity.clone(),
        }
    }
}

impl Drop for EmbedActiveGuard {
    fn drop(&mut self) {
        self.activity.set_embed_active(None);
    }
}
