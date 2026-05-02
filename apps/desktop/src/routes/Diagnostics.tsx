import { useEffect, useMemo, useState } from "react";
import { api } from "../lib/api";
import type {
  AppConfig,
  HealthSnapshot,
  StageTimingStats,
  WorkerQueueSnapshot,
} from "../lib/api";

/** Bar fill scales with backlog; over ~this many pending, the bar reads as “full”. */
const QUEUE_VISUAL_CAP = 200;

type PerfEvent = {
  ts: string;
  event: string;
  fields?: Record<string, unknown>;
};

function parseLines(lines: string[]): PerfEvent[] {
  const out: PerfEvent[] = [];
  for (const l of lines) {
    try {
      const x = JSON.parse(l) as PerfEvent;
      if (x && typeof x.event === "string") out.push(x);
    } catch {
      // keep parsing best-effort
    }
  }
  return out;
}

function median(nums: number[]): number {
  if (nums.length === 0) return 0;
  const sorted = [...nums].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

function formatActive(ms: number | null | undefined): string {
  if (ms == null) return "";
  if (ms >= 10_000) return `${(ms / 1000).toFixed(0)}s`;
  if (ms >= 1000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.round(ms)}ms`;
}

function formatMsStat(n: number | null | undefined): string {
  if (n == null || Number.isNaN(n)) return "—";
  if (n >= 10_000) return `${(n / 1000).toFixed(1)}s`;
  if (n >= 1000) return `${(n / 1000).toFixed(2)}s`;
  return `${Math.round(n)}ms`;
}

/** Rolling in-process stats (last ~400 completed jobs per stage). */
function formatStageTiming(t: StageTimingStats): string {
  if (t.sampleCount === 0) {
    return "Recent per frame: no samples yet (this session).";
  }
  const mean =
    t.meanMs != null && Number.isFinite(t.meanMs)
      ? `${t.meanMs < 100 ? t.meanMs.toFixed(1) : Math.round(t.meanMs)}ms`
      : "—";
  return `Recent per frame (${t.sampleCount} samples): min ${formatMsStat(t.minMs)} · p50 ${formatMsStat(t.p50Ms)} · mean ${mean} · p95 ${formatMsStat(t.p95Ms)} · max ${formatMsStat(t.maxMs)}`;
}

function WorkerQueueBar({
  title,
  q,
  timing,
  accentClass,
}: {
  title: string;
  q: { pendingInDb: number; activeFrameId: number | null; activeElapsedMs: number | null };
  timing: StageTimingStats;
  accentClass: string;
}) {
  const fillPct = Math.min(100, (q.pendingInDb / QUEUE_VISUAL_CAP) * 100);
  const status =
    q.activeFrameId != null
      ? `Frame ${q.activeFrameId}${q.activeElapsedMs != null ? ` · ${formatActive(q.activeElapsedMs)}` : ""}`
      : "Idle (no in-flight job)";
  return (
    <div className="space-y-2">
      <div className="flex items-baseline justify-between gap-2">
        <span className="text-xs font-medium text-text">{title}</span>
        <span className="shrink-0 text-[11px] text-text-faint">
          {q.pendingInDb} pending in DB
        </span>
      </div>
      <div
        className="h-2.5 w-full overflow-hidden rounded bg-bg"
        title={`Backlog visual cap ≈${QUEUE_VISUAL_CAP} (bar full ≈ that many or more pending)`}
      >
        <div
          className={"h-full rounded transition-[width] duration-500 ease-out " + accentClass}
          style={{ width: `${fillPct}%` }}
        />
      </div>
      <p className="text-[11px] text-text-muted">{status}</p>
      <p className="text-[11px] leading-snug text-text-faint">{formatStageTiming(timing)}</p>
    </div>
  );
}

export default function Diagnostics() {
  const [loading, setLoading] = useState(false);
  const [requeueEmbedBusy, setRequeueEmbedBusy] = useState(false);
  const [requeueEmbedMsg, setRequeueEmbedMsg] = useState<string | null>(null);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [recording, setRecording] = useState<boolean | null>(null);
  const [perfPath, setPerfPath] = useState("");
  const [perfRaw, setPerfRaw] = useState("");
  const [runtimeRaw, setRuntimeRaw] = useState("");
  const [processPath, setProcessPath] = useState("");
  const [processRaw, setProcessRaw] = useState("");
  const [queues, setQueues] = useState<WorkerQueueSnapshot | null>(null);
  const [health, setHealth] = useState<HealthSnapshot | null>(null);

  const refresh = async () => {
    setLoading(true);
    try {
      const [path, procPath, perf, runtime, processLines, q, snap] =
        await Promise.all([
          api.getPerfLogPath(),
          api.getProcessLogPath(),
          api.getPerfLogTail(500),
          api.getRuntimeLogTail(500),
          api.getProcessLogTail(500),
          api.getWorkerQueueSnapshot().catch(() => null),
          api.getHealthSnapshot().catch(() => null),
        ]);
      const [cfg, status] = await Promise.all([
        api.getConfig(),
        api.getStatus(),
      ]);
      setConfig(cfg);
      setRecording(status.recording);
      setPerfPath(path);
      setProcessPath(procPath);
      setPerfRaw(perf.join("\n"));
      setRuntimeRaw(runtime.join("\n"));
      setProcessRaw(processLines.join("\n"));
      if (q) setQueues(q);
      setHealth(snap);
    } finally {
      setLoading(false);
    }
  };

  const report = useMemo(() => {
    const events = parseLines(perfRaw ? perfRaw.split("\n") : []);
    const by: Record<string, PerfEvent[]> = {};
    for (const e of events) (by[e.event] ??= []).push(e);
    const msFor = (name: string) =>
      (by[name] ?? [])
        .map((e) => Number((e.fields as any)?.ms ?? 0))
        .filter((n) => Number.isFinite(n) && n > 0);
    const ocr = msFor("ocr_ok");
    const embed = msFor("embed_ok");
    const chat = msFor("chat_ok");
    const timeoutCount = (by["chat_timeout"] ?? []).length;
    const embedErrCount = (by["embed_error"] ?? []).length;
    const semFallbackCount = (by["search_semantic_fallback"] ?? []).length;

    const lines = [
      "ScreenRecall Diagnostics",
      `Perf log path: ${perfPath || "(unknown)"}`,
      `Process log path: ${processPath || "(unknown)"}`,
      `Events parsed: ${events.length}`,
      "",
      "Current configuration:",
      `- Recording: ${recording === null ? "unknown" : recording ? "on" : "paused"}`,
      `- OCR engine: ${config?.ocr_engine ?? "unknown"}`,
      `- LLM backend: ${config?.llm_backend ?? "unknown"}`,
      `- Chat model: ${config?.chat_model ?? "unknown"}`,
      `- Embed model: ${config?.embed_model ?? "unknown"}`,
      `- Vision model: ${config?.vision_model ?? "(none)"}`,
      `- OpenAI base: ${config?.openai_base_url ?? "unknown"}`,
      `- OpenAI embed base: ${config?.openai_embedding_base_url?.trim() || "(same as OpenAI base / default)"}`,
      `- Ollama URL: ${config?.ollama_url ?? "unknown"}`,
      `- Pause on lockscreen: ${config?.pause_when_workstation_locked ?? "unknown"}`,
      `- Close behavior: ${config?.close_behavior ?? "unknown"}`,
      "",
      ...(health
        ? [
            "Storage (SQLite files on disk — large main DB is normal with embeddings):",
            `- screenrecall.db ≈ ${health.sqliteMainMb.toFixed(1)} MB`,
            `- screenrecall.db-wal ≈ ${health.sqliteWalMb.toFixed(1)} MB`,
            `- screenrecall.db-shm ≈ ${health.sqliteShmMb.toFixed(1)} MB`,
            `- Tesseract known path: ${health.tesseractKnownPath ?? "(PATH / custom — not in standard locations)"}`,
            ...health.managedServers.map(
              (m) =>
                `- Managed ${m.kind}: ${m.running ? `running pid ${m.pid ?? "?"}` : "stopped"}${
                  !m.running && m.lastExitCode != null
                    ? ` (last exit ${m.lastExitCode})`
                    : ""
                }`,
            ),
            "",
          ]
        : []),
      `OCR ok count: ${(by["ocr_ok"] ?? []).length}, median ms: ${median(ocr).toFixed(0)}`,
      `Embed ok count: ${(by["embed_ok"] ?? []).length}, median ms: ${median(embed).toFixed(0)}`,
      `Chat ok count: ${(by["chat_ok"] ?? []).length}, median ms: ${median(chat).toFixed(0)}`,
      `Chat timeouts: ${timeoutCount}`,
      `Embed errors: ${embedErrCount}`,
      `Search semantic fallbacks: ${semFallbackCount}`,
      "",
      "Top event counts:",
      ...Object.entries(by)
        .sort((a, b) => b[1].length - a[1].length)
        .slice(0, 12)
        .map(([k, v]) => `- ${k}: ${v.length}`),
    ];
    if (queues) {
      lines.push("");
      lines.push("Indexer queue (live):");
      lines.push(`- Frame rows (total): ${queues.frameTotal}`);
      lines.push(
        `- OCR: ${queues.ocr.pendingInDb} pending in DB; active ${queues.ocr.activeFrameId != null ? `frame ${queues.ocr.activeFrameId} (${formatActive(queues.ocr.activeElapsedMs ?? null)})` : "none"}`,
      );
      lines.push(`- OCR timing (session): ${formatStageTiming(queues.ocrTiming)}`);
      lines.push(
        `- Embed: ${queues.embed.pendingInDb} pending in DB; active ${queues.embed.activeFrameId != null ? `frame ${queues.embed.activeFrameId} (${formatActive(queues.embed.activeElapsedMs ?? null)})` : "none"}`,
      );
      lines.push(`- Embed timing (session): ${formatStageTiming(queues.embedTiming)}`);
    }
    return lines.join("\n");
  }, [config, perfPath, processPath, perfRaw, recording, queues, health]);

  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {}
  };

  useEffect(() => {
    void refresh();
  }, []);

  useEffect(() => {
    const tick = () => {
      void Promise.all([
        api
          .getWorkerQueueSnapshot()
          .then(setQueues)
          .catch(() => setQueues(null)),
        api
          .getHealthSnapshot()
          .then(setHealth)
          .catch(() => setHealth(null)),
      ]);
    };
    tick();
    const id = setInterval(tick, 4000);
    return () => clearInterval(id);
  }, []);

  return (
    <div className="flex h-full flex-col">
      <header className="flex h-12 items-center border-b border-border px-4">
        <h1 className="text-sm font-medium">Diagnostics</h1>
        <div className="ml-auto flex flex-wrap justify-end gap-2">
          <button
            type="button"
            onClick={async () => {
              setRequeueEmbedBusy(true);
              setRequeueEmbedMsg(null);
              try {
                const n = await api.requeueEmbedRerun();
                setRequeueEmbedMsg(`Requeued ${n} frame(s) for embedding retry.`);
                await refresh();
              } catch (e) {
                setRequeueEmbedMsg(`Requeue embed failed: ${String(e)}`);
              } finally {
                setRequeueEmbedBusy(false);
              }
            }}
            disabled={requeueEmbedBusy}
            className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-bg-hover disabled:opacity-50"
            title="Set embed_done = 0 for frames with OCR text but no vector"
          >
            {requeueEmbedBusy ? "Requeueing…" : "Requeue embeds"}
          </button>
          <button
            type="button"
            onClick={() => void refresh()}
            className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-bg-hover"
          >
            {loading ? "Loading…" : "Refresh"}
          </button>
          <button
            type="button"
            onClick={() => void copy(report)}
            className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-bg-hover"
          >
            Copy report
          </button>
          <button
            type="button"
            onClick={() => void copy(perfRaw)}
            className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-bg-hover"
          >
            Copy perf log
          </button>
          <button
            type="button"
            onClick={() => void copy(processRaw)}
            className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-bg-hover"
          >
            Copy process log
          </button>
        </div>
      </header>
      {requeueEmbedMsg && (
        <div className="border-b border-border px-4 py-2 text-xs text-text-muted">
          {requeueEmbedMsg}
        </div>
      )}
      <div className="grid flex-1 grid-cols-1 gap-3 overflow-y-auto p-3 xl:grid-cols-2">
        <section className="col-span-1 flex flex-col gap-3 rounded-md border border-border bg-bg-elevated p-4 xl:col-span-2">
          <div>
            <h2 className="text-xs font-semibold text-text">Indexer queue</h2>
            <p className="mt-0.5 text-[11px] text-text-faint">
              Backlog = SQLite (frames not finished for that stage). Current job = frame being
              processed now. Channel depth is not exposed; a burst of new captures is reflected as DB
              backlog. Bars scale to ~{QUEUE_VISUAL_CAP} pending, then read as full. “Recent per
              frame” uses the last ~400 completed jobs per stage in this app session (includes skips
              and failed API calls, so a server error still adds a sample).
            </p>
            {queues && (
              <p className="mt-1 text-[11px] text-text-muted">
                Total frames in DB: {queues.frameTotal}
              </p>
            )}
          </div>
          {queues ? (
            <div className="grid gap-6 sm:grid-cols-2">
              <WorkerQueueBar
                title="OCR worker"
                q={queues.ocr}
                timing={queues.ocrTiming}
                accentClass="bg-sky-500/80"
              />
              <WorkerQueueBar
                title="Embedding worker"
                q={queues.embed}
                timing={queues.embedTiming}
                accentClass="bg-violet-500/80"
              />
            </div>
          ) : (
            <p className="text-xs text-text-faint">Queue snapshot unavailable.</p>
          )}
        </section>

        <section className="col-span-1 space-y-2 rounded-md border border-border bg-bg-elevated p-4 xl:col-span-2">
          <div>
            <h2 className="text-xs font-semibold text-text">Subsystems &amp; storage</h2>
            <p className="mt-0.5 text-[11px] text-text-faint">
              Managed llama servers are ScreenRecall child processes. If one exits, check stderr in
              Settings or the process spawn log below. OCR uses short-lived{" "}
              <code className="font-mono text-[10px]">tesseract</code> subprocesses only while
              indexing. Very large{" "}
              <code className="font-mono text-[10px]">screenrecall.db</code> sizes are expected when
              many embeddings exist; ScreenRecall now limits SQLite memory-mapping so RSS should stay
              much lower — restart once after upgrading.
            </p>
          </div>
          {health ? (
            <div className="grid gap-3 text-[11px] sm:grid-cols-2">
              <div className="space-y-1 rounded border border-border/60 bg-bg p-3">
                <div className="font-medium text-text">SQLite files</div>
                <div className="text-text-muted">
                  screenrecall.db: {health.sqliteMainMb.toFixed(1)} MB
                </div>
                <div className="text-text-muted">
                  WAL: {health.sqliteWalMb.toFixed(1)} MB · SHM:{" "}
                  {health.sqliteShmMb.toFixed(1)} MB
                </div>
              </div>
              <div className="space-y-1 rounded border border-border/60 bg-bg p-3">
                <div className="font-medium text-text">Tesseract</div>
                <div className="break-all text-text-muted">
                  {health.tesseractKnownPath ??
                    "Not detected in Program Files paths (may still work via PATH)."}
                </div>
              </div>
              <div className="sm:col-span-2 space-y-2 rounded border border-border/60 bg-bg p-3">
                <div className="font-medium text-text">Managed llama.cpp</div>
                <ul className="space-y-1 text-text-muted">
                  {health.managedServers.map((m) => (
                    <li key={m.kind}>
                      <span className="font-mono">{m.kind}</span>:{" "}
                      {m.running ? (
                        <>
                          running
                          {m.pid != null ? ` · pid ${m.pid}` : ""}
                        </>
                      ) : (
                        <>
                          stopped
                          {m.lastExitCode != null
                            ? ` · last exit ${m.lastExitCode}`
                            : ""}
                          {m.lastStderrTail ? " · see stderr tail in Settings" : ""}
                        </>
                      )}
                    </li>
                  ))}
                </ul>
              </div>
            </div>
          ) : (
            <p className="text-xs text-text-faint">
              Health snapshot unavailable (invoke failed).
            </p>
          )}
        </section>

        <section className="flex min-h-0 flex-col rounded-md border border-border bg-bg-elevated">
          <div className="border-b border-border px-3 py-2 text-xs text-text-muted">
            Pasteable report
          </div>
          <textarea
            readOnly
            value={report}
            className="h-full w-full resize-none bg-transparent p-3 font-mono text-xs text-text outline-none"
          />
        </section>
        <section className="flex min-h-0 flex-col rounded-md border border-border bg-bg-elevated">
          <div className="border-b border-border px-3 py-2 text-xs text-text-muted">
            Perf log JSONL (latest 500 lines)
          </div>
          <textarea
            readOnly
            value={perfRaw || "(No perf log lines yet. Use the app, then Refresh.)"}
            className="h-full w-full resize-none bg-transparent p-3 font-mono text-xs text-text outline-none"
          />
        </section>
        <section className="col-span-1 flex min-h-0 flex-col rounded-md border border-border bg-bg-elevated xl:col-span-2">
          <div className="border-b border-border px-3 py-2 text-xs text-text-muted">
            Runtime log (if configured)
          </div>
          <textarea
            readOnly
            value={runtimeRaw || "(Runtime log not present yet.)"}
            className="h-full min-h-36 w-full resize-none bg-transparent p-3 font-mono text-xs text-text outline-none"
          />
        </section>
        <section className="col-span-1 flex min-h-0 flex-col rounded-md border border-border bg-bg-elevated xl:col-span-2">
          <div className="border-b border-border px-3 py-2 text-xs text-text-muted">
            Process spawn log JSONL (latest 500 lines)
          </div>
          <textarea
            readOnly
            value={processRaw || "(No process spawn log lines yet.)"}
            className="h-full min-h-36 w-full resize-none bg-transparent p-3 font-mono text-xs text-text outline-none"
          />
        </section>
      </div>
    </div>
  );
}

