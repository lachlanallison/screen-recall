import { NavLink, Route, Routes, Navigate, useLocation } from "react-router-dom";
import {
  Activity,
  CalendarClock,
  Check,
  ChevronsLeft,
  ChevronsRight,
  CircleX,
  DoorClosed,
  Minimize2,
  MessageSquare,
  Search,
  Settings as SettingsIcon,
  Pause,
  Play,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import type { DiskStatus } from "./lib/api";
import Timeline from "./routes/Timeline";
import SearchView from "./routes/Search";

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}
import Chat from "./routes/Chat";
import Settings from "./routes/Settings";
import Diagnostics from "./routes/Diagnostics";
import FirstRun from "./routes/FirstRun";
import FrameWindow from "./routes/FrameWindow";
import { api } from "./lib/api";
import { cn } from "./lib/cn";

const NAV = [
  { to: "/timeline", label: "Timeline", icon: CalendarClock },
  { to: "/search", label: "Search", icon: Search },
  { to: "/chat", label: "Chat", icon: MessageSquare },
  { to: "/settings", label: "Settings", icon: SettingsIcon },
  { to: "/diagnostics", label: "Diagnostics", icon: Activity },
];
const NAV_COLLAPSED_KEY = "screenrecall:nav-collapsed";

export default function App() {
  const location = useLocation();
  const isFrameWindow = location.pathname === "/frame-window";
  const [recording, setRecording] = useState<boolean>(true);
  const [setupReady, setSetupReady] = useState<boolean | null>(null);
  const [closeBehavior, setCloseBehavior] = useState<
    "ask" | "minimize" | "quit"
  >("ask");
  const [showClosePrompt, setShowClosePrompt] = useState(false);
  const [dontAskAgain, setDontAskAgain] = useState(false);
  const [stats, setStats] = useState<{ frames: number; bytes: number } | null>(
    null,
  );
  const [diskStatus, setDiskStatus] = useState<DiskStatus | null>(null);
  const [navCollapsed, setNavCollapsed] = useState<boolean>(() => {
    try {
      return localStorage.getItem(NAV_COLLAPSED_KEY) === "1";
    } catch {
      return false;
    }
  });
  const allowCloseRef = useRef(false);

  useEffect(() => {
    let cancelled = false;
    const bootstrap = async () => {
      try {
        const cfg = await api.getConfig();
        const depsOk = cfg.setup_complete
          ? true
          : (await api.checkDependencies()).ok;
        if (!cancelled) {
          setSetupReady(cfg.setup_complete && depsOk);
          setCloseBehavior(cfg.close_behavior);
        }
      } catch {
        if (!cancelled) setSetupReady(false);
      }
    };
    const refreshStatus = async () => {
      try {
        const s = await api.getStatus();
        if (!cancelled) setRecording(s.recording);
      } catch {}
    };
    const refreshStats = async () => {
      try {
        const st = await api.getStats();
        if (!cancelled) setStats({ frames: st.frameCount, bytes: st.diskBytes });
      } catch {}
    };
    const refreshDisk = async () => {
      try {
        const d = await api.getDiskStatus();
        if (!cancelled) setDiskStatus(d);
      } catch {}
    };

    bootstrap();
    refreshStatus();
    refreshStats();
    refreshDisk();
    const statusId = setInterval(refreshStatus, 5000);

    // Poll stats every 2s. Once cached, this is instant (just reads atomic values).
    const statsId = setInterval(refreshStats, 2000);
    const diskId = setInterval(refreshDisk, 10000);

    const unlisten = listen<DiskStatus>("screenrecall:disk-status", (ev) => {
      if (!cancelled) setDiskStatus(ev.payload);
    });

    return () => {
      cancelled = true;
      clearInterval(statusId);
      clearInterval(statsId);
      clearInterval(diskId);
      unlisten.then((f) => f());
    };
  }, []);

  const minimizeToTray = async () => {
    await api.windowMinimizeToTray();
  };

  const quitApp = async () => {
    allowCloseRef.current = true;
    await api.windowQuitApp();
  };

  const persistCloseBehavior = async (behavior: "ask" | "minimize" | "quit") => {
    try {
      const cfg = await api.getConfig();
      await api.setConfig({ ...cfg, close_behavior: behavior });
      setCloseBehavior(behavior);
    } catch (e) {
      console.error("Failed to save close behavior", e);
    }
  };

  const handlePromptChoice = async (choice: "minimize" | "quit") => {
    const persistAs = dontAskAgain ? choice : "ask";
    setShowClosePrompt(false);
    setDontAskAgain(false);
    if (persistAs !== closeBehavior) {
      await persistCloseBehavior(persistAs);
    }
    if (choice === "minimize") {
      await minimizeToTray();
    } else {
      await quitApp();
    }
  };

  useEffect(() => {
    if (isFrameWindow) return;
    let unlisten: (() => void) | undefined;
    (async () => {
      const win = getCurrentWindow();
      unlisten = await win.onCloseRequested(async (event: any) => {
        if (allowCloseRef.current) {
          allowCloseRef.current = false;
          return;
        }

        if (closeBehavior === "quit") {
          return;
        }

        event.preventDefault();
        if (closeBehavior === "minimize") {
          await minimizeToTray();
          return;
        }

        setShowClosePrompt(true);
      });
    })();

    return () => {
      if (unlisten) unlisten();
    };
  }, [isFrameWindow, closeBehavior]);

  const togglePause = async () => {
    const next = !recording;
    setRecording(next);
    try {
      await api.setRecording(next);
    } catch {
      setRecording(!next);
    }
  };

  const toggleNavCollapsed = () => {
    setNavCollapsed((v) => {
      const next = !v;
      try {
        localStorage.setItem(NAV_COLLAPSED_KEY, next ? "1" : "0");
      } catch {}
      return next;
    });
  };

  useEffect(() => {
    const onKey = (evt: KeyboardEvent) => {
      if (!(evt.ctrlKey || evt.metaKey) || evt.altKey) return;
      if (evt.key !== "\\") return;
      const t = evt.target as HTMLElement | null;
      const tag = t?.tagName?.toLowerCase();
      const isTypingTarget =
        !!t &&
        (t.isContentEditable ||
          tag === "input" ||
          tag === "textarea" ||
          tag === "select");
      if (isTypingTarget) return;
      evt.preventDefault();
      toggleNavCollapsed();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  if (isFrameWindow) {
    return (
      <Routes>
        <Route path="/frame-window" element={<FrameWindow />} />
      </Routes>
    );
  }

  if (setupReady === null) {
    return (
      <div className="flex h-full items-center justify-center bg-bg text-sm text-text-muted">
        Checking dependencies...
      </div>
    );
  }

  return (
    <div className="flex h-full w-full bg-bg text-text">
      {diskStatus?.stopped && (
        <div className="absolute inset-x-0 top-0 z-50 flex h-8 items-center justify-center bg-red-600 text-xs font-medium text-white">
          Recording stopped — disk critically low ({diskStatus.freePct.toFixed(1)}% free)
        </div>
      )}
      {diskStatus?.warning && !diskStatus.stopped && (
        <div className="absolute inset-x-0 top-0 z-50 flex h-8 items-center justify-center bg-amber-500 text-xs font-medium text-black">
          Warning — disk space low ({diskStatus.freePct.toFixed(1)}% free)
        </div>
      )}
      {setupReady && (
        <aside
          className={
            "flex shrink-0 flex-col border-r border-border bg-bg-elevated transition-[width] " +
            (navCollapsed ? "w-16" : "w-56")
          }
        >
        <div className="flex h-12 items-center gap-2 px-4 border-b border-border">
          <div className="h-5 w-5 rounded bg-accent" />
          {!navCollapsed && (
            <span className="text-sm font-semibold tracking-wide">ScreenRecall</span>
          )}
          <button
            type="button"
            onClick={toggleNavCollapsed}
            className="ml-auto rounded border border-border p-1 text-text-muted hover:bg-bg-hover hover:text-text"
            title={navCollapsed ? "Expand navigation" : "Collapse navigation"}
          >
            {navCollapsed ? (
              <ChevronsRight className="h-3.5 w-3.5" />
            ) : (
              <ChevronsLeft className="h-3.5 w-3.5" />
            )}
          </button>
        </div>

        <nav className="flex-1 p-2 space-y-0.5">
          {NAV.map(({ to, label, icon: Icon }) => (
            <NavLink
              key={to}
              to={to}
              className={({ isActive }) =>
                cn(
                  "flex items-center gap-3 rounded-md px-3 py-2 text-sm transition-colors",
                  isActive
                    ? "bg-bg-hover text-text"
                    : "text-text-muted hover:bg-bg-hover hover:text-text",
                )
              }
              title={navCollapsed ? label : undefined}
            >
              <Icon className="h-4 w-4" />
              {!navCollapsed && label}
            </NavLink>
          ))}
        </nav>

        <div className="border-t border-border p-3 space-y-2">
          <button
            onClick={togglePause}
            className={cn(
              "flex w-full items-center justify-center gap-2 rounded-md border px-3 py-1.5 text-sm font-medium transition-colors",
              recording
                ? "border-red-500/40 bg-red-500/10 text-red-300 hover:bg-red-500/20"
                : "border-emerald-500/40 bg-emerald-500/10 text-emerald-300 hover:bg-emerald-500/20",
            )}
            title={recording ? "Pause recording" : "Resume recording"}
          >
            {recording ? (
              <>
                <Pause className="h-4 w-4" /> {!navCollapsed && "Pause recording"}
              </>
            ) : (
              <>
                <Play className="h-4 w-4" /> {!navCollapsed && "Resume recording"}
              </>
            )}
          </button>
          {!navCollapsed && stats && (
            <div className="text-xs text-text-faint">
              {stats ? (
                stats.bytes === 0 && stats.frames === 0 ? (
                  <span className="text-text-faint">Calculating disk usage…</span>
                ) : (
                  <>
                    {stats.frames.toLocaleString()} frames · {formatBytes(stats.bytes)}
                  </>
                )
              ) : (
                <span className="text-text-faint">Loading…</span>
              )}
            </div>
          )}
        </div>
        </aside>
      )}

      <main className="flex-1 overflow-hidden">
        <Routes>
          <Route
            path="/"
            element={
              <Navigate to={setupReady ? "/timeline" : "/first-run"} replace />
            }
          />
          <Route
            path="/first-run"
            element={
              setupReady ? (
                <Navigate to="/timeline" replace />
              ) : (
                <FirstRun onComplete={async () => setSetupReady(true)} />
              )
            }
          />
          <Route
            path="/timeline"
            element={
              setupReady ? <Timeline /> : <Navigate to="/first-run" replace />
            }
          />
          <Route
            path="/search"
            element={
              setupReady ? <SearchView /> : <Navigate to="/first-run" replace />
            }
          />
          <Route
            path="/chat"
            element={setupReady ? <Chat /> : <Navigate to="/first-run" replace />}
          />
          <Route
            path="/settings"
            element={
              setupReady ? <Settings /> : <Navigate to="/first-run" replace />
            }
          />
          <Route
            path="/diagnostics"
            element={
              setupReady ? <Diagnostics /> : <Navigate to="/first-run" replace />
            }
          />
        </Routes>
      </main>

      {showClosePrompt && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4">
          <div className="w-full max-w-md rounded-lg border border-border bg-bg-elevated p-4 shadow-xl">
            <div className="flex items-start gap-3">
              <CircleX className="mt-0.5 h-5 w-5 text-text-faint" />
              <div className="space-y-1">
                <h2 className="text-sm font-semibold">Close ScreenRecall?</h2>
                <p className="text-xs text-text-muted">
                  Choose whether to keep ScreenRecall running in the tray or fully
                  quit it.
                </p>
              </div>
            </div>

            <label className="mt-3 flex cursor-pointer items-center gap-2 rounded-md border border-border bg-bg px-2 py-1.5 text-xs text-text-muted">
              <input
                type="checkbox"
                checked={dontAskAgain}
                onChange={(e) => setDontAskAgain(e.target.checked)}
                className="rounded border-border"
              />
              <Check className="h-3.5 w-3.5" />
              Don&apos;t ask again (can be changed in Settings)
            </label>

            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setShowClosePrompt(false)}
                className="rounded-md border border-border px-3 py-1.5 text-xs text-text-muted hover:bg-bg-hover"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => void handlePromptChoice("minimize")}
                className="inline-flex items-center gap-1 rounded-md border border-border px-3 py-1.5 text-xs hover:bg-bg-hover"
              >
                <Minimize2 className="h-3.5 w-3.5" />
                Minimize to tray
              </button>
              <button
                type="button"
                onClick={() => void handlePromptChoice("quit")}
                className="inline-flex items-center gap-1 rounded-md border border-red-500/40 bg-red-500/10 px-3 py-1.5 text-xs text-red-300 hover:bg-red-500/20"
              >
                <DoorClosed className="h-3.5 w-3.5" />
                Quit
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
