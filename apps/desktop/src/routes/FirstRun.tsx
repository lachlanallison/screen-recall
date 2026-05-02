import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, type AppConfig, type DependencyReport } from "../lib/api";

type PullProgress = {
  model: string;
  status: string;
  progress?: number | null;
};

export default function FirstRun({
  onComplete,
}: {
  onComplete: () => Promise<void>;
}) {
  const [report, setReport] = useState<DependencyReport | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pulls, setPulls] = useState<Record<string, PullProgress>>({});
  const [cfg, setCfg] = useState<AppConfig | null>(null);

  const refresh = async () => {
    const next = await api.checkDependencies();
    setReport(next);
    if (next.ok) {
      await api.completeSetup();
      await onComplete();
    }
  };

  useEffect(() => {
    api
      .getConfig()
      .then(setCfg)
      .catch(() => {});
    refresh().catch((e) => setError(String(e)));
    const unlisten = listen<PullProgress>("setup:pull-progress", (event) => {
      const payload = event.payload;
      const key = payload.model || "model";
      setPulls((prev) => ({ ...prev, [key]: payload }));
    });
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  const runAction = async (label: string, fn: () => Promise<void>) => {
    setBusy(label);
    setError(null);
    try {
      await fn();
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="h-full overflow-y-auto bg-bg p-6 text-text">
      <div className="mx-auto max-w-3xl space-y-6">
        <div>
          <h1 className="text-xl font-semibold">First-run setup</h1>
          <p className="mt-2 text-sm text-text-muted">
            ScreenRecall needs OCR. LLM can be local Ollama or any OpenAI-compatible
            endpoint — exact model names in Settings are optional for this step.
          </p>
        </div>

        <div className="rounded-lg border border-border bg-bg-elevated p-4">
          <h2 className="text-sm font-medium">Dependency check</h2>
          <div className="mt-3 space-y-2">
            {(report?.items ?? []).map((item) => (
              <div
                key={item.key}
                className="rounded border border-border/70 bg-bg px-3 py-2 text-xs"
              >
                <div className="flex items-center justify-between">
                  <span>{item.label}</span>
                  <span
                    className={
                      item.status === "ok"
                        ? "text-emerald-400"
                        : item.status === "optional"
                          ? "text-sky-400"
                          : "text-amber-400"
                    }
                  >
                    {item.status === "ok"
                      ? "OK"
                      : item.status === "optional"
                        ? "Optional"
                        : "Missing"}
                  </span>
                </div>
                <div className="mt-1 text-text-faint">{item.detail}</div>
              </div>
            ))}
          </div>
        </div>

        <div className="rounded-lg border border-border bg-bg-elevated p-4">
          <h2 className="text-sm font-medium">Fix missing items</h2>
          <div className="mt-3 flex flex-wrap gap-2">
            <button
              disabled={busy !== null}
              onClick={() => runAction("tesseract", api.installTesseract)}
              className="rounded-md border border-border px-3 py-2 text-xs hover:bg-bg-hover disabled:opacity-50"
            >
              Install Tesseract
            </button>
            <button
              disabled={busy !== null}
              onClick={() => runAction("ollama", api.installOllama)}
              className="rounded-md border border-border px-3 py-2 text-xs hover:bg-bg-hover disabled:opacity-50"
            >
              Install Ollama
            </button>
            <button
              disabled={busy !== null || !cfg}
              onClick={() =>
                runAction("chat model", () =>
                  api.pullModel(cfg!.chat_model.split(":")[0] || cfg!.chat_model),
                )
              }
              className="rounded-md border border-border px-3 py-2 text-xs hover:bg-bg-hover disabled:opacity-50"
            >
              Pull chat model{cfg ? ` (${cfg.chat_model})` : ""}
            </button>
            <button
              disabled={busy !== null || !cfg}
              onClick={() =>
                runAction("embed model", () =>
                  api.pullModel(cfg!.embed_model.split(":")[0] || cfg!.embed_model),
                )
              }
              className="rounded-md border border-border px-3 py-2 text-xs hover:bg-bg-hover disabled:opacity-50"
            >
              Pull embed model{cfg ? ` (${cfg.embed_model})` : ""}
            </button>
            <button
              disabled={busy !== null}
              onClick={() => runAction("recheck", refresh)}
              className="rounded-md border border-border px-3 py-2 text-xs hover:bg-bg-hover disabled:opacity-50"
            >
              Recheck
            </button>
          </div>
          {busy && (
            <p className="mt-3 text-xs text-text-muted">Working on: {busy}</p>
          )}
          {error && <p className="mt-2 text-xs text-red-300">{error}</p>}
          {!!Object.keys(pulls).length && (
            <div className="mt-3 space-y-1 text-xs text-text-muted">
              {Object.entries(pulls).map(([name, p]) => (
                <div key={name}>
                  {name}: {p.status}
                  {typeof p.progress === "number"
                    ? ` (${Math.round(p.progress * 100)}%)`
                    : ""}
                </div>
              ))}
            </div>
          )}
        </div>

        {!!report && !report.ok && (
          <p className="text-xs text-text-muted">
            Manual fallback (dev): run `scripts/setup-windows.ps1` from this
            repository, install missing pieces from the README, or adjust OCR /
            LLM settings for OpenAI-compatible endpoints — then click Recheck.
          </p>
        )}

        {!!report && report.ok && (
          <p className="text-sm text-emerald-400">All required checks passed. Continuing…</p>
        )}
      </div>
    </div>
  );
}
