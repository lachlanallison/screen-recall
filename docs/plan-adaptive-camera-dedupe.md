# Plan: Adaptive Deduplication for Noisy Camera Feeds (Experimental)

## Problem

The global dHash deduplication slider (`capture_dedupe_threshold`) is a compromise:
- **Low (default 3):** perfect for normal screens — tiny changes are skipped, real changes are saved.
- **High (6–10):** needed for noisy sources like CCTV/security camera feeds shown on a monitor, where compression/ sensor noise flips dHash bits on every capture.

Raising the global threshold fixes cameras but causes us to miss legitimate small changes on normal screens (e.g., a new sentence typed, a notification popping up).

## Goal

Add an **experimental adaptive-dedupe mode** that:
1. Automatically detects which monitors are showing noisy static content (CCTV feeds, HDMI artefacts, etc.).
2. Applies a higher dedupe threshold **only** to those monitors.
3. Leaves normal monitors on the user-defined global threshold.
4. Can be enabled/disabled in Settings with a single toggle.

## Detection Heuristic

Noisy camera feeds have a very different dHash signature from normal screen usage:

| Source | Static frames | Saved-frame distances | Pattern |
|--------|--------------|----------------------|---------|
| Normal screen | skipped (dist 0–2) | sparse, mostly 10+ when user acts | low save rate, high variance |
| Noisy camera | **saved** (dist 3–10) | dense, consistently 3–10 | high save rate, bounded variance |
| Video playing | saved (dist 10–16+) | dense, often > 13 | high save rate, high distances |

**Algorithm** (per monitor, tick-based):

- Maintain a ring buffer of the last **20 capture ticks**.
  - `Some(distance)` if the frame was **saved**.
  - `None` if the frame was **skipped** (deduped).
- A monitor is classified **noisy** when:
  1. Buffer has ≥ 15 entries (ticks).
  2. At least 15 of those entries are `Some(...)` (very high save rate).
  3. **Every** saved distance is `< 13` (never a real scene change).
- The noisy flag is cleared immediately when:
  - Any saved frame has distance `≥ 13` (a real change happened), **or**
  - The save rate drops below the threshold (monitor went quiet or stopped being noisy).

Why this works:
- A camera feed produces medium distances (3–10) on *every* tick. Within ~60s (20 ticks @ 3s interval) it hits 15 saved frames, all < 13 → flagged.
- A normal user typing/scrolling saves frames sporadically; it won't hit 15 saves in 20 ticks.
- A playing video usually produces distances > 13 fairly often, so condition 3 fails.
- When the camera app is closed, the desktop background appears — distance spikes to ≥ 13, flag clears instantly.

*The 20-tick / 15-save / 13-bound constants are chosen to be conservative. As an experimental feature they can be hardcoded; we can expose them later if users need tuning.*

## Config Additions

Three new fields in `AppConfig` (all with serde defaults so old configs migrate automatically):

```rust
/// When true, auto-detect noisy monitors and use a higher dedupe threshold for them.
#[serde(default)]
pub capture_adaptive_dedupe_enabled: bool,

/// dHash threshold applied to monitors that are classified as noisy.
/// Must be higher than `capture_dedupe_threshold` to have any effect.
#[serde(default = "default_capture_noisy_monitor_threshold")]
pub capture_noisy_monitor_threshold: u32,

/// Safety buffer: frames newer than this many seconds are never archived.
/// Default 60s protects retroactive adaptive-dedupe merges from colliding
/// with the archiver.
#[serde(default = "default_archive_delay_secs")]
pub archive_delay_secs: u64,
```

Defaults:
- `capture_adaptive_dedupe_enabled`: `false`
- `capture_noisy_monitor_threshold`: `8`
- `archive_delay_secs`: `60`

Frontend type updates in `api.ts` to match.

## Rust Capture Module Changes (`capture/mod.rs`)

### New state struct
```rust
#[derive(Clone, Debug, Default)]
struct AdaptiveState {
    /// Last 20 ticks: Some(distance) if saved, None if skipped.
    ticks: VecDeque<Option<u32>>,
    is_noisy: bool,
}

impl AdaptiveState {
    fn push(&mut self, saved: bool, distance: u32) { ... }
    fn evaluate(&mut self) { ... }
}
```

### Scheduler loop
In `run_scheduler`, add alongside `fingerprint_cache`:
```rust
let mut adaptive_state: HashMap<i32, AdaptiveState> = HashMap::new();
```

Pass it into `capture_once` by clone (same pattern as `fingerprint_cache`).

### Per-monitor capture thread
1. Compute `distance = hamming(prev, phash)`.
2. Clone the monitor's `AdaptiveState` from the snapshot.
3. Determine **effective threshold**:
   ```rust
   let effective = if adaptive_enabled && dedupe_threshold > 0 && local_adaptive.is_noisy {
       noisy_threshold
   } else {
       dedupe_threshold
   };
   ```
4. Run dedupe check against `effective` instead of `dedupe_threshold`.
5. **Before returning**, push the tick into local adaptive state and evaluate:
   ```rust
   local_adaptive.push(saved, distance);
   local_adaptive.evaluate();
   ```
6. Return the updated `AdaptiveState` so the scheduler can merge it back.

### Merge back & retroactive cleanup
After `spawn_blocking` resolves:
```rust
for (monitor_id, local_adaptive) in result.adaptive_updates {
    let old_noisy = adaptive_state.get(&monitor_id).map(|s| s.is_noisy).unwrap_or(false);
    let new_noisy = local_adaptive.is_noisy;
    if !old_noisy && new_noisy {
        // Run retroactive cleanup on this monitor
        retroactive_cleanup(state, monitor_id, noisy_threshold, archive_delay_ms)?;
    }
    adaptive_state.insert(monitor_id, local_adaptive);
}
```

### Retroactive cleanup (`retroactive_cleanup`)
When a monitor is newly classified as noisy, frames captured before the flag was set were saved using the **default** (low) threshold. Many of them would have been skipped with the noisy threshold.

The cleanup:
1. Queries the last 120 unarchived, fully-indexed frames for that monitor (regardless of age — the archiver delay protects them, not the cleanup).
2. Groups consecutive frames where `hamming(prev, next) ≤ noisy_threshold`.
3. For each group with >1 frame:
   - **Keeper** = earliest frame.
   - Extends keeper's `static_until_ms` to the group's last timestamp.
   - Deletes redundant frames' DB rows (`embeddings`, `ocr_text`, `frames`).
   - Deletes their frame files from disk.

This compacts the noisy run into a single representative frame per visually-static block.

> **Why no age filter in cleanup?** The archiver delay already prevents the archiver from touching recent frames. Filtering cleanup by the same delay would mean frames from the detection window (which just finished) are "too recent" to clean up, defeating the purpose. The archiver delay is the single point of coordination.

## Archive Delay (`archive/mod.rs`)

To prevent the archiver from touching frames that the adaptive cleanup may still want to merge, we introduce an **archive delay**:

```rust
let delay_ms = cfg.archive_delay_secs as i64 * 1000;
let cutoff = now - delay_ms;
```

Only frames with `ts < cutoff` are eligible for archival. The archiver's bookmark is set to `cutoff`, not `now`, so the next run starts exactly where this one left off, creating contiguous, non-overlapping buckets.

With the default 60s delay and default 3s capture interval:
- Adaptive detection needs up to 60s (20 ticks × 3s).
- The archiver never touches frames from the last 60s.
- By the time the archiver sees them, adaptive cleanup has already merged the noisy burst.
- Zero locking or coordination between capture and archiver is required.

**Safety invariant:** `archive_delay_secs` should be at least `capture_interval_secs × 20`. If it is shorter, the Settings UI shows a warning because the archiver may grab frames before adaptive cleanup can merge them.

## Frontend Changes (`Settings.tsx`)

### New "Experimental" section
1. **Toggle**: "Adaptive deduplication for noisy camera feeds"
   - Checkbox bound to `capture_adaptive_dedupe_enabled`
   - Help text explaining auto-detection of CCTV/security-camera style noise.

2. **Slider** (visible only when toggle is on): "Noisy monitor threshold (1–16)"
   - Bound to `capture_noisy_monitor_threshold`
   - Help text: "Applied to monitors automatically detected as noisy. Must be higher than the default deduplication threshold above."

### Video archival section
3. **Field**: "Archive delay (seconds)"
   - Bound to `archive_delay_secs`
   - Help text explains the 60s default protects retroactive adaptive merges from colliding with the archiver.
   - **Warning banner** appears in the Experimental section when adaptive is enabled and `archive_delay_secs < capture_interval_secs * 20`. This tells the user the archiver may grab frames before adaptive has a chance to merge them.

## Logging & Observability

- `info!` when a monitor transitions into or out of noisy state.
- `info!` when retroactive cleanup merges frames (with count).
- The existing per-monitor capture perf stats already record `monitor_id` and `monitor_name`; noisy classification doesn't change the perf pipeline.

## Testing Plan

1. **Unit test** `AdaptiveState::evaluate` with synthetic tick sequences:
   - 15 saves all with distance 5 → should flag noisy.
   - 10 saves with distance 5, 10 skips → should NOT flag (save rate too low).
   - 15 saves with one distance 14 → should NOT flag (real change occurred).
   - After flagging, one tick with saved distance 14 → should clear flag.

2. **Manual test**: Open a browser tab with a local IP camera or YouTube "static noise" video on a secondary monitor. Enable adaptive mode. Verify:
   - Dedupe kicks in on the camera monitor after ~60s.
   - The primary monitor still saves small UI changes at the default threshold.
   - Closing the camera tab causes the flag to clear on the next capture tick.
   - Archiver never archives frames younger than the delay setting.

3. **Regression**: With adaptive mode **disabled**, behavior is byte-for-byte identical to before (the `effective` threshold always equals `dedupe_threshold`).

## Migration & Backwards Compatibility

- Old configs missing the new keys will deserialize with defaults (`false`, `8`, `60`) thanks to `#[serde(default)]`.
- No DB schema changes required.
- No changes to the existing dHash or Hamming implementations.

## Future Extensions (out of scope for v1)

- Expose the 20-tick window size / 13-bound as advanced config knobs.
- Use monitor **names** instead of IDs for persistence (so flags survive monitor reconnects).
- Allow per-monitor manual override lists (e.g., always treat "Monitor 2" as noisy).
