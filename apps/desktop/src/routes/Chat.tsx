import { useEffect, useMemo, useRef, useState, useCallback } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  Archive,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  MessageSquarePlus,
  Send,
  Square,
  Trash2,
} from "lucide-react";
import { api, normalizeFrame, type Frame } from "../lib/api";
import {
  loadChatState,
  saveChatState,
  type StoredChatState,
  type StoredThread,
} from "../lib/chatStorage";
import { openFrameWindow } from "../lib/frameWindow";
import { FrameThumbnail, FrameViewer } from "../lib/components/ImageViewer";
import { ContextMenu } from "../lib/components/ContextMenu";
import { useEscape } from "../lib/components/useEscape";

type ChatThread = StoredThread;
const CHAT_SIDEBAR_COLLAPSED_KEY = "screenrecall:chat-sidebar-collapsed";
const CHAT_SIDEBAR_WIDTH_KEY = "screenrecall:chat-sidebar-width";
const CHAT_SIDEBAR_MIN = 220;
const CHAT_SIDEBAR_MAX = 480;

function makeThread(): ChatThread {
  return { id: crypto.randomUUID(), title: "New chat", archived: false, messages: [] };
}

function normalizeBoot(s: StoredChatState | null): {
  threads: ChatThread[];
  activeThreadId: string;
  sessionId: string;
} {
  if (s && s.threads.length > 0) {
    const activeThreadId = s.threads.some((t) => t.id === s.activeThreadId)
      ? s.activeThreadId
      : s.threads[0].id;
    return {
      threads: s.threads,
      activeThreadId,
      sessionId: s.sessionId || crypto.randomUUID(),
    };
  }
  const t = makeThread();
  return { threads: [t], activeThreadId: t.id, sessionId: crypto.randomUUID() };
}

export default function Chat() {
  const [hydrated, setHydrated] = useState(false);
  const [threads, setThreads] = useState<ChatThread[]>([]);
  const [activeThreadId, setActiveThreadId] = useState("");
  const [prompt, setPrompt] = useState("");
  const [streaming, setStreaming] = useState(false);
  const [thinking, setThinking] = useState(false);
  const [thinkElapsedSec, setThinkElapsedSec] = useState(0);
  const thinkTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const thinkTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** Thread that initiated the current in-flight chat (for stall timeout). */
  const stallThreadRef = useRef<string | null>(null);
  const [showArchived, setShowArchived] = useState(false);
  const [viewer, setViewer] = useState<Frame | null>(null);
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    frame: Frame;
  } | null>(null);
  const sessionRef = useRef("");
  const [sidebarCollapsed, setSidebarCollapsed] = useState<boolean>(() => {
    try {
      return localStorage.getItem(CHAT_SIDEBAR_COLLAPSED_KEY) === "1";
    } catch {
      return false;
    }
  });
  const [sidebarWidth, setSidebarWidth] = useState<number>(() => {
    try {
      const raw = Number(localStorage.getItem(CHAT_SIDEBAR_WIDTH_KEY) ?? "288");
      if (Number.isFinite(raw)) {
        return Math.max(CHAT_SIDEBAR_MIN, Math.min(CHAT_SIDEBAR_MAX, raw));
      }
    } catch {}
    return 288;
  });

  useEffect(() => {
    let cancelled = false;
    (async () => {
      let boot: ReturnType<typeof normalizeBoot> | null = null;
      try {
        const disk = await api.loadChatUiState();
        if (!cancelled && disk) {
          const parsed = JSON.parse(disk) as Partial<StoredChatState>;
          if (
            Array.isArray(parsed.threads) &&
            parsed.threads.length > 0 &&
            typeof parsed.activeThreadId === "string"
          ) {
            boot = normalizeBoot(parsed as StoredChatState);
          }
        }
      } catch {
        /* ignore */
      }
      if (!boot) {
        boot = normalizeBoot(loadChatState());
      }
      if (cancelled) return;
      setThreads(boot.threads);
      setActiveThreadId(boot.activeThreadId);
      sessionRef.current = boot.sessionId;
      setHydrated(true);
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!hydrated) return;
    const payload = {
      threads,
      activeThreadId,
      sessionId: sessionRef.current,
    };
    saveChatState(payload);
    void api.saveChatUiState(JSON.stringify(payload)).catch(() => {});
  }, [threads, activeThreadId, hydrated]);
  const runRef = useRef<{ startedAt: number; assistantId: string } | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  const activeThread = useMemo(
    () => threads.find((t) => t.id === activeThreadId) ?? threads[0],
    [threads, activeThreadId],
  );
  const messages = activeThread?.messages ?? [];
  const visibleThreads = useMemo(
    () => threads.filter((t) => (showArchived ? t.archived : !t.archived)),
    [threads, showArchived],
  );

  useEffect(() => {
    let disposed = false;
    const disposers: UnlistenFn[] = [];

    const attach = async (
      event: string,
      handler: (evt: any) => void,
    ): Promise<void> => {
      const off = await listen(event, handler);
      if (disposed) {
        off();
        return;
      }
      disposers.push(off);
    };

    const register = async () => {
      await attach("chat:delta", (evt: { payload: { session_id: string; delta: string } }) => {
        if (evt.payload.session_id !== sessionRef.current) return;
        const delta = evt.payload.delta;
        if (!delta) return;
        stallThreadRef.current = null;
        setThinking(false);
        setThreads((prev) =>
          prev.map((t) => {
            if (t.id !== activeThreadId) return t;
            const clone = [...t.messages];
            const last = clone[clone.length - 1];
            if (!last || last.role !== "assistant") return t;
            if (delta.length <= 16 && last.content.endsWith(delta)) return t;
            clone[clone.length - 1] = { ...last, content: last.content + delta };
            return { ...t, messages: clone };
          }),
        );
      });

      await attach("chat:citations", (evt: { payload: { session_id: string; frames: Frame[] } }) => {
        if (evt.payload.session_id !== sessionRef.current) return;
        setThreads((prev) =>
          prev.map((t) => {
            if (t.id !== activeThreadId) return t;
            const clone = [...t.messages];
            const last = clone[clone.length - 1];
            if (!last || last.role !== "assistant") return t;
            clone[clone.length - 1] = {
              ...last,
              citations: evt.payload.frames.map(normalizeFrame),
            };
            return { ...t, messages: clone };
          }),
        );
      });

      await attach("chat:done", (evt: { payload: { session_id: string } }) => {
        if (evt.payload.session_id !== sessionRef.current) return;
        stallThreadRef.current = null;
        setThinking(false);
        setStreaming(false);
        const run = runRef.current;
        if (!run) return;
        const ms = Math.max(1, Date.now() - run.startedAt);
        setThreads((prev) =>
          prev.map((t) => {
            if (t.id !== activeThreadId) return t;
            const clone = [...t.messages];
            const idx = clone.findIndex((m) => m.id === run.assistantId);
            if (idx < 0) return t;
            const msg = clone[idx];
            const chars = msg.content.length;
            clone[idx] = {
              ...msg,
              stats: {
                ms,
                chars,
                est_tps: Number(((chars / 4) / (ms / 1000)).toFixed(1)),
              },
            };
            return { ...t, messages: clone };
          }),
        );
      });

      await attach("chat:error", (evt: { payload: { session_id: string; error: string } }) => {
        if (evt.payload.session_id !== sessionRef.current) return;
        stallThreadRef.current = null;
        setThreads((prev) =>
          prev.map((t) =>
            t.id !== activeThreadId
              ? t
              : {
                  ...t,
                  messages: [
                    ...t.messages,
                    { id: crypto.randomUUID(), role: "assistant", content: `Error: ${evt.payload.error}` },
                  ],
                },
          ),
        );
        setThinking(false);
        setStreaming(false);
      });
    };

    register();
    return () => {
      disposed = true;
      disposers.forEach((d) => d());
    };
  }, [activeThreadId]);

  useEffect(() => {
    scrollRef.current?.scrollTo({
      top: scrollRef.current.scrollHeight,
      behavior: "smooth",
    });
  }, [messages, streaming, thinking]);

  const CHAT_STALL_SEC = 120;

  useEffect(() => {
    if (!streaming || !thinking) {
      setThinkElapsedSec(0);
      if (thinkTimerRef.current) {
        clearInterval(thinkTimerRef.current);
        thinkTimerRef.current = null;
      }
      if (thinkTimeoutRef.current) {
        clearTimeout(thinkTimeoutRef.current);
        thinkTimeoutRef.current = null;
      }
      return;
    }
    const start = Date.now();
    setThinkElapsedSec(0);
    thinkTimerRef.current = setInterval(() => {
      setThinkElapsedSec(Math.floor((Date.now() - start) / 1000));
    }, 400);
    thinkTimeoutRef.current = setTimeout(() => {
      void api.chatCancel({ sessionId: sessionRef.current });
      setThinking(false);
      setStreaming(false);
      const tid = stallThreadRef.current;
      stallThreadRef.current = null;
      if (!tid) return;
      setThreads((prev) =>
        prev.map((t) => {
          if (t.id !== tid) return t;
          const clone = [...t.messages];
          const last = clone[clone.length - 1];
          if (last?.role === "assistant" && last.content === "") {
            clone[clone.length - 1] = {
              ...last,
              content: `No response after ${CHAT_STALL_SEC}s. The model may be stuck or overloaded — try Stop, a shorter question, or check your LLM server.`,
            };
          }
          return { ...t, messages: clone };
        }),
      );
    }, CHAT_STALL_SEC * 1000);
    return () => {
      if (thinkTimerRef.current) {
        clearInterval(thinkTimerRef.current);
        thinkTimerRef.current = null;
      }
      if (thinkTimeoutRef.current) {
        clearTimeout(thinkTimeoutRef.current);
        thinkTimeoutRef.current = null;
      }
    };
  }, [streaming, thinking]);

  const closeMenu = useCallback(() => setMenu(null), []);
  useEscape(() => { setViewer(null); setMenu(null); });

  useEffect(() => {
    window.addEventListener("click", closeMenu);
    return () => window.removeEventListener("click", closeMenu);
  }, [closeMenu]);

  const send = async () => {
    if (!hydrated || !prompt.trim() || streaming) return;
    const userMessage = prompt.trim();
    const assistantId = crypto.randomUUID();
    setThreads((prev) =>
      prev.map((t) =>
        t.id !== activeThreadId
          ? t
          : {
              ...t,
              title: t.title === "New chat" ? userMessage.slice(0, 48) : t.title,
              messages: [
                ...t.messages,
                { id: crypto.randomUUID(), role: "user", content: userMessage },
                { id: assistantId, role: "assistant", content: "" },
              ],
            },
      ),
    );
    setPrompt("");
    stallThreadRef.current = activeThreadId;
    setStreaming(true);
    setThinking(true);
    runRef.current = { startedAt: Date.now(), assistantId };
    try {
      await api.chat({
        prompt: userMessage,
        sessionId: sessionRef.current,
        k: 6,
      });
    } catch (e: unknown) {
      setThreads((prev) =>
        prev.map((t) =>
          t.id !== activeThreadId
            ? t
            : {
                ...t,
                messages: [
                  ...t.messages,
                  { id: crypto.randomUUID(), role: "assistant", content: `Error: ${String(e)}` },
                ],
              },
        ),
      );
      setThinking(false);
      setStreaming(false);
    }
  };

  const cancel = async () => {
    try {
      await api.chatCancel({ sessionId: sessionRef.current });
    } catch {}
    setThinking(false);
    setStreaming(false);
  };

  const newThread = () => {
    const t = makeThread();
    setThreads((prev) => [t, ...prev]);
    setActiveThreadId(t.id);
    setPrompt("");
  };

  const archiveThread = (id: string) => {
    setThreads((prev) => prev.map((t) => (t.id === id ? { ...t, archived: true } : t)));
    if (id === activeThreadId) {
      const next = threads.find((t) => !t.archived && t.id !== id);
      if (next) setActiveThreadId(next.id);
      else newThread();
    }
  };

  const deleteThread = (id: string) => {
    const remaining = threads.filter((t) => t.id !== id);
    if (remaining.length === 0) {
      const t = makeThread();
      setThreads([t]);
      setActiveThreadId(t.id);
      return;
    }
    setThreads(remaining);
    if (id === activeThreadId) {
      const next = remaining.find((t) => !t.archived) ?? remaining[0];
      setActiveThreadId(next.id);
    }
  };

  const toggleSidebarCollapsed = () => {
    setSidebarCollapsed((v) => {
      const next = !v;
      try {
        localStorage.setItem(CHAT_SIDEBAR_COLLAPSED_KEY, next ? "1" : "0");
      } catch {}
      return next;
    });
  };

  return (
    <div className="relative flex h-full">
      {!hydrated && (
        <div className="absolute inset-0 z-10 flex items-center justify-center bg-bg/80 backdrop-blur-[1px]">
          <span className="text-sm text-text-muted">Loading chat…</span>
        </div>
      )}
      <aside
        className="flex shrink-0 flex-col border-r border-border bg-bg-elevated"
        style={{ width: sidebarCollapsed ? 56 : sidebarWidth }}
      >
        <div className="flex h-12 items-center gap-2 border-b border-border px-3">
          {!sidebarCollapsed && <h1 className="text-sm font-medium">Chats</h1>}
          <button
            type="button"
            onClick={newThread}
            disabled={!hydrated}
            className="inline-flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs hover:bg-bg-hover disabled:opacity-50"
            title="New chat"
          >
            <MessageSquarePlus className="h-3.5 w-3.5" />
            {!sidebarCollapsed && "New"}
          </button>
          <button
            type="button"
            onClick={toggleSidebarCollapsed}
            className="ml-auto rounded border border-border p-1 text-text-muted hover:bg-bg-hover hover:text-text"
            title={sidebarCollapsed ? "Expand chat sidebar" : "Collapse chat sidebar"}
          >
            {sidebarCollapsed ? (
              <ChevronRight className="h-3.5 w-3.5" />
            ) : (
              <ChevronLeft className="h-3.5 w-3.5" />
            )}
          </button>
        </div>
        {!sidebarCollapsed && (
          <div className="border-b border-border p-2">
          <button
            type="button"
            onClick={() => setShowArchived((v) => !v)}
            disabled={!hydrated}
            className="inline-flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs hover:bg-bg-hover disabled:opacity-50"
          >
            <Archive className="h-3.5 w-3.5" />
            {showArchived ? "Archived" : "Active"}
          </button>
          </div>
        )}
        <div className="flex-1 space-y-1 overflow-y-auto p-2 scrollbar-thin">
          {visibleThreads.map((t) => (
            <button
              key={t.id}
              onClick={() => setActiveThreadId(t.id)}
              className={
                "group flex w-full items-center gap-2 rounded-md border px-2 py-2 text-left text-xs " +
                (t.id === activeThreadId
                  ? "border-accent bg-accent/10 text-accent"
                  : "border-border text-text-muted hover:text-text hover:bg-bg-hover")
              }
              title={t.title}
            >
              {!sidebarCollapsed && (
                <span className="min-w-0 flex-1 truncate">{t.title}</span>
              )}
              {sidebarCollapsed && <span className="text-[10px] font-mono">#{t.id.slice(0, 4)}</span>}
              {!t.archived && (
                <Archive
                  className={
                    "h-3.5 w-3.5 opacity-60 hover:opacity-100 " +
                    (sidebarCollapsed ? "hidden" : "")
                  }
                  onClick={(e) => {
                    e.stopPropagation();
                    archiveThread(t.id);
                  }}
                />
              )}
              <Trash2
                className={
                  "h-3.5 w-3.5 opacity-60 hover:opacity-100 hover:text-red-300 " +
                  (sidebarCollapsed ? "hidden" : "")
                }
                onClick={(e) => {
                  e.stopPropagation();
                  deleteThread(t.id);
                }}
              />
            </button>
          ))}
        </div>
      </aside>
      {!sidebarCollapsed && (
        <div
          className="w-1 cursor-col-resize bg-border/50 hover:bg-accent/40"
          onMouseDown={(e) => {
            e.preventDefault();
            const startX = e.clientX;
            const startW = sidebarWidth;
            let currentW = startW;
            const onMove = (ev: MouseEvent) => {
              currentW = Math.max(
                CHAT_SIDEBAR_MIN,
                Math.min(CHAT_SIDEBAR_MAX, startW + (ev.clientX - startX)),
              );
              setSidebarWidth(currentW);
            };
            const onUp = () => {
              try {
                localStorage.setItem(CHAT_SIDEBAR_WIDTH_KEY, String(currentW));
              } catch {}
              window.removeEventListener("mousemove", onMove);
              window.removeEventListener("mouseup", onUp);
            };
            window.addEventListener("mousemove", onMove);
            window.addEventListener("mouseup", onUp);
          }}
          title="Drag to resize chat sidebar"
        />
      )}

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-12 items-center gap-2 border-b border-border px-4">
          <h1 className="truncate text-sm font-medium">
            {activeThread?.title || "Chat with your history"}
          </h1>
        </header>

      <div ref={scrollRef} className="flex-1 overflow-y-auto px-4 py-6 scrollbar-thin">
        <div className="mx-auto max-w-3xl space-y-6">
          {messages.length === 0 && (
            <div className="rounded-lg border border-border bg-bg-elevated p-6 text-sm text-text-muted">
              Ask anything about what you've been doing. ScreenRecall will retrieve
              the most relevant frames from your recent history and answer based
              on what it saw on screen.
            </div>
          )}

          {messages.map((m, idx) => {
            const pendingAssistant =
              m.role === "assistant" &&
              !m.content &&
              streaming &&
              thinking &&
              idx === messages.length - 1;
            return (
            <div key={m.id} className="space-y-2">
              <div
                className={
                  m.role === "user"
                    ? "ml-auto max-w-[80%] rounded-2xl rounded-tr-sm bg-accent/15 px-4 py-2 text-sm"
                    : "mr-auto max-w-[90%] rounded-2xl rounded-tl-sm bg-bg-elevated px-4 py-2 text-sm whitespace-pre-wrap"
                }
              >
                {pendingAssistant ? (
                  <span className="inline-flex items-center gap-2 text-text-muted">
                    <span>
                      Thinking · {thinkElapsedSec}s
                    </span>
                    <span className="flex items-center gap-1">
                      <span className="h-1.5 w-1.5 animate-bounce rounded-full bg-text-muted [animation-delay:0ms]" />
                      <span className="h-1.5 w-1.5 animate-bounce rounded-full bg-text-muted [animation-delay:120ms]" />
                      <span className="h-1.5 w-1.5 animate-bounce rounded-full bg-text-muted [animation-delay:240ms]" />
                    </span>
                  </span>
                ) : (
                  m.content || ""
                )}
              </div>
              {m.role === "assistant" && m.stats && (
                <div className="mr-auto max-w-[90%] text-[11px] text-text-faint">
                  {`~${m.stats.est_tps} tok/s · ${(m.stats.ms / 1000).toFixed(1)}s · ${m.stats.chars} chars`}
                </div>
              )}
              {m.citations && m.citations.length > 0 && (
                <div className="mr-auto flex max-w-[90%] gap-2 overflow-x-auto scrollbar-thin">
                  {m.citations.map((f) => (
                    <button
                      key={f.id}
                      type="button"
                      onClick={() => setViewer(f)}
                      onContextMenu={(e) => {
                        e.preventDefault();
                        setMenu({ x: e.clientX, y: e.clientY, frame: f });
                      }}
                      className="shrink-0 overflow-hidden rounded border border-border hover:border-accent"
                      title={f.window_title ?? f.app ?? ""}
                    >
                      <FrameThumbnail
                        frame={f}
                        className="h-24 w-40 object-cover sm:h-28 sm:w-44"
                      />
                    </button>
                  ))}
                </div>
              )}
            </div>
            );
          })}
        </div>
      </div>

      <form
        onSubmit={(e) => {
          e.preventDefault();
          send();
        }}
        className="border-t border-border bg-bg-elevated p-3"
      >
        <div className="mx-auto flex max-w-3xl items-center gap-2">
          <input
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            placeholder="Ask about your screen history..."
            disabled={!hydrated || streaming}
            className="flex-1 rounded-md border border-border bg-bg px-3 py-2 text-sm outline-none placeholder:text-text-faint focus:border-accent"
          />
          {streaming ? (
            <button
              type="button"
              onClick={cancel}
              className="flex items-center gap-1 rounded-md border border-border px-3 py-2 text-xs hover:bg-bg-hover"
            >
              <Square className="h-3.5 w-3.5" /> Stop
            </button>
          ) : (
            <button
              type="submit"
              disabled={!prompt.trim()}
              className="flex items-center gap-1 rounded-md bg-accent px-3 py-2 text-xs font-medium text-black hover:bg-accent-hover disabled:opacity-50"
            >
              <Send className="h-3.5 w-3.5" /> Send
            </button>
          )}
        </div>
      </form>
      </div>

      {viewer && (
        <FrameViewer
          frame={viewer}
          onClose={() => setViewer(null)}
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
          ]}
        />
      )}
    </div>
  );
}
