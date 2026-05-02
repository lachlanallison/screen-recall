/**
 * Human-readable “unchanged for …” for timeline/search when dedupe extended the row.
 * Returns null if the difference is under 1s (noise / single-tick).
 */
export function staticHeldLabel(staticUntilMs: number | undefined, ts: number): string | null {
  const end = staticUntilMs ?? ts;
  const ms = end - ts;
  if (ms < 1000) return null;
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s unchanged`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  if (m < 60) return r ? `${m}m ${r}s unchanged` : `${m}m unchanged`;
  const h = Math.floor(m / 60);
  const rem = m % 60;
  return rem ? `${h}h ${rem}m unchanged` : `${h}h unchanged`;
}
