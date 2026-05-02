import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { Frame } from "./api";

export function openFrameWindow(frame: Frame): void {
  const label = `frame-${frame.id}-${Date.now()}`;
  const routeUrl =
    `/#/frame-window` +
    `?src=${encodeURIComponent(frame.path)}` +
    `&title=${encodeURIComponent(frame.window_title ?? frame.app ?? "ScreenRecall Frame")}`;
  const win = new WebviewWindow(label, {
    url: routeUrl,
    title: frame.window_title ?? frame.app ?? "ScreenRecall Frame",
    width: 1400,
    height: 900,
    resizable: true,
    center: true,
  });
  win.once("tauri://error", (e) =>
    console.error("open frame window failed", e),
  );
}
