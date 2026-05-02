import { useEffect, useState } from "react";
import { FolderOpen, Trash2 } from "lucide-react";
import {
  api,
  type AppConfig,
  type DependencyReport,
  type ManagedLlamaStatus,
} from "../lib/api";

export default function Settings() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [requeueOcrBusy, setRequeueOcrBusy] = useState(false);
  const [requeueOcrMsg, setRequeueOcrMsg] = useState<string | null>(null);
  const [llmTestBusy, setLlmTestBusy] = useState<
    "ollama" | "chat" | "embed" | null
  >(null);
  const [llmTestMsg, setLlmTestMsg] = useState<string | null>(null);
  const [managed, setManaged] = useState<ManagedLlamaStatus[]>([]);
  const [managedBusy, setManagedBusy] = useState<"chat" | "embed" | null>(null);
  const [managedMsg, setManagedMsg] = useState<string | null>(null);
  const [managedLogKind, setManagedLogKind] = useState<"chat" | "embed">("embed");
  const [managedLogStdout, setManagedLogStdout] = useState("");
  const [managedLogStderr, setManagedLogStderr] = useState("");
  const [launchOnStartup, setLaunchOnStartup] = useState<boolean>(false);
  const [launchBusy, setLaunchBusy] = useState(false);
  const [depReport, setDepReport] = useState<DependencyReport | null>(null);
  const [depLoading, setDepLoading] = useState(false);
  const [depDismissed, setDepDismissed] = useState(false);

  useEffect(() => {
    api
      .getConfig()
      .then(setConfig)
      .catch((e) => console.error(e));
  }, []);

  useEffect(() => {
    let mounted = true;
    const loadDeps = async () => {
      setDepLoading(true);
      try {
        const r = await api.checkDependencies();
        if (mounted) setDepReport(r);
      } catch {
        if (mounted) setDepReport(null);
      } finally {
        if (mounted) setDepLoading(false);
      }
    };
    void loadDeps();
    return () => {
      mounted = false;
    };
  }, []);

  useEffect(() => {
    let mounted = true;
    const poll = async () => {
      try {
        const s = await api.getManagedLlamaStatus();
        if (mounted) setManaged(s);
      } catch {
        if (mounted) setManaged([]);
      }
    };
    void poll();
    const id = window.setInterval(() => void poll(), 5000);
    return () => {
      mounted = false;
      clearInterval(id);
    };
  }, []);

  useEffect(() => {
    let mounted = true;
    void api
      .getLaunchOnStartupStatus()
      .then((s) => {
        if (mounted) setLaunchOnStartup(s.enabled);
      })
      .catch(() => {});
    return () => {
      mounted = false;
    };
  }, []);

  const save = async () => {
    if (!config) return;
    setSaving(true);
    setSaved(false);
    try {
      await api.setConfig(config);
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } finally {
      setSaving(false);
    }
  };

  if (!config) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-text-muted">
        Loading config…
      </div>
    );
  }

  const patch = (partial: Partial<AppConfig>) =>
    setConfig((c) => (c ? { ...c, ...partial } : c));

  const testLlmOllama = async () => {
    if (!config) return;
    setLlmTestBusy("ollama");
    setLlmTestMsg(null);
    try {
      const r = await api.testOllamaConnection(config.ollama_url);
      setLlmTestMsg(
        `${r.ok ? "OK" : "Failed"} (Ollama /api/tags): ${r.detail}`,
      );
    } catch (e) {
      setLlmTestMsg(`Ollama test error: ${String(e)}`);
    } finally {
      setLlmTestBusy(null);
    }
  };

  const testLlmOpenaiChat = async () => {
    if (!config) return;
    setLlmTestBusy("chat");
    setLlmTestMsg(null);
    try {
      const r = await api.testOpenaiChatConnection({
        baseUrl: config.openai_base_url,
        apiKey: config.openai_api_key,
      });
      setLlmTestMsg(
        `${r.ok ? "OK" : "Failed"} (chat /v1/models): ${r.detail}`,
      );
    } catch (e) {
      setLlmTestMsg(`Chat server test error: ${String(e)}`);
    } finally {
      setLlmTestBusy(null);
    }
  };

  const resolveEmbedTestBase = (): string | null => {
    if (!config) return null;
    const o = config.openai_embedding_base_url?.trim();
    if (o) return o;
    if (config.llm_backend === "openai")
      return config.openai_base_url.trim() || null;
    return null;
  };

  const testLlmOpenaiEmbed = async () => {
    if (!config) return;
    const base = resolveEmbedTestBase();
    if (!base) {
      setLlmTestMsg(
        "Set “Embeddings base URL” (Ollama backend) or a chat base (OpenAI-compatible) first.",
      );
      return;
    }
    setLlmTestBusy("embed");
    setLlmTestMsg(null);
    try {
      const r = await api.testOpenaiEmbedConnection({
        baseUrl: base,
        apiKey: config.openai_api_key,
        model: config.embed_model,
      });
      setLlmTestMsg(
        `${r.ok ? "OK" : "Failed"} (embed /v1/embeddings → ${base}): ${r.detail}`,
      );
    } catch (e) {
      setLlmTestMsg(`Embedding server test error: ${String(e)}`);
    } finally {
      setLlmTestBusy(null);
    }
  };

  const testBtnClass =
    "shrink-0 rounded-md border border-border px-2.5 py-1.5 text-xs text-text hover:bg-bg-hover disabled:opacity-50";
  const managedBtnClass =
    "rounded-md border border-border px-2.5 py-1.5 text-xs text-text hover:bg-bg-hover disabled:opacity-50";

  const isBlockingDep = (item: DependencyReport["items"][number]): boolean => {
    if (item.status === "ok" || item.status === "optional") return false;
    if (item.key === "tesseract" && config.ocr_engine !== "tesseract") return false;
    if (item.key === "ollama" && config.llm_backend === "openai") return false;
    return true;
  };

  const blockingDeps = (depReport?.items ?? []).filter(isBlockingDep);
  const hasBlockingDeps = blockingDeps.length > 0;
  const onlyTesseractMissingForNonTesseract =
    hasBlockingDeps &&
    blockingDeps.every((d) => d.key === "tesseract") &&
    config.ocr_engine !== "tesseract";

  const statusFor = (kind: "chat" | "embed"): ManagedLlamaStatus | null =>
    managed.find((m) => m.kind === kind) ?? null;

  const startManaged = async (kind: "chat" | "embed") => {
    if (!config) return;
    const command =
      kind === "chat"
        ? (config.managed_chat_server_command ?? "")
        : (config.managed_embed_server_command ?? "");
    if (!command.trim()) {
      setManagedMsg(
        `Set a ${kind} server command first (Managed llama.cpp section).`,
      );
      return;
    }
    setManagedBusy(kind);
    setManagedMsg(null);
    try {
      const out = await api.startManagedLlama({
        kind,
        command: command.trim(),
        cwd: config.managed_server_working_dir?.trim() || null,
      });
      setManagedMsg(
        `Started ${kind} server (pid ${out.pid ?? "unknown"}). Save commands in Settings for reuse.`,
      );
      setManaged(await api.getManagedLlamaStatus());
    } catch (e) {
      setManagedMsg(`Start ${kind} server failed: ${String(e)}`);
    } finally {
      setManagedBusy(null);
    }
  };

  const startManagedBoth = async () => {
    setManagedBusy("chat");
    setManagedMsg(null);
    try {
      const r = await api.startManagedLlamaBoth();
      setManagedMsg(
        `Started: ${r.started.join(", ") || "(none)"}; skipped (no command): ${r.skipped.join(", ") || "(none)"}`,
      );
      setManaged(await api.getManagedLlamaStatus());
    } catch (e) {
      setManagedMsg(`Start both failed: ${String(e)}`);
    } finally {
      setManagedBusy(null);
    }
  };

  const stopManaged = async (kind: "chat" | "embed") => {
    setManagedBusy(kind);
    setManagedMsg(null);
    try {
      await api.stopManagedLlama(kind);
      setManagedMsg(`Stopped ${kind} server.`);
      setManaged(await api.getManagedLlamaStatus());
    } catch (e) {
      setManagedMsg(`Stop ${kind} server failed: ${String(e)}`);
    } finally {
      setManagedBusy(null);
    }
  };

  const refreshManagedLogs = async (kind: "chat" | "embed") => {
    try {
      const [out, err] = await Promise.all([
        api.getManagedLlamaLogTail({ kind, stream: "stdout", limit: 120 }),
        api.getManagedLlamaLogTail({ kind, stream: "stderr", limit: 120 }),
      ]);
      setManagedLogKind(kind);
      setManagedLogStdout(out.join("\n"));
      setManagedLogStderr(err.join("\n"));
    } catch (e) {
      setManagedMsg(`Load ${kind} logs failed: ${String(e)}`);
    }
  };

  return (
    <div className="flex h-full flex-col">
      {hasBlockingDeps && !depDismissed && (
        <div className="border-b border-amber-500/40 bg-amber-500/10 px-4 py-3">
          <div className="mx-auto max-w-2xl space-y-2">
            <div className="text-xs font-medium text-amber-200">
              Dependency check found missing requirements
            </div>
            <div className="space-y-1">
              {blockingDeps.map((d) => (
                <div
                  key={d.key}
                  className="rounded border border-amber-500/30 bg-bg px-2 py-1 text-[11px] text-text-muted"
                >
                  <span className="text-text">{d.label}:</span> {d.detail}
                </div>
              ))}
            </div>
            <div className="flex items-center gap-2">
              <button
                type="button"
                className={managedBtnClass}
                disabled={depLoading}
                onClick={async () => {
                  setDepLoading(true);
                  try {
                    const r = await api.checkDependencies();
                    setDepReport(r);
                    if (!(r.items ?? []).filter(isBlockingDep).length) {
                      setDepDismissed(false);
                    }
                  } finally {
                    setDepLoading(false);
                  }
                }}
              >
                {depLoading ? "Checking…" : "Recheck dependencies"}
              </button>
              {onlyTesseractMissingForNonTesseract && (
                <button
                  type="button"
                  className={managedBtnClass}
                  onClick={() => setDepDismissed(true)}
                >
                  Dismiss (using non-Tesseract OCR)
                </button>
              )}
            </div>
          </div>
        </div>
      )}
      <header className="flex h-12 items-center border-b border-border px-4">
        <h1 className="text-sm font-medium">Settings</h1>
        <div className="ml-auto flex items-center gap-2">
          {saved && (
            <span className="text-xs text-emerald-400">Saved.</span>
          )}
          <button
            onClick={save}
            disabled={saving}
            className="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-black hover:bg-accent-hover disabled:opacity-50"
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </header>

      <div className="flex-1 overflow-y-auto scrollbar-thin">
        <div className="mx-auto max-w-2xl p-6 space-y-8">
          <Section title="Capture">
            <Field label="Interval (seconds)">
              <input
                type="number"
                min={1}
                max={60}
                value={config.capture_interval_secs}
                onChange={(e) =>
                  patch({
                    capture_interval_secs: Number(e.target.value),
                  })
                }
                className="input"
              />
            </Field>
            <Field label="Retention (days, 0 = forever)">
              <input
                type="number"
                min={0}
                value={config.retention_days}
                onChange={(e) =>
                  patch({ retention_days: Number(e.target.value) })
                }
                className="input"
              />
            </Field>
            <Field label="Timeline auto-refresh (seconds, 0 = off)">
              <input
                type="number"
                min={0}
                max={300}
                value={config.timeline_refresh_secs}
                onChange={(e) =>
                  patch({
                    timeline_refresh_secs: Math.max(
                      0,
                      Math.min(300, Number(e.target.value) || 0),
                    ),
                  })
                }
                className="input"
              />
              <p className="mt-1 text-[11px] text-text-faint">
                How often the Timeline reloads new frames. Save, then switch back to Timeline (or
                focus the window) to apply.
              </p>
            </Field>
            <Field label="Pause when screen is locked (Windows)">
              <div className="space-y-1">
                <label className="flex cursor-pointer items-center gap-2 text-sm text-text">
                  <input
                    type="checkbox"
                    checked={config.pause_when_workstation_locked}
                    onChange={(e) =>
                      patch({ pause_when_workstation_locked: e.target.checked })
                    }
                    className="rounded border-border"
                  />
                  <span>Do not record while the workstation is locked (Win+L)</span>
                </label>
                <p className="text-[11px] text-text-faint">
                  Saves disk when the session is locked. No effect on macOS/Linux until supported
                  there.
                </p>
              </div>
            </Field>
            <Field label="Close button behavior">
              <div className="space-y-1">
                <div className="flex flex-wrap gap-2">
                  {(
                    [
                      ["ask", "Ask each time"],
                      ["minimize", "Minimize to tray"],
                      ["quit", "Quit app"],
                    ] as const
                  ).map(([value, label]) => (
                    <button
                      key={value}
                      type="button"
                      onClick={() => patch({ close_behavior: value })}
                      className={
                        "rounded-md border px-3 py-1.5 text-xs " +
                        (config.close_behavior === value
                          ? "border-accent bg-accent/10 text-accent"
                          : "border-border text-text-muted hover:text-text")
                      }
                    >
                      {label}
                    </button>
                  ))}
                </div>
                <p className="text-[11px] text-text-faint">
                  Controls what happens when you click the top-right X on the main window.
                </p>
              </div>
            </Field>
            <Field label="Data directory">
              <div className="flex items-center gap-2">
                <input
                  value={config.data_dir}
                  onChange={(e) => patch({ data_dir: e.target.value })}
                  className="input flex-1 font-mono text-xs"
                />
                <button
                  onClick={() => api.openDataDir()}
                  className="rounded-md border border-border p-1.5 hover:bg-bg-hover"
                  title="Open folder"
                >
                  <FolderOpen className="h-4 w-4" />
                </button>
              </div>
            </Field>
          </Section>

          <Section title="Privacy">
            <Field label="Excluded process names (one per line)">
              <textarea
                rows={3}
                value={config.excluded_processes.join("\n")}
                onChange={(e) =>
                  patch({
                    excluded_processes: splitLines(e.target.value),
                  })
                }
                className="input font-mono text-xs"
                placeholder="1Password.exe&#10;keepassxc.exe"
              />
            </Field>
            <Field label="Excluded window title substrings (one per line)">
              <textarea
                rows={3}
                value={config.excluded_window_substrings.join("\n")}
                onChange={(e) =>
                  patch({
                    excluded_window_substrings: splitLines(e.target.value),
                  })
                }
                className="input font-mono text-xs"
                placeholder="Incognito&#10;Private Browsing"
              />
            </Field>
          </Section>

          <Section title="LLM">
            <Field label="Backend">
              <div className="flex gap-2">
                {(["ollama", "openai"] as const).map((b) => (
                  <button
                    key={b}
                    onClick={() => patch({ llm_backend: b })}
                    className={
                      "rounded-md border px-3 py-1.5 text-xs " +
                      (config.llm_backend === b
                        ? "border-accent bg-accent/10 text-accent"
                        : "border-border text-text-muted hover:text-text")
                    }
                  >
                    {b === "ollama" ? "Ollama (local)" : "OpenAI-compatible"}
                  </button>
                ))}
              </div>
            </Field>
            {config.llm_backend === "ollama" ? (
              <>
                <Field label="Ollama URL">
                  <div className="flex gap-2">
                    <input
                      value={config.ollama_url}
                      onChange={(e) => patch({ ollama_url: e.target.value })}
                      className="input flex-1"
                      placeholder="http://localhost:11434"
                    />
                    <button
                      type="button"
                      className={testBtnClass}
                      disabled={llmTestBusy !== null}
                      onClick={testLlmOllama}
                    >
                      {llmTestBusy === "ollama" ? "…" : "Test"}
                    </button>
                  </div>
                </Field>
                <Field label="Embeddings (OpenAI base, optional)">
                  <div className="flex gap-2">
                    <input
                      value={config.openai_embedding_base_url ?? ""}
                      onChange={(e) => {
                        const v = e.target.value.trim();
                        patch({
                          openai_embedding_base_url: v ? v : null,
                        });
                      }}
                      className="input flex-1"
                      placeholder="http://127.0.0.1:8081/v1"
                    />
                    <button
                      type="button"
                      className={testBtnClass}
                      disabled={llmTestBusy !== null}
                      onClick={testLlmOpenaiEmbed}
                    >
                      {llmTestBusy === "embed" ? "…" : "Test"}
                    </button>
                  </div>
                  <p className="mt-1 text-[11px] text-text-faint">
                    A separate <code className="font-mono">llama-server</code> for{" "}
                    <code className="font-mono">POST /v1/embeddings</code> (e.g. Vulkan + embedding
                    GGUF). For llama.cpp you must start that process with{" "}
                    <code className="font-mono">--embeddings</code> (otherwise HTTP 501 on this
                    route). When set, semantic search uses this instead of the Ollama URL. Save after
                    changing.
                  </p>
                </Field>
              </>
            ) : (
              <>
                <Field label="Base URL (chat)">
                  <div className="flex gap-2">
                    <input
                      value={config.openai_base_url}
                      onChange={(e) =>
                        patch({ openai_base_url: e.target.value })
                      }
                      className="input flex-1"
                      placeholder="https://api.openai.com/v1"
                    />
                    <button
                      type="button"
                      className={testBtnClass}
                      disabled={llmTestBusy !== null}
                      onClick={testLlmOpenaiChat}
                    >
                      {llmTestBusy === "chat" ? "…" : "Test"}
                    </button>
                  </div>
                </Field>
                <Field label="Embeddings base URL (optional)">
                  <div className="flex gap-2">
                    <input
                      value={config.openai_embedding_base_url ?? ""}
                      onChange={(e) => {
                        const v = e.target.value.trim();
                        patch({
                          openai_embedding_base_url: v ? v : null,
                        });
                      }}
                      className="input flex-1"
                      placeholder="http://127.0.0.1:8081/v1"
                    />
                    <button
                      type="button"
                      className={testBtnClass}
                      disabled={llmTestBusy !== null}
                      onClick={testLlmOpenaiEmbed}
                    >
                      {llmTestBusy === "embed" ? "…" : "Test"}
                    </button>
                  </div>
                  <p className="mt-1 text-[11px] text-text-faint">
                    When set, semantic search uses this server for{" "}
                    <code className="font-mono">/v1/embeddings</code> (e.g. a second
                    <code className="font-mono">llama-server</code> for an embedding-only GGUF on
                    the GPU). For llama.cpp, start that process with{" "}
                    <code className="font-mono">--embeddings</code> (otherwise HTTP 501). Chat
                    still uses Base URL above. Save after changing; Test uses the field above
                    and “Embedding model” together.
                  </p>
                </Field>
                <Field label="API key">
                  <input
                    type="password"
                    value={config.openai_api_key}
                    onChange={(e) =>
                      patch({ openai_api_key: e.target.value })
                    }
                    className="input"
                  />
                </Field>
              </>
            )}
            <Field label="Chat model">
              <input
                value={config.chat_model}
                onChange={(e) => patch({ chat_model: e.target.value })}
                className="input"
              />
            </Field>
            <Field label="System prompt (optional)">
              <textarea
                rows={9}
                value={config.chat_system_prompt ?? ""}
                onChange={(e) => {
                  const v = e.target.value;
                  patch({
                    chat_system_prompt: v.trim() ? v : null,
                  });
                }}
                className="input min-h-[8rem] font-mono text-xs"
                placeholder="Leave empty to use ScreenRecall's default instructions (OCR context, citations, limitations)."
              />
              <div className="mt-2 flex flex-wrap gap-2">
                <button
                  type="button"
                  className={managedBtnClass}
                  onClick={() => patch({ chat_system_prompt: null })}
                >
                  Use built-in default
                </button>
              </div>
              <p className="mt-1 text-[11px] text-text-faint">
                Replaces only the assistant system message. The user message still includes
                retrieved screen context. Save before chatting.
              </p>
            </Field>
            <Field label="Embedding model">
              <input
                value={config.embed_model}
                onChange={(e) => patch({ embed_model: e.target.value })}
                className="input"
              />
              <p className="mt-1 text-[11px] text-text-faint">
                Use a real embedding model name the server accepts (e.g.{" "}
                <code className="font-mono">nomic-embed-text</code> in GGUF, not a chat
                model). If you set an optional <code className="font-mono">llama-server</code> for
                embeddings, use the name that server expects in{" "}
                <code className="font-mono">/v1/embeddings</code>. If that route is missing on the
                primary server and you did not set a dedicated embeddings base, ScreenRecall falls
                back to the Ollama URL for semantic search.
              </p>
            </Field>
            {llmTestMsg && (
              <p
                className={
                  "rounded-md border border-border bg-bg-elevated p-2 text-[11px] font-mono " +
                  (llmTestMsg.startsWith("OK") || llmTestMsg.startsWith("Failed")
                    ? "text-text"
                    : "text-amber-400/90")
                }
              >
                {llmTestMsg}
              </p>
            )}
            <Field label="Vision model (optional)">
              <input
                value={config.vision_model ?? ""}
                onChange={(e) =>
                  patch({
                    vision_model: e.target.value.trim() || null,
                  })
                }
                placeholder="e.g. llava, moondream"
                className="input"
              />
            </Field>
            <Field label="Managed llama.cpp servers (optional)">
              <div className="space-y-2">
                <p className="text-[11px] text-text-faint">
                  Run local <code className="font-mono">llama-server</code> commands under ScreenRecall
                  (all-in-one mode). Commands run via OS shell and are kept as child processes of
                  this app. Use one line per command.
                </p>
                <div className="flex items-center gap-4 text-[11px] text-text-muted">
                  <div className="inline-flex items-center gap-1.5">
                    <input
                      id="managed-chat-autostart"
                      type="checkbox"
                      checked={config.managed_chat_server_autostart}
                      onChange={(e) =>
                        patch({ managed_chat_server_autostart: e.target.checked })
                      }
                    />
                    <label htmlFor="managed-chat-autostart">
                      Auto-start chat on app launch
                    </label>
                  </div>
                  <div className="inline-flex items-center gap-1.5">
                    <input
                      id="managed-embed-autostart"
                      type="checkbox"
                      checked={config.managed_embed_server_autostart}
                      onChange={(e) =>
                        patch({ managed_embed_server_autostart: e.target.checked })
                      }
                    />
                    <label htmlFor="managed-embed-autostart">
                      Auto-start embeddings on app launch
                    </label>
                  </div>
                </div>
                <div className="flex items-center gap-2">
                  <button
                    type="button"
                    className={managedBtnClass}
                    disabled={managedBusy !== null}
                    onClick={() => void startManagedBoth()}
                  >
                    {managedBusy ? "Starting…" : "Start both servers"}
                  </button>
                </div>
                <input
                  value={config.managed_server_working_dir ?? ""}
                  onChange={(e) => {
                    const v = e.target.value.trim();
                    patch({ managed_server_working_dir: v ? v : null });
                  }}
                  className="input font-mono text-xs"
                  placeholder="Working dir (optional), e.g. C:\\Users\\Lachlan\\llama.cpp"
                />
                <div className="space-y-1">
                  <div className="text-[11px] text-text-muted">
                    Chat server command (for Base URL /v1, e.g. port 8080)
                  </div>
                  <textarea
                    rows={2}
                    value={config.managed_chat_server_command ?? ""}
                    onChange={(e) => {
                      const v = e.target.value.trim();
                      patch({ managed_chat_server_command: v ? v : null });
                    }}
                    className="input font-mono text-[11px]"
                    placeholder='.\build\bin\Release\llama-server.exe -m "C:\models\chat.gguf" --host 127.0.0.1 --port 8080 -ngl 999 -c 8192'
                  />
                  <div className="flex items-center gap-2">
                    <button
                      type="button"
                      className={managedBtnClass}
                      disabled={managedBusy !== null || !!statusFor("chat")?.running}
                      onClick={() => void startManaged("chat")}
                    >
                      {managedBusy === "chat" ? "Starting…" : "Start chat server"}
                    </button>
                    <button
                      type="button"
                      className={managedBtnClass}
                      disabled={managedBusy !== null}
                      onClick={() => void stopManaged("chat")}
                    >
                      {managedBusy === "chat" ? "Stopping…" : "Stop"}
                    </button>
                    <span className="text-[11px] text-text-faint">
                      {statusFor("chat")?.running
                        ? `running (pid ${statusFor("chat")?.pid ?? "?"})`
                        : "stopped"}
                    </span>
                    <button
                      type="button"
                      className={managedBtnClass}
                      onClick={() => void refreshManagedLogs("chat")}
                    >
                      Logs
                    </button>
                  </div>
                  {!statusFor("chat")?.running && statusFor("chat")?.lastStderrTail && (
                    <pre className="max-h-24 overflow-auto rounded border border-border bg-bg p-2 text-[10px] text-amber-300 scrollbar-thin">
                      {statusFor("chat")?.lastStderrTail}
                    </pre>
                  )}
                </div>
                <div className="space-y-1">
                  <div className="text-[11px] text-text-muted">
                    Embed server command (for /v1/embeddings, e.g. port 8081 with --embeddings)
                  </div>
                  <textarea
                    rows={2}
                    value={config.managed_embed_server_command ?? ""}
                    onChange={(e) => {
                      const v = e.target.value.trim();
                      patch({ managed_embed_server_command: v ? v : null });
                    }}
                    className="input font-mono text-[11px]"
                    placeholder='.\build\bin\Release\llama-server.exe -m "C:\models\embed.gguf" --host 127.0.0.1 --port 8081 --embeddings -ngl 99 -c 1024 -b 1024'
                  />
                  <div className="flex items-center gap-2">
                    <button
                      type="button"
                      className={managedBtnClass}
                      disabled={managedBusy !== null || !!statusFor("embed")?.running}
                      onClick={() => void startManaged("embed")}
                    >
                      {managedBusy === "embed" ? "Starting…" : "Start embeddings server"}
                    </button>
                    <button
                      type="button"
                      className={managedBtnClass}
                      disabled={managedBusy !== null}
                      onClick={() => void stopManaged("embed")}
                    >
                      {managedBusy === "embed" ? "Stopping…" : "Stop"}
                    </button>
                    <span className="text-[11px] text-text-faint">
                      {statusFor("embed")?.running
                        ? `running (pid ${statusFor("embed")?.pid ?? "?"})`
                        : "stopped"}
                    </span>
                    <button
                      type="button"
                      className={managedBtnClass}
                      onClick={() => void refreshManagedLogs("embed")}
                    >
                      Logs
                    </button>
                  </div>
                  {!statusFor("embed")?.running && statusFor("embed")?.lastStderrTail && (
                    <pre className="max-h-24 overflow-auto rounded border border-border bg-bg p-2 text-[10px] text-amber-300 scrollbar-thin">
                      {statusFor("embed")?.lastStderrTail}
                    </pre>
                  )}
                </div>
                <div className="space-y-1">
                  <div className="text-[11px] text-text-muted">
                    {managedLogKind} logs (latest lines)
                  </div>
                  <details className="rounded border border-border bg-bg-elevated p-2">
                    <summary className="cursor-pointer text-[11px] text-text-muted">stderr</summary>
                    <pre className="mt-2 max-h-28 overflow-auto whitespace-pre-wrap break-words text-[10px] text-amber-300 scrollbar-thin">
                      {managedLogStderr || "(no stderr yet)"}
                    </pre>
                  </details>
                  <details className="rounded border border-border bg-bg-elevated p-2">
                    <summary className="cursor-pointer text-[11px] text-text-muted">stdout</summary>
                    <pre className="mt-2 max-h-28 overflow-auto whitespace-pre-wrap break-words text-[10px] text-text-muted scrollbar-thin">
                      {managedLogStdout || "(no stdout yet)"}
                    </pre>
                  </details>
                </div>
                {managedMsg && (
                  <p className="text-[11px] text-text-muted">{managedMsg}</p>
                )}
              </div>
            </Field>
          </Section>

          <Section title="OCR">
            <Field label="Engine">
              <div className="flex flex-wrap gap-2">
                {(["tesseract", "native", "vision"] as const).map((o) => (
                  <button
                    key={o}
                    onClick={() => patch({ ocr_engine: o })}
                    className={
                      "rounded-md border px-3 py-1.5 text-xs " +
                      (config.ocr_engine === o
                        ? "border-accent bg-accent/10 text-accent"
                        : "border-border text-text-muted hover:text-text")
                    }
                  >
                    {o === "tesseract"
                      ? "Tesseract"
                      : o === "native"
                        ? "Platform native"
                        : "Vision LLM"}
                  </button>
                ))}
              </div>
            </Field>
            <Field label="Re-run OCR (empty or stuck)">
              <p className="text-[11px] text-text-faint">
                Re-queues frames that are still waiting on OCR or finished with no text. Frames that
                already have stored OCR text are left unchanged. Use this after changing OCR
                software or if old WebP captures never got text.
              </p>
              <button
                type="button"
                disabled={requeueOcrBusy}
                onClick={async () => {
                  setRequeueOcrBusy(true);
                  setRequeueOcrMsg(null);
                  try {
                    const n = await api.requeueOcrRerun();
                    setRequeueOcrMsg(
                      n === 0
                        ? "No matching frames; nothing to re-queue."
                        : `Re-queued ${n} frame(s). OCR runs in the background.`,
                    );
                  } catch (e) {
                    setRequeueOcrMsg(
                      e instanceof Error ? e.message : "Re-queue failed.",
                    );
                  } finally {
                    setRequeueOcrBusy(false);
                  }
                }}
                className="mt-2 rounded-md border border-border px-3 py-1.5 text-xs text-text hover:bg-bg-hover disabled:opacity-50"
              >
                {requeueOcrBusy ? "Re-queuing…" : "Re-queue empty / pending OCR"}
              </button>
              {requeueOcrMsg && (
                <p className="mt-1.5 text-xs text-text-muted">{requeueOcrMsg}</p>
              )}
            </Field>
          </Section>

          <Section title="App">
            <Field label="Startup">
              <div className="inline-flex items-center gap-2 text-[11px] text-text-muted">
                <input
                  id="launch-on-startup"
                  type="checkbox"
                  checked={launchOnStartup}
                  disabled={launchBusy}
                  onChange={async (e) => {
                    const v = e.target.checked;
                    setLaunchBusy(true);
                    try {
                      const r = await api.setLaunchOnStartup(v);
                      setLaunchOnStartup(r.enabled);
                      setManagedMsg(r.detail);
                    } catch (err) {
                      setManagedMsg(`Set launch on startup failed: ${String(err)}`);
                    } finally {
                      setLaunchBusy(false);
                    }
                  }}
                />
                <label htmlFor="launch-on-startup">
                  Launch ScreenRecall when Windows starts
                </label>
              </div>
            </Field>
          </Section>

          <Section title="Danger zone">
            <button
              onClick={async () => {
                if (
                  confirm(
                    "Delete all captured frames and index? This cannot be undone.",
                  )
                ) {
                  await api.deleteAll();
                }
              }}
              className="flex items-center gap-2 rounded-md border border-red-500/40 bg-red-500/5 px-3 py-2 text-xs text-red-300 hover:bg-red-500/10"
            >
              <Trash2 className="h-4 w-4" />
              Delete all data
            </button>
          </Section>
        </div>
      </div>

      <style>{`
        .input {
          width: 100%;
          border-radius: 0.375rem;
          border: 1px solid #232932;
          background: #0b0d10;
          padding: 0.45rem 0.6rem;
          font-size: 0.8125rem;
          color: #e6e8eb;
          outline: none;
        }
        .input:focus { border-color: #7c9cff; }
      `}</style>
    </div>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-3">
      <h2 className="text-xs font-semibold uppercase tracking-wider text-text-muted">
        {title}
      </h2>
      <div className="space-y-3 rounded-lg border border-border bg-bg-elevated p-4">
        {children}
      </div>
    </section>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="block space-y-1">
      <div className="text-xs text-text-muted">{label}</div>
      {children}
    </div>
  );
}

function splitLines(s: string): string[] {
  return s
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter(Boolean);
}
