import { useMemo } from "react";
import { useSearchParams } from "react-router-dom";
import { api } from "../lib/api";

export default function FrameWindow() {
  const [params] = useSearchParams();
  const src = params.get("src") ?? "";
  const title = params.get("title") ?? "Frame";

  const asset = useMemo(() => (src ? api.assetUrl(src) : ""), [src]);

  if (!src) {
    return (
      <div className="flex h-full items-center justify-center bg-bg text-sm text-text-muted">
        Missing image source.
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col bg-black">
      <header className="flex h-9 shrink-0 items-center border-b border-border bg-bg-elevated px-3 text-xs text-text-muted">
        <span className="truncate">{title}</span>
      </header>
      <div className="h-[calc(100%-2.25rem)] w-full">
        <img src={asset} alt={title} className="h-full w-full object-contain" />
      </div>
    </div>
  );
}
