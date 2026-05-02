import { useCallback, useEffect, useMemo, useState } from "react";
import { format } from "date-fns";
import { Copy, Expand, ExternalLink, FolderOpen, X } from "lucide-react";
import { api, type EmbeddingPreview, type Frame } from "../lib/api";
import { openFrameWindow } from "../lib/frameWindow";
import { staticHeldLabel } from "../lib/staticHeld";

const PAGE_SIZE = 120;

export default function Timeline() {
  const [frames, setFrames] = useState<Frame[]>([]);
  const [selected, setSelected] = useState<Frame | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(true);
  const [refreshSecs, setRefreshSecs] = useState(5);
  const [viewer, setViewer] = useState<Frame | null>(null);
  const [viewerFullscreen, setViewerFullscreen] = useState(false);
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    frame: Frame;
  } | null>(null);
  const [ocrText, setOcrText] = useState<string | null>(null);
  const [ocrLoading, setOcrLoading] = useState(false);
  const [embedPreview, setEmbedPreview] = useState<EmbeddingPreview | null>(null);
  const [embedLoading, setEmbedLoading] = useState(false);
  const [ocrFilter, setOcrFilter] = useState<"any" | "done" | "pending">("any");
  const [embedFilter, setEmbedFilter] = useState<
    "any" | "vector" | "na" | "pending"
  >("any");

  const refreshFrames = useCallback(async () => {
    try {
      const result = await api.listFrames({ limit: PAGE_SIZE });
      setFrames(result);
      setHasMore(result.length >= PAGE_SIZE);
      setSelected((prev) => {
        if (!prev) return result[0] ?? null;
        const still = result.find((f) => f.id === prev.id);
        return still ?? result[0] ?? null;
      });
    } catch {
      /* ignore */
    } finally {
      setLoading(false);
    }
  }, []);

  const loadOlder = useCallback(async () => {
    if (loadingMore || !hasMore || frames.length === 0) return;
    const oldestTs = frames[frames.length - 1]?.ts;
    if (!oldestTs) return;
    setLoadingMore(true);
    try {
      const older = await api.listFrames({ limit: PAGE_SIZE, beforeTs: oldestTs });
      if (older.length === 0) {
        setHasMore(false);
        return;
      }
      setFrames((prev) => {
        const seen = new Set(prev.map((f) => f.id));
        const add = older.filter((f) => !seen.has(f.id));
        return [...prev, ...add];
      });
      setHasMore(older.length >= PAGE_SIZE);
    } catch {
      // ignore
    } finally {
      setLoadingMore(false);
    }
  }, [frames, hasMore, loadingMore]);

  useEffect(() => {
    void refreshFrames();
  }, [refreshFrames]);

  useEffect(() => {
    const load = () => {
      api
        .getConfig()
        .then((c) => setRefreshSecs(c.timeline_refresh_secs))
        .catch(() => {});
    };
    load();
    window.addEventListener("focus", load);
    return () => window.removeEventListener("focus", load);
  }, []);

  useEffect(() => {
    if (refreshSecs === 0) return;
    const id = window.setInterval(() => void refreshFrames(), refreshSecs * 1000);
    return () => clearInterval(id);
  }, [refreshSecs, refreshFrames]);

  useEffect(() => {
    const closeMenu = () => setMenu(null);
    window.addEventListener("click", closeMenu);
    return () => window.removeEventListener("click", closeMenu);
  }, []);

  useEffect(() => {
    const onKey = (evt: KeyboardEvent) => {
      if (evt.key === "Escape") {
        setViewer(null);
        setViewerFullscreen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const filteredFrames = useMemo(
    () => frames.filter((f) => matchDebugFilters(f, ocrFilter, embedFilter)),
    [frames, ocrFilter, embedFilter],
  );

  useEffect(() => {
    setSelected((prev) => {
      if (!prev) return filteredFrames[0] ?? null;
      if (filteredFrames.some((f) => f.id === prev.id)) return prev;
      return filteredFrames[0] ?? null;
    });
  }, [filteredFrames]);

  useEffect(() => {
    if (!selected) {
      setOcrText(null);
      setEmbedPreview(null);
      return;
    }
    let cancelled = false;
    setOcrLoading(true);
    setEmbedLoading(true);
    setOcrText(null);
    setEmbedPreview(null);
    void Promise.all([
      api.getFrameOcr(selected.id),
      api.getFrameEmbeddingPreview(selected.id),
    ])
      .then(([t, emb]) => {
        if (cancelled) return;
        setOcrText(t);
        setEmbedPreview(emb);
      })
      .catch((e) => {
        if (!cancelled) {
          console.error("frame detail load failed", e);
          setOcrText(null);
          setEmbedPreview(null);
        }
      })
      .finally(() => {
        if (!cancelled) {
          setOcrLoading(false);
          setEmbedLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [selected?.id]);

  const grouped = groupByDay(filteredFrames);
  const selectedHeld = selected
    ? staticHeldLabel(selected.static_until_ms, selected.ts)
    : null;

  return (
    <div className="flex h-full flex-col">
      <header className="flex flex-col gap-2 border-b border-border px-4 py-2">
        <div className="flex min-h-8 items-center">
          <h1 className="text-sm font-medium">Timeline</h1>
          <span className="ml-auto text-xs text-text-faint">
            {filteredFrames.length} shown
            {frames.length !== filteredFrames.length
              ? ` of ${frames.length}`
              : ""}
            {refreshSecs > 0 ? ` · refresh ${refreshSecs}s` : " · manual refresh"}
          </span>
        </div>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[10px] text-text-faint">
          <span className="text-text-muted">Debug</span>
          <label className="inline-flex items-center gap-1">
            <span className="text-text-faint">OCR</span>
            <select
              value={ocrFilter}
              onChange={(e) => setOcrFilter(e.target.value as typeof ocrFilter)}
              className="max-w-[7rem] rounded border border-border bg-bg px-1 py-0.5 text-[10px] text-text"
            >
              <option value="any">any</option>
              <option value="done">done</option>
              <option value="pending">pending</option>
            </select>
          </label>
          <label className="inline-flex items-center gap-1">
            <span className="text-text-faint">Embed</span>
            <select
              value={embedFilter}
              onChange={(e) =>
                setEmbedFilter(e.target.value as typeof embedFilter)
              }
              className="max-w-[9rem] rounded border border-border bg-bg px-1 py-0.5 text-[10px] text-text"
            >
              <option value="any">any</option>
              <option value="vector">has vector</option>
              <option value="na">not needed (no text)</option>
              <option value="pending">pending</option>
            </select>
          </label>
          <button
            type="button"
            onClick={() => void loadOlder()}
            disabled={!hasMore || loadingMore}
            className="ml-auto rounded border border-border px-2 py-0.5 text-[10px] text-text-muted hover:bg-bg-hover disabled:opacity-50"
          >
            {loadingMore ? "Loading older…" : hasMore ? "Load older" : "No older frames"}
          </button>
        </div>
      </header>

      {loading ? (
        <Empty message="Loading..." />
      ) : frames.length === 0 ? (
        <Empty message="Nothing captured yet. ScreenRecall will start indexing in the background." />
      ) : filteredFrames.length === 0 ? (
        <Empty message="No frames match the debug filters. Clear OCR / Embed filters to see all." />
      ) : (
        <div className="flex flex-1 overflow-hidden">
          <div className="flex-1 overflow-y-auto scrollbar-thin p-4 space-y-6">
            {Object.entries(grouped).map(([day, dayFrames]) => (
              <section key={day} className="space-y-2">
                <h2 className="text-xs font-semibold uppercase tracking-wider text-text-muted">
                  {day}
                </h2>
                <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
                  {dayFrames.map((f) => {
                    const held = staticHeldLabel(f.static_until_ms, f.ts);
                    return (
                    <button
                      key={f.id}
                      onClick={() => setSelected(f)}
                      onContextMenu={(e) => {
                        e.preventDefault();
                        setMenu({ x: e.clientX, y: e.clientY, frame: f });
                      }}
                      className={
                        "group overflow-hidden rounded-lg border bg-bg-elevated text-left transition-all " +
                        (selected?.id === f.id
                          ? "border-accent ring-1 ring-accent"
                          : "border-border hover:border-border-strong")
                      }
                    >
                      <div className="aspect-video bg-black/40 overflow-hidden">
                        <img
                          src={api.assetUrl(f.path)}
                          alt=""
                          loading="lazy"
                          decoding="async"
                          className="h-full w-full object-cover opacity-90 group-hover:opacity-100"
                        />
                      </div>
                      <div className="px-3 py-2">
                        <div className="truncate text-xs text-text">
                          {f.window_title ?? f.app ?? "—"}
                        </div>
                        <div className="text-[10px] text-text-faint">
                          #{f.id} ·{" "}
                          {format(f.ts, "HH:mm:ss")}
                          {held && (
                            <span className="ml-1 text-text-muted">· {held}</span>
                          )}
                        </div>
                      </div>
                    </button>
                    );
                  })}
                </div>
              </section>
            ))}
          </div>

          {selected && (
            <aside className="w-96 shrink-0 border-l border-border bg-bg-elevated overflow-y-auto scrollbar-thin">
              <button
                type="button"
                onClick={() => setViewer(selected)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setMenu({ x: e.clientX, y: e.clientY, frame: selected });
                }}
                className="relative block w-full border-b border-border"
                title="Open large preview"
              >
                <img
                  src={api.assetUrl(selected.path)}
                  alt=""
                  decoding="async"
                  className="w-full"
                />
                <span className="absolute right-2 top-2 rounded border border-border bg-bg/80 p-1 text-text-muted">
                  <Expand className="h-3.5 w-3.5" />
                </span>
              </button>
              <div className="p-4 space-y-3">
                <div>
                  <div className="text-xs text-text-muted">Frame ID</div>
                  <div className="text-sm font-mono">#{selected.id}</div>
                </div>
                <div>
                  <div className="text-xs text-text-muted">Time</div>
                  <div className="text-sm">
                    {format(selected.ts, "PPpp")}
                    {selectedHeld && (
                      <div className="mt-0.5 text-xs text-text-muted">
                        {selectedHeld} (end{" "}
                        {format(selected.static_until_ms ?? selected.ts, "PPpp")})
                      </div>
                    )}
                  </div>
                </div>
                <div>
                  <div className="text-xs text-text-muted">App</div>
                  <div className="text-sm">{selected.app ?? "—"}</div>
                </div>
                <div>
                  <div className="text-xs text-text-muted">Window</div>
                  <div className="text-sm break-words">
                    {selected.window_title ?? "—"}
                  </div>
                </div>
                <div>
                  <div className="flex items-center justify-between gap-2">
                    <div className="text-xs text-text-muted">File</div>
                    <button
                      type="button"
                      className="inline-flex items-center gap-1 rounded border border-border px-1.5 py-0.5 text-[10px] text-text-muted hover:bg-bg-hover"
                      title="Open in File Explorer"
                      onClick={() => {
                        void api.revealFrameInFolder(selected.path).catch(() => {});
                      }}
                    >
                      <FolderOpen className="h-3 w-3" />
                      Open location
                    </button>
                  </div>
                  <div className="mt-0.5 text-[11px] font-medium text-text break-all">
                    {pathBasename(selected.path)}
                  </div>
                  <div className="mt-0.5 break-all font-mono text-[10px] leading-snug text-text-faint">
                    {selected.path}
                  </div>
                </div>
                <div className="flex flex-wrap gap-2 text-[10px] text-text-faint">
                  <span
                    className={
                      "rounded px-1.5 py-0.5 " +
                      (selected.ocr_done
                        ? "bg-emerald-500/10 text-emerald-300"
                        : "bg-bg-hover")
                    }
                  >
                    OCR {selected.ocr_done ? "✓" : "…"}
                  </span>
                  <EmbedPill f={selected} />
                </div>
                <div>
                  <div className="flex items-center justify-between gap-2">
                    <div className="text-xs text-text-muted">OCR text</div>
                    {ocrText != null && ocrText.length > 0 && (
                      <button
                        type="button"
                        className="inline-flex items-center gap-1 rounded border border-border px-1.5 py-0.5 text-[10px] text-text-muted hover:bg-bg-hover"
                        onClick={() => {
                          void navigator.clipboard.writeText(ocrText);
                        }}
                        title="Copy"
                      >
                        <Copy className="h-3 w-3" />
                        Copy
                      </button>
                    )}
                  </div>
                  {ocrLoading ? (
                    <div className="mt-1 text-xs text-text-faint">Loading…</div>
                  ) : ocrText == null || ocrText === "" ? (
                    <div className="mt-1 text-xs text-text-faint">
                      {selected.ocr_done
                        ? "No text extracted."
                        : "OCR not finished yet."}
                    </div>
                  ) : (
                    <pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap break-words rounded border border-border bg-bg px-2 py-1.5 text-[11px] leading-relaxed text-text-muted scrollbar-thin">
                      {ocrText}
                    </pre>
                  )}
                </div>
                <div>
                  <div className="flex items-center justify-between gap-2">
                    <div className="text-xs text-text-muted">Embedding vector (preview)</div>
                    {embedPreview && (
                      <button
                        type="button"
                        className="inline-flex items-center gap-1 rounded border border-border px-1.5 py-0.5 text-[10px] text-text-muted hover:bg-bg-hover"
                        onClick={() => {
                          const payload = JSON.stringify(embedPreview);
                          void navigator.clipboard.writeText(payload);
                        }}
                        title="Copy JSON"
                      >
                        <Copy className="h-3 w-3" />
                        Copy
                      </button>
                    )}
                  </div>
                  {embedLoading ? (
                    <div className="mt-1 text-xs text-text-faint">Loading…</div>
                  ) : !selected.has_embedding || !embedPreview ? (
                    <div className="mt-1 text-xs text-text-faint">
                      {selected.embed_done
                        ? "No vector stored for this frame."
                        : "Embedding not finished yet."}
                    </div>
                  ) : (
                    <>
                      <div className="mt-1 text-[11px] text-text-faint">
                        dim {embedPreview.dim} · showing first {embedPreview.values.length}
                      </div>
                      <pre className="mt-1 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded border border-border bg-bg px-2 py-1.5 font-mono text-[10px] leading-relaxed text-text-muted scrollbar-thin">
                        [{embedPreview.values.map((v) => Number(v).toFixed(6)).join(", ")}]
                      </pre>
                    </>
                  )}
                </div>
              </div>
            </aside>
          )}
        </div>
      )}

      {viewer && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-4"
          onClick={() => {
            setViewer(null);
            setViewerFullscreen(false);
          }}
        >
          <div
            className={
              "relative overflow-hidden rounded-lg border border-border bg-bg-elevated " +
              (viewerFullscreen ? "h-[95vh] w-[95vw]" : "h-[80vh] w-[80vw] max-w-6xl")
            }
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center gap-2 border-b border-border px-3 py-2 text-xs text-text-muted">
              <span className="shrink-0 font-mono">#{viewer.id}</span>
              <span className="truncate">{viewer.window_title ?? viewer.app ?? viewer.path}</span>
              <button
                type="button"
                onClick={() => setViewerFullscreen((v) => !v)}
                className="ml-auto rounded border border-border p-1 hover:bg-bg-hover"
                title={viewerFullscreen ? "Exit fullscreen" : "Fullscreen"}
              >
                <Expand className="h-3.5 w-3.5" />
              </button>
              <button
                type="button"
                onClick={() => {
                  void api.revealFrameInFolder(viewer.path).catch(() => {});
                }}
                className="rounded border border-border p-1 hover:bg-bg-hover"
                title="Open file location"
              >
                <FolderOpen className="h-3.5 w-3.5" />
              </button>
              <button
                type="button"
                onClick={() => {
                  openFrameWindow(viewer);
                }}
                className="rounded border border-border p-1 hover:bg-bg-hover"
                title="Open in new window"
              >
                <ExternalLink className="h-3.5 w-3.5" />
              </button>
              <button
                type="button"
                onClick={() => {
                  setViewer(null);
                  setViewerFullscreen(false);
                }}
                className="rounded border border-border p-1 hover:bg-bg-hover"
                title="Close"
              >
                <X className="h-3.5 w-3.5" />
              </button>
            </div>
            <div className="h-[calc(100%-2.25rem)] w-full bg-black">
              <img
                src={api.assetUrl(viewer.path)}
                alt={viewer.window_title ?? viewer.app ?? "Captured frame"}
                decoding="async"
                className="h-full w-full object-contain"
                onContextMenu={(e) => {
                  e.preventDefault();
                  setMenu({ x: e.clientX, y: e.clientY, frame: viewer });
                }}
              />
            </div>
          </div>
        </div>
      )}
      {menu && (
        <div
          className="fixed z-[60] min-w-44 rounded-md border border-border bg-bg-elevated p-1 shadow-lg"
          style={{ left: menu.x, top: menu.y }}
          onClick={(e) => e.stopPropagation()}
          role="menu"
        >
          <button
            type="button"
            onClick={() => {
              openFrameWindow(menu.frame);
              setMenu(null);
            }}
            className="flex w-full items-center gap-2 rounded px-2 py-1 text-xs hover:bg-bg-hover"
          >
            <ExternalLink className="h-3.5 w-3.5" />
            Open in new window
          </button>
          <button
            type="button"
            onClick={() => {
              void api.revealFrameInFolder(menu.frame.path).catch(() => {});
              setMenu(null);
            }}
            className="flex w-full items-center gap-2 rounded px-2 py-1 text-xs hover:bg-bg-hover"
          >
            <FolderOpen className="h-3.5 w-3.5" />
            Open file location
          </button>
        </div>
      )}
    </div>
  );
}

function pathBasename(p: string): string {
  const s = p.replace(/\\/g, "/");
  const i = s.lastIndexOf("/");
  return i >= 0 ? s.slice(i + 1) : s;
}

function matchDebugFilters(
  f: Frame,
  ocr: "any" | "done" | "pending",
  emb: "any" | "vector" | "na" | "pending",
): boolean {
  if (ocr === "done" && !f.ocr_done) return false;
  if (ocr === "pending" && f.ocr_done) return false;
  if (emb === "vector" && !f.has_embedding) return false;
  if (emb === "na" && !(f.embed_done && !f.has_embedding)) return false;
  if (emb === "pending" && !(f.ocr_done && !f.embed_done)) return false;
  return true;
}

function EmbedPill({ f }: { f: Frame }) {
  if (!f.ocr_done) {
    return (
      <span
        className="rounded px-1.5 py-0.5 bg-bg-hover"
        title="OCR not finished; embedding not started"
      >
        Embed …
      </span>
    );
  }
  if (f.has_embedding) {
    return (
      <span className="rounded px-1.5 py-0.5 bg-emerald-500/10 text-emerald-300" title="Indexed for search">
        Embed ✓
      </span>
    );
  }
  if (f.embed_done) {
    return (
      <span
        className="rounded px-1.5 py-0.5 bg-emerald-500/10 text-emerald-300"
        title="No text to embed (empty OCR)"
      >
        Embed — not needed
      </span>
    );
  }
  return (
    <span
      className="rounded px-1.5 py-0.5 bg-bg-hover"
      title="OCR has text; embedding in progress or queued"
    >
      Embed …
    </span>
  );
}

function groupByDay(frames: Frame[]): Record<string, Frame[]> {
  const out: Record<string, Frame[]> = {};
  for (const f of frames) {
    const key = format(f.ts, "EEEE, MMM d");
    (out[key] ??= []).push(f);
  }
  return out;
}

function Empty({ message }: { message: string }) {
  return (
    <div className="flex flex-1 items-center justify-center text-sm text-text-muted">
      {message}
    </div>
  );
}
