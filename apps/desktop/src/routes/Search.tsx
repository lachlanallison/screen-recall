import { useEffect, useState } from "react";
import { format } from "date-fns";
import { Expand, ExternalLink, Search as SearchIcon, Sparkles, X } from "lucide-react";
import { api, type SearchHit } from "../lib/api";
import { cn } from "../lib/cn";
import { openFrameWindow } from "../lib/frameWindow";
import { staticHeldLabel } from "../lib/staticHeld";

export default function SearchView() {
  const [query, setQuery] = useState("");
  const [semantic, setSemantic] = useState(true);
  const [loading, setLoading] = useState(false);
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [viewer, setViewer] = useState<SearchHit | null>(null);
  const [viewerFullscreen, setViewerFullscreen] = useState(false);
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    hit: SearchHit;
  } | null>(null);

  const run = async () => {
    if (!query.trim()) return;
    setLoading(true);
    setError(null);
    try {
      const res = await api.search({ query, limit: 60, semantic });
      setHits(res);
    } catch (e: unknown) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    const closeMenu = () => setMenu(null);
    const closeViewer = (evt: KeyboardEvent) => {
      if (evt.key === "Escape") {
        setViewer(null);
        setViewerFullscreen(false);
      }
    };
    window.addEventListener("click", closeMenu);
    window.addEventListener("keydown", closeViewer);
    return () => {
      window.removeEventListener("click", closeMenu);
      window.removeEventListener("keydown", closeViewer);
    };
  }, []);

  return (
    <div className="flex h-full flex-col">
      <header className="flex h-12 items-center gap-3 border-b border-border px-4">
        <h1 className="text-sm font-medium">Search</h1>
        <form
          className="ml-auto flex w-full max-w-2xl items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            run();
          }}
        >
          <div className="flex flex-1 items-center rounded-md border border-border bg-bg-elevated px-2">
            <SearchIcon className="h-4 w-4 text-text-muted" />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={
                semantic
                  ? "Describe what you're looking for…"
                  : "Full-text search OCR…"
              }
              className="flex-1 bg-transparent px-2 py-1.5 text-sm outline-none placeholder:text-text-faint"
            />
          </div>
          <button
            type="button"
            onClick={() => setSemantic(!semantic)}
            className={cn(
              "flex items-center gap-1 rounded-md border px-2 py-1.5 text-xs",
              semantic
                ? "border-accent/40 bg-accent/10 text-accent"
                : "border-border text-text-muted hover:text-text",
            )}
            title="Toggle semantic vs full-text"
          >
            <Sparkles className="h-3.5 w-3.5" />
            Semantic
          </button>
          <button
            type="submit"
            disabled={loading}
            className="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-black hover:bg-accent-hover disabled:opacity-50"
          >
            {loading ? "Searching…" : "Search"}
          </button>
        </form>
      </header>

      {error && (
        <div className="border-b border-red-500/30 bg-red-500/5 px-4 py-2 text-xs text-red-300">
          {error}
        </div>
      )}

      <div className="flex-1 overflow-y-auto scrollbar-thin p-4">
        {hits.length === 0 && !loading && (
          <div className="flex h-full items-center justify-center text-sm text-text-muted">
            Try: “error message yesterday”, “github pull request”, or
            “invoice template”.
          </div>
        )}
        <div className="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-3">
          {hits.map((h) => {
            const held = staticHeldLabel(h.frame.static_until_ms, h.frame.ts);
            return (
            <article
              key={h.frame.id}
              className="group overflow-hidden rounded-lg border border-border bg-bg-elevated"
              onContextMenu={(e) => {
                e.preventDefault();
                setMenu({ x: e.clientX, y: e.clientY, hit: h });
              }}
            >
              <div className="aspect-video overflow-hidden bg-black/40">
                <button type="button" onClick={() => setViewer(h)} className="h-full w-full">
                  <img
                    src={api.assetUrl(h.frame.path)}
                    alt=""
                    loading="lazy"
                    className="h-full w-full object-cover opacity-90 group-hover:opacity-100"
                  />
                </button>
              </div>
              <div className="p-3 space-y-1">
                <div className="flex items-center justify-between">
                  <div className="truncate text-xs font-medium">
                    {h.frame.window_title ?? h.frame.app ?? "—"}
                  </div>
                  <div className="shrink-0 text-[10px] text-text-faint">
                    {(h.score * 100).toFixed(0)}%
                  </div>
                </div>
                <div className="text-[10px] text-text-faint">
                  {format(h.frame.ts, "PPpp")}
                  {held && <span className="ml-1 text-text-muted">· {held}</span>}
                </div>
                {h.snippet && (
                  <p className="line-clamp-3 text-xs text-text-muted">
                    {h.snippet}
                  </p>
                )}
              </div>
            </article>
            );
          })}
        </div>
      </div>

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
              <span className="truncate">
                {viewer.frame.window_title ?? viewer.frame.app ?? viewer.frame.path}
              </span>
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
                onClick={() => openFrameWindow(viewer.frame)}
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
                src={api.assetUrl(viewer.frame.path)}
                alt={viewer.frame.window_title ?? viewer.frame.app ?? "Captured frame"}
                className="h-full w-full object-contain"
                onContextMenu={(e) => {
                  e.preventDefault();
                  setMenu({ x: e.clientX, y: e.clientY, hit: viewer });
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
        >
          <button
            type="button"
            onClick={() => {
              openFrameWindow(menu.hit.frame);
              setMenu(null);
            }}
            className="flex w-full items-center gap-2 rounded px-2 py-1 text-xs hover:bg-bg-hover"
          >
            <ExternalLink className="h-3.5 w-3.5" />
            Open in new window
          </button>
        </div>
      )}
    </div>
  );
}
