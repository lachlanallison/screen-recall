import { useCallback, useEffect, useMemo, useState } from "react";
import { format } from "date-fns";
import { Copy, Expand, ExternalLink, FolderOpen, Film, Image as ImageIcon } from "lucide-react";
import { api, type EmbeddingPreview, type Frame, type Video } from "../lib/api";
import { FrameViewer } from "../lib/components/ImageViewer";
import { ContextMenu } from "../lib/components/ContextMenu";
import { useEscape } from "../lib/components/useEscape";
import { openFrameWindow } from "../lib/frameWindow";
import { staticHeldLabel } from "../lib/staticHeld";

const PAGE_SIZE = 120;

/** A group of frames that share the same video_path (archived segment) or a single still frame. */
type TimelineItem =
  | { type: "video"; videoPath: string; frames: Frame[] }
  | { type: "still"; frame: Frame };

function buildTimelineItems(frames: Frame[]): TimelineItem[] {
  const videoGroups = new Map<string, Frame[]>();
  const stills: Frame[] = [];

  for (const f of frames) {
    if (f.video_path) {
      const group = videoGroups.get(f.video_path);
      if (group) {
        group.push(f);
      } else {
        videoGroups.set(f.video_path, [f]);
      }
    } else {
      stills.push(f);
    }
  }

  const items: TimelineItem[] = [];
  for (const [videoPath, groupFrames] of videoGroups) {
    groupFrames.sort((a, b) => a.ts - b.ts);
    items.push({ type: "video", videoPath, frames: groupFrames });
  }
  for (const f of stills) {
    items.push({ type: "still", frame: f });
  }

  items.sort((a, b) => {
    const aTs = a.type === "video" ? a.frames[a.frames.length - 1].ts : a.frame.ts;
    const bTs = b.type === "video" ? b.frames[b.frames.length - 1].ts : b.frame.ts;
    return bTs - aTs;
  });
  return items;
}

export default function Timeline() {
  const [frames, setFrames] = useState<Frame[]>([]);
  const [selected, setSelected] = useState<Frame | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(true);
  const [refreshSecs, setRefreshSecs] = useState(5);
  const [viewer, setViewer] = useState<Frame | null>(null);
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
  const [viewMode, setViewMode] = useState<"latest" | "archived">("latest");
  const [monitorFilter, setMonitorFilter] = useState<string>("all");

  // State for video frame extraction
  const [frozenFrameUrl, setFrozenFrameUrl] = useState<string | null>(null);
  const [activeFrame, setActiveFrame] = useState<Frame | null>(null);

  const [videos, setVideos] = useState<Video[]>([]);
  const [loadingVideos, setLoadingVideos] = useState(false);
  const [loadingMoreVideos, setLoadingMoreVideos] = useState(false);
  const [hasMoreVideos, setHasMoreVideos] = useState(true);

  const refreshFrames = useCallback(async () => {
    try {
      const result = await api.listFrames({ limit: PAGE_SIZE });
      setFrames((prev) => {
        const idToResult = new Map(result.map((f) => [f.id, f]));
        const seen = new Set(prev.map((f) => f.id));
        const add = result.filter((f) => !seen.has(f.id));

        if (add.length === 0 && idToResult.size === 0) return prev;

        // Update static_until_ms for existing frames that appear in the refresh result
        const updated = prev.map((f) => {
          const refreshed = idToResult.get(f.id);
          if (refreshed && refreshed.static_until_ms !== f.static_until_ms) {
            return { ...f, static_until_ms: refreshed.static_until_ms };
          }
          return f;
        });

        return [...add, ...updated];
      });
    } catch {
      /* ignore */
    } finally {
      setLoading(false);
    }
  }, []);

  const refreshVideos = useCallback(async () => {
    try {
      const result = await api.listVideos({ limit: PAGE_SIZE });
      setVideos(result);
      setHasMoreVideos(result.length >= PAGE_SIZE);
    } catch {
      /* ignore */
    } finally {
      setLoadingVideos(false);
    }
  }, []);

  const loadOlderVideos = useCallback(async () => {
    if (loadingMoreVideos || !hasMoreVideos || videos.length === 0) return;
    const oldest = videos[videos.length - 1]?.created_at;
    if (!oldest) return;
    setLoadingMoreVideos(true);
    try {
      const older = await api.listVideos({ limit: PAGE_SIZE, beforeTs: oldest });
      if (older.length === 0) {
        setHasMoreVideos(false);
        return;
      }
      setVideos((prev) => {
        const seen = new Set(prev.map((v) => v.id));
        const add = older.filter((v) => !seen.has(v.id));
        return [...prev, ...add];
      });
      setHasMoreVideos(older.length >= PAGE_SIZE);
    } catch {
      // ignore
    } finally {
      setLoadingMoreVideos(false);
    }
  }, [videos, hasMoreVideos, loadingMoreVideos]);

  const monitors = useMemo(
    () => {
      const seen = new Map<number, string>();
      for (const f of frames) {
        if (!seen.has(f.monitor_id)) {
          seen.set(f.monitor_id, f.monitor_name ?? `Monitor ${f.monitor_id}`);
        }
      }
      for (const v of videos) {
        if (!seen.has(v.monitor_id)) {
          seen.set(v.monitor_id, v.monitor_name ?? `Monitor ${v.monitor_id}`);
        }
      }
      return [...seen.entries()].sort((a, b) => a[1].localeCompare(b[1]));
    },
    [frames, videos],
  );

  const filterByMonitor = (f: Frame): boolean =>
    monitorFilter === "all" || String(f.monitor_id) === monitorFilter;

  const filterVideoByMonitor = (v: Video): boolean =>
    monitorFilter === "all" || String(v.monitor_id) === monitorFilter;

  // Separate filtered views derived from the same frames array — instant switching.
  const latestFrames = useMemo(
    () => frames.filter((f) => matchDebugFilters(f, ocrFilter, embedFilter) && !f.video_path && filterByMonitor(f)),
    [frames, ocrFilter, embedFilter, monitorFilter],
  );
  const filteredVideos = useMemo(
    () => videos.filter(filterVideoByMonitor),
    [videos, monitorFilter],
  );

  useEffect(() => {
    setSelected((prev) => {
      if (!prev) return latestFrames[0] ?? null;
      if (latestFrames.some((f) => f.id === prev.id)) return prev;
      return latestFrames[0] ?? null;
    });
  }, [latestFrames]);

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
    if (viewMode === "archived") {
      setLoadingVideos(true);
      void refreshVideos();
    }
  }, [viewMode, refreshVideos]);

  useEffect(() => {
    if (refreshSecs === 0 || viewMode !== "archived") return;
    const id = window.setInterval(() => void refreshVideos(), refreshSecs * 1000);
    return () => clearInterval(id);
  }, [refreshSecs, viewMode, refreshVideos]);

  const closeMenu = useCallback(() => setMenu(null), []);
  useEscape(() => { setViewer(null); setMenu(null); });

  useEffect(() => {
    window.addEventListener("click", closeMenu);
    return () => window.removeEventListener("click", closeMenu);
  }, [closeMenu]);

  useEffect(() => {
    setSelected((prev) => {
      if (!prev) return latestFrames[0] ?? null;
      if (latestFrames.some((f) => f.id === prev.id)) return prev;
      return latestFrames[0] ?? null;
    });
  }, [latestFrames]);

  useEffect(() => {
    if (!selected) {
      setOcrText(null);
      setEmbedPreview(null);
      setActiveFrame(null);
      setFrozenFrameUrl(null);
      return;
    }
    // Reset freeze state when selection changes
    if (activeFrame?.id !== selected.id && !frozenFrameUrl) {
      setActiveFrame(selected);
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

  // Load details for active frame (when frozen or scrubbed)
  useEffect(() => {
    if (!activeFrame || activeFrame.id === selected?.id) return;
    let cancelled = false;
    setOcrLoading(true);
    setEmbedLoading(true);
    setOcrText(null);
    setEmbedPreview(null);
    void Promise.all([
      api.getFrameOcr(activeFrame.id),
      api.getFrameEmbeddingPreview(activeFrame.id),
    ])
      .then(([t, emb]) => {
        if (cancelled) return;
        setOcrText(t);
        setEmbedPreview(emb);
      })
      .catch((e) => {
        if (!cancelled) {
          console.error("active frame detail load failed", e);
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
  }, [activeFrame?.id]);

  const displayFrame = activeFrame ?? selected;
  const selectedHeld = displayFrame
    ? staticHeldLabel(displayFrame.static_until_ms, displayFrame.ts)
    : null;

  const handleFreezeFrame = (video: HTMLVideoElement) => {
    video.pause();
    const canvas = document.createElement("canvas");
    canvas.width = video.videoWidth;
    canvas.height = video.videoHeight;
    const ctx = canvas.getContext("2d");
    if (!ctx || !displayFrame) return;
    ctx.drawImage(video, 0, 0);
    canvas.toBlob((blob) => {
      if (!blob) return;
      const url = URL.createObjectURL(blob);
      setFrozenFrameUrl(url);
      // Find closest frame
      const currentMs = video.currentTime * 1000;
      if (displayFrame.video_path) {
        const segmentFrames = frames.filter(
          (f) => f.video_path === displayFrame.video_path
        );
        let closest = segmentFrames[0];
        let minDiff = Infinity;
        for (const f of segmentFrames) {
          if (f.video_offset_ms == null) continue;
          const diff = Math.abs(f.video_offset_ms - currentMs);
          if (diff < minDiff) {
            minDiff = diff;
            closest = f;
          }
        }
        if (closest) {
          setActiveFrame(closest);
        }
      }
    }, "image/png");
  };

  const handleResumeVideo = () => {
    if (frozenFrameUrl) {
      URL.revokeObjectURL(frozenFrameUrl);
      setFrozenFrameUrl(null);
    }
    setActiveFrame(null);
  };

  const copyFrozenFrame = async () => {
    if (!frozenFrameUrl) return;
    try {
      const res = await fetch(frozenFrameUrl);
      const blob = await res.blob();
      await navigator.clipboard.write([
        new ClipboardItem({ [blob.type]: blob }),
      ]);
    } catch {
      // Fallback: open in new tab
      window.open(frozenFrameUrl, "_blank");
    }
  };

  return (
    <div className="flex h-full flex-col">
      <header className="flex flex-col gap-2 border-b border-border px-4 py-2">
        <div className="flex min-h-8 items-center gap-3">
          <div className="flex items-center gap-1 rounded-md border border-border bg-bg p-0.5">
            <button
              type="button"
              onClick={() => setViewMode("latest")}
              className={"rounded px-2.5 py-1 text-xs font-medium transition-colors " + (viewMode === "latest" ? "bg-accent text-black" : "text-text-muted hover:text-text")}
            >
              Latest
            </button>
            <button
              type="button"
              onClick={() => setViewMode("archived")}
              className={"rounded px-2.5 py-1 text-xs font-medium transition-colors " + (viewMode === "archived" ? "bg-accent text-black" : "text-text-muted hover:text-text")}
            >
              Archived
            </button>
          </div>
          {viewMode === "latest" && (
            <span className="ml-auto text-xs text-text-faint">
              {latestFrames.length} shown
              {frames.length !== latestFrames.length
                ? ` of ${frames.length}`
                : ""}
              {refreshSecs > 0 ? ` · refresh ${refreshSecs}s` : " · manual refresh"}
            </span>
          )}
          {viewMode === "archived" && (
            <span className="ml-auto text-xs text-text-faint">
              {filteredVideos.length} videos
              {videos.length !== filteredVideos.length ? ` of ${videos.length}` : ""}
              {refreshSecs > 0 ? ` · refresh ${refreshSecs}s` : ""}
            </span>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[10px] text-text-faint">
          {viewMode === "latest" && (
            <>
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
            </>
          )}
          {monitors.length > 1 && (
            <label className="inline-flex items-center gap-1">
              <span className="text-text-faint">Monitor</span>
              <select
                value={monitorFilter}
                onChange={(e) => setMonitorFilter(e.target.value)}
                className="max-w-[10rem] rounded border border-border bg-bg px-1 py-0.5 text-[10px] text-text"
              >
                <option value="all">all</option>
                {monitors.map(([id, name]) => (
                  <option key={id} value={String(id)}>{name}</option>
                ))}
              </select>
            </label>
          )}
          {viewMode === "latest" ? (
            <button
              type="button"
              onClick={() => void loadOlder()}
              disabled={!hasMore || loadingMore}
              className="ml-auto rounded border border-border px-2 py-0.5 text-[10px] text-text-muted hover:bg-bg-hover disabled:opacity-50"
            >
              {loadingMore ? "Loading older…" : hasMore ? "Load older" : "No older frames"}
            </button>
          ) : (
            <button
              type="button"
              onClick={() => void loadOlderVideos()}
              disabled={!hasMoreVideos || loadingMoreVideos}
              className="ml-auto rounded border border-border px-2 py-0.5 text-[10px] text-text-muted hover:bg-bg-hover disabled:opacity-50"
            >
              {loadingMoreVideos ? "Loading older…" : hasMoreVideos ? "Load older" : "No older videos"}
            </button>
          )}
        </div>
      </header>

      {loading || (viewMode === "archived" && loadingVideos) ? (
        <Empty message="Loading..." />
      ) : viewMode === "archived" ? (
        filteredVideos.length === 0 ? (
          <Empty message={videos.length === 0 ? "No archived videos yet. They appear after the archiver runs." : "No videos match the monitor filter."} />
        ) : (
          <div className="flex-1 overflow-y-auto scrollbar-thin p-4 space-y-6">
            {Object.entries(groupVideosByDay(filteredVideos)).map(([day, dayVideos]) => (
              <section key={day} className="space-y-2">
                <h2 className="text-xs font-semibold uppercase tracking-wider text-text-muted">
                  {day} · {dayVideos.length} video{dayVideos.length !== 1 ? "s" : ""}
                </h2>
                <div className="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-3">
                  {dayVideos.map((v) => (
                    <ArchivedVideoCard
                      key={v.id}
                      video={v}
                      onClick={() => {
                        const frame: Frame = {
                          id: 0,
                          ts: v.start_ts,
                          path: v.path,
                          app: null,
                          window_title: null,
                          monitor_id: v.monitor_id,
                          monitor_name: v.monitor_name,
                          ocr_done: true,
                          embed_done: true,
                          has_embedding: false,
                          static_until_ms: v.end_ts,
                          video_path: v.path,
                          video_offset_ms: 0,
                          archived_at: null,
                          source_deleted_at: null,
                        };
                        setViewer(frame);
                      }}
                      onContextMenu={(e) => {
                        e.preventDefault();
                        const frame: Frame = {
                          id: 0,
                          ts: v.start_ts,
                          path: v.path,
                          app: null,
                          window_title: null,
                          monitor_id: v.monitor_id,
                          monitor_name: v.monitor_name,
                          ocr_done: true,
                          embed_done: true,
                          has_embedding: false,
                          static_until_ms: v.end_ts,
                          video_path: v.path,
                          video_offset_ms: 0,
                          archived_at: null,
                          source_deleted_at: null,
                        };
                        setMenu({ x: e.clientX, y: e.clientY, frame });
                      }}
                    />
                  ))}
                </div>
              </section>
            ))}
          </div>
        )
      ) : frames.length === 0 ? (
        <Empty message="Nothing captured yet. ScreenRecall will start indexing in the background." />
      ) : latestFrames.length === 0 ? (
        <Empty message="No frames match the debug filters." />
      ) : (
        <div className="flex flex-1 overflow-hidden">
          <div className="flex-1 overflow-y-auto scrollbar-thin p-4 space-y-6">
            {Object.entries(groupByDay(latestFrames)).map(([day, dayFrames]) => {
              const dayItems = buildTimelineItems(dayFrames);
              return (
              <section key={day} className="space-y-2">
                <h2 className="text-xs font-semibold uppercase tracking-wider text-text-muted">
                  {day}
                </h2>
                <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
                  {dayItems.map((item) => {
                    if (item.type === "video") {
                      return (
                        <VideoSegmentCard
                          key={item.videoPath}
                          item={item}
                          selected={selected}
                          onSelect={(f) => setSelected(f)}
                          onContextMenu={(e, f) => {
                            e.preventDefault();
                            setMenu({ x: e.clientX, y: e.clientY, frame: f });
                          }}
                        />
                      );
                    }
                    const f = item.frame;
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
                          {f.monitor_name && <> · {f.monitor_name}</>}
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
              );
            })}
          </div>

          {displayFrame && (
            <aside className="w-96 shrink-0 border-l border-border bg-bg-elevated overflow-y-auto scrollbar-thin">
              {displayFrame.video_path ? (
                <div className="relative block w-full border-b border-border">
                  {frozenFrameUrl ? (
                    <div className="relative">
                      <img
                        src={frozenFrameUrl}
                        alt="Frozen frame"
                        className="w-full"
                      />
                      <button
                        type="button"
                        onClick={handleResumeVideo}
                        className="absolute right-2 top-2 rounded border border-border bg-bg/80 p-1 text-text-muted hover:bg-bg-hover"
                        title="Resume video"
                      >
                        <Film className="h-3.5 w-3.5" />
                      </button>
                    </div>
                  ) : (
                    <div className="relative">
                      <video
                        src={api.assetUrl(displayFrame.video_path)}
                        className="w-full"
                        controls
                        preload="metadata"
                        onLoadedMetadata={(e) => {
                          const video = e.currentTarget;
                          if (displayFrame.video_offset_ms != null) {
                            video.currentTime = displayFrame.video_offset_ms / 1000;
                          }
                        }}
                      />
                      <button
                        type="button"
                        onClick={(e) => {
                          const video = (e.currentTarget.parentElement?.querySelector("video")) as HTMLVideoElement | null;
                          if (video) handleFreezeFrame(video);
                        }}
                        className="absolute right-2 top-2 rounded border border-border bg-bg/80 p-1 text-text-muted hover:bg-bg-hover"
                        title="Freeze frame at current time"
                      >
                        <ImageIcon className="h-3.5 w-3.5" />
                      </button>
                    </div>
                  )}
                </div>
              ) : (
                <button
                  type="button"
                  onClick={() => setViewer(displayFrame)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    setMenu({ x: e.clientX, y: e.clientY, frame: displayFrame });
                  }}
                  className="relative block w-full border-b border-border"
                  title="Open large preview"
                >
                  <img
                    src={api.assetUrl(displayFrame.path)}
                    alt=""
                    decoding="async"
                    className="w-full"
                  />
                  <span className="absolute right-2 top-2 rounded border border-border bg-bg/80 p-1 text-text-muted">
                    <Expand className="h-3.5 w-3.5" />
                  </span>
                </button>
              )}
              <div className="p-4 space-y-3">
                <div>
                  <div className="text-xs text-text-muted">Frame ID</div>
                  <div className="text-sm font-mono">#{displayFrame.id}</div>
                </div>
                <div>
                  <div className="text-xs text-text-muted">Time</div>
                  <div className="text-sm">
                    {format(displayFrame.ts, "PPpp")}
                    {selectedHeld && (
                      <div className="mt-0.5 text-xs text-text-muted">
                        {selectedHeld} (end{" "}
                        {format(displayFrame.static_until_ms, "PPpp")})
                      </div>
                    )}
                  </div>
                </div>
                <div>
                  <div className="text-xs text-text-muted">App</div>
                  <div className="text-sm">{displayFrame.app ?? "—"}</div>
                </div>
                <div>
                  <div className="text-xs text-text-muted">Window</div>
                  <div className="text-sm break-words">
                    {displayFrame.window_title ?? "—"}
                  </div>
                </div>
                <div>
                  <div className="flex items-center justify-between gap-2">
                    <div className="text-xs text-text-muted">File</div>
                    <div className="flex gap-1">
                      {frozenFrameUrl && (
                        <button
                          type="button"
                          className="inline-flex items-center gap-1 rounded border border-border px-1.5 py-0.5 text-[10px] text-text-muted hover:bg-bg-hover"
                          onClick={copyFrozenFrame}
                          title="Copy frozen frame to clipboard"
                        >
                          <Copy className="h-3 w-3" />
                          Copy Image
                        </button>
                      )}
                      <button
                        type="button"
                        className="inline-flex items-center gap-1 rounded border border-border px-1.5 py-0.5 text-[10px] text-text-muted hover:bg-bg-hover"
                        title="Open in File Explorer"
                        onClick={() => {
                          void api.revealFrameInFolder(displayFrame.video_path ?? displayFrame.path).catch(() => {});
                        }}
                      >
                        <FolderOpen className="h-3 w-3" />
                        Open location
                      </button>
                    </div>
                  </div>
                  <div className="mt-0.5 text-[11px] font-medium text-text break-all">
                    {pathBasename(displayFrame.video_path ?? displayFrame.path)}
                  </div>
                  <div className="mt-0.5 break-all font-mono text-[10px] leading-snug text-text-faint">
                    {displayFrame.video_path ?? displayFrame.path}
                  </div>
                </div>
                <div className="flex flex-wrap gap-2 text-[10px] text-text-faint">
                  <span
                    className={
                      "rounded px-1.5 py-0.5 " +
                      (displayFrame.ocr_done
                        ? "bg-emerald-500/10 text-emerald-300"
                        : "bg-bg-hover")
                    }
                  >
                    OCR {displayFrame.ocr_done ? "✓" : "…"}
                  </span>
                  <EmbedPill f={displayFrame} />
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
                      {displayFrame.ocr_done
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
                  ) : !displayFrame.has_embedding || !embedPreview ? (
                    <div className="mt-1 text-xs text-text-faint">
                      {displayFrame.embed_done
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
        <FrameViewer
          frame={viewer}
          onClose={() => setViewer(null)}
          showIdBadge
          showFolderOpen
          onContextMenu={(e) => {
            e.preventDefault();
            setMenu({ x: e.clientX, y: e.clientY, frame: viewer });
          }}
        />
      )}
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={closeMenu}
          items={[
            {
              label: "Open in new window",
              icon: <ExternalLink className="h-3.5 w-3.5" />,
              onClick: () => openFrameWindow(menu.frame),
            },
            {
              label: "Open file location",
              icon: <FolderOpen className="h-3.5 w-3.5" />,
              onClick: () => void api.revealFrameInFolder(menu.frame.video_path ?? menu.frame.path).catch(() => {}),
            },
          ]}
        />
      )}
    </div>
  );
}

function VideoSegmentCard({
  item,
  selected,
  onSelect,
  onContextMenu,
}: {
  item: { type: "video"; videoPath: string; frames: Frame[] };
  selected: Frame | null;
  onSelect: (f: Frame) => void;
  onContextMenu: (e: React.MouseEvent, f: Frame) => void;
}) {
  const first = item.frames[0];
  const last = item.frames[item.frames.length - 1];
  const isSelected = item.frames.some((f) => f.id === selected?.id);
  const durationSec = Math.round((last.ts - first.ts) / 1000);
  return (
    <div
      className={
        "group overflow-hidden rounded-lg border bg-bg-elevated transition-all " +
        (isSelected ? "border-accent ring-1 ring-accent" : "border-border hover:border-border-strong")
      }
    >
      <div className="aspect-video bg-black/40 overflow-hidden">
        <video
          src={api.assetUrl(item.videoPath)}
          className="h-full w-full object-cover opacity-90 group-hover:opacity-100"
          preload="metadata"
          muted
          onLoadedMetadata={(e) => {
            const video = e.currentTarget;
            if (first.video_offset_ms != null) {
              video.currentTime = first.video_offset_ms / 1000;
            }
          }}
          onClick={() => onSelect(first)}
          onContextMenu={(e) => onContextMenu(e, first)}
        />
      </div>
      <div className="px-3 py-2">
        <div className="flex items-center gap-1.5">
          <Film className="h-3 w-3 text-blue-400" />
          <span className="truncate text-xs text-text">
            {first.window_title ?? first.app ?? "—"}
          </span>
        </div>
        <div className="text-[10px] text-text-faint">
          {format(first.ts, "HH:mm:ss")} – {format(last.ts, "HH:mm:ss")}
          {" · "}
          {durationSec}s
          {" · "}
          {item.frames.length} captures
          {first.monitor_name && <> · {first.monitor_name}</>}
        </div>
      </div>
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

function groupVideosByDay(videos: Video[]): Record<string, Video[]> {
  const out: Record<string, Video[]> = {};
  for (const v of videos) {
    const key = format(v.created_at, "EEEE, MMM d");
    (out[key] ??= []).push(v);
  }
  return out;
}

function ArchivedVideoCard({ video, onClick, onContextMenu }: { video: Video; onClick: () => void; onContextMenu: (e: React.MouseEvent) => void }) {
  const durationSec = Math.round((video.end_ts - video.start_ts) / 1000);
  return (
    <button
      type="button"
      onClick={onClick}
      onContextMenu={onContextMenu}
      className="group overflow-hidden rounded-lg border border-border bg-bg-elevated text-left"
    >
      <div className="aspect-video bg-black/40 overflow-hidden">
        <video src={api.assetUrl(video.path)} className="h-full w-full object-cover" preload="metadata" muted />
      </div>
      <div className="p-3 space-y-1">
        <div className="flex items-center gap-1.5 text-xs">
          <Film className="h-3 w-3 text-blue-400 shrink-0" />
          <span className="truncate text-text-muted">{video.path.split(/[/\\]/).pop()}</span>
        </div>
        <div className="text-[10px] text-text-faint">
          {format(video.start_ts, "HH:mm:ss")} – {format(video.end_ts, "HH:mm:ss")}
          {" · "}
          {durationSec}s
          {" · "}
          {video.frame_count} captures
          {video.monitor_name && <> · {video.monitor_name}</>}
        </div>
      </div>
    </button>
  );
}

function Empty({ message }: { message: string }) {
  return (
    <div className="flex flex-1 items-center justify-center text-sm text-text-muted">
      {message}
    </div>
  );
}
