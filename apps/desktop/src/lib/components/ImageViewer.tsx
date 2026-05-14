import { type ReactNode, useState } from "react";
import { Expand, ExternalLink, FolderOpen, Film, X } from "lucide-react";
import { api, type Frame } from "../api";
import { openFrameWindow } from "../frameWindow";
import { useEscape } from "./useEscape";

export function FrameViewer({
  frame,
  onClose,
  onContextMenu,
  showIdBadge = false,
  showFolderOpen = false,
  children,
}: {
  frame: Frame;
  onClose: () => void;
  onContextMenu?: (e: React.MouseEvent) => void;
  showIdBadge?: boolean;
  showFolderOpen?: boolean;
  children?: ReactNode;
}) {
  const [fullscreen, setFullscreen] = useState(false);

  useEscape(onClose);

  const close = () => {
    onClose();
    setFullscreen(false);
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-4"
      onClick={close}
    >
      <div
        className={
          "relative overflow-hidden rounded-lg border border-border bg-bg-elevated " +
          (fullscreen ? "h-[95vh] w-[95vw]" : "h-[80vh] w-[80vw] max-w-6xl")
        }
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 border-b border-border px-3 py-2 text-xs text-text-muted">
          {showIdBadge && (
            <span className="shrink-0 font-mono">#{frame.id}</span>
          )}
          <span className="truncate">
            {frame.window_title ?? frame.app ?? frame.path}
          </span>
          {frame.video_path && (
            <span className="ml-1 inline-flex items-center gap-1 rounded bg-blue-500/10 px-1.5 py-0.5 text-[10px] text-blue-300">
              <Film className="h-3 w-3" /> video
            </span>
          )}
          {children}
          <button
            type="button"
            onClick={() => setFullscreen((v) => !v)}
            className="ml-auto rounded border border-border p-1 hover:bg-bg-hover"
            title={fullscreen ? "Exit fullscreen" : "Fullscreen"}
          >
            <Expand className="h-3.5 w-3.5" />
          </button>
          {showFolderOpen && (
            <button
              type="button"
              onClick={() => {
                void api.revealFrameInFolder(frame.path).catch(() => {});
              }}
              className="rounded border border-border p-1 hover:bg-bg-hover"
              title="Open file location"
            >
              <FolderOpen className="h-3.5 w-3.5" />
            </button>
          )}
          <button
            type="button"
            onClick={() => openFrameWindow(frame)}
            className="rounded border border-border p-1 hover:bg-bg-hover"
            title="Open in new window"
          >
            <ExternalLink className="h-3.5 w-3.5" />
          </button>
          <button
            type="button"
            onClick={close}
            className="rounded border border-border p-1 hover:bg-bg-hover"
            title="Close"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
        <div className="h-[calc(100%-2.25rem)] w-full bg-black">
          {frame.video_path ? (
            <video
              src={api.assetUrl(frame.video_path)}
              className="h-full w-full object-contain"
              controls
              autoPlay
              onContextMenu={onContextMenu}
            />
          ) : (
            <img
              src={api.assetUrl(frame.path)}
              alt={frame.window_title ?? frame.app ?? "Captured frame"}
              className="h-full w-full object-contain"
              onContextMenu={onContextMenu}
            />
          )}
        </div>
      </div>
    </div>
  );
}
