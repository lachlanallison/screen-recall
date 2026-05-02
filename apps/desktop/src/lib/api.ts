import { invoke } from "@tauri-apps/api/core";
import { convertFileSrc } from "@tauri-apps/api/core";

/* ------------------------------------------------------------------ */
/* Types                                                              */
/* ------------------------------------------------------------------ */

export type Frame = {
  id: number;
  ts: number; // unix ms
  path: string; // absolute path on disk
  app: string | null;
  window_title: string | null;
  monitor_id: number;
  ocr_done: boolean;
  embed_done: boolean;
  /** True when a row exists in the embeddings table (searchable vector). */
  has_embedding: boolean;
  /** Last unix ms this view still matched; duration ≈ static_until_ms - ts. */
  static_until_ms?: number;
};

export type SearchHit = {
  frame: Frame;
  score: number;
  snippet: string | null;
};

export type EmbeddingPreview = {
  dim: number;
  values: number[];
};

export type AppConfig = {
  capture_interval_secs: number;
  retention_days: number;
  data_dir: string;
  excluded_processes: string[];
  excluded_window_substrings: string[];
  llm_backend: "ollama" | "openai";
  ollama_url: string;
  openai_base_url: string;
  /** If set, POST /v1/embeddings uses this base (e.g. second llama-server); chat uses `openai_base_url`. */
  openai_embedding_base_url: string | null;
  openai_api_key: string;
  chat_model: string;
  /** When null/empty, ScreenRecall uses its built-in Chat system prompt. */
  chat_system_prompt: string | null;
  embed_model: string;
  vision_model: string | null;
  managed_chat_server_command: string | null;
  managed_embed_server_command: string | null;
  managed_server_working_dir: string | null;
  managed_chat_server_autostart: boolean;
  managed_embed_server_autostart: boolean;
  ocr_engine: "tesseract" | "native" | "vision";
  setup_complete: boolean;
  /** Seconds between Timeline refreshes; 0 = manual only. */
  timeline_refresh_secs: number;
  /** When true, no capture while the session is locked (e.g. Win+L). Windows only. */
  pause_when_workstation_locked: boolean;
  /** Main-window close behavior: ask, minimize-to-tray, or fully quit. */
  close_behavior: "ask" | "minimize" | "quit";
};

export type Stats = {
  frame_count: number;
  disk_mb: number;
  indexed_count: number;
};

export type OneWorkerQueue = {
  pendingInDb: number;
  activeFrameId: number | null;
  activeElapsedMs: number | null;
};

/** Last ~400 completed per-frame timings (this session), in milliseconds. */
export type StageTimingStats = {
  sampleCount: number;
  minMs: number | null;
  maxMs: number | null;
  meanMs: number | null;
  p50Ms: number | null;
  p95Ms: number | null;
};

export type WorkerQueueSnapshot = {
  ocr: OneWorkerQueue;
  embed: OneWorkerQueue;
  ocrTiming: StageTimingStats;
  embedTiming: StageTimingStats;
  frameTotal: number;
};

export type DependencyStatus = {
  key: string;
  label: string;
  status: "ok" | "missing" | "optional";
  detail: string;
};

export type DependencyReport = {
  ok: boolean;
  items: DependencyStatus[];
};

export type LlmConnectionTest = {
  ok: boolean;
  detail: string;
};

export type ManagedLlamaStatus = {
  kind: "chat" | "embed" | string;
  running: boolean;
  pid: number | null;
  command: string | null;
  cwd: string | null;
  startedMsAgo: number | null;
  stdoutPath: string | null;
  stderrPath: string | null;
  lastExitCode: number | null;
  lastStderrTail: string | null;
};

export type ManagedLlamaStartBothResult = {
  started: string[];
  skipped: string[];
};

export type LaunchOnStartupStatus = {
  enabled: boolean;
  detail: string;
};

export type HealthSnapshot = {
  sqliteMainMb: number;
  sqliteWalMb: number;
  sqliteShmMb: number;
  tesseractKnownPath: string | null;
  managedServers: ManagedLlamaStatus[];
};

/* ------------------------------------------------------------------ */
/* API                                                                */
/* ------------------------------------------------------------------ */

/* Note: Tauri 2 auto-converts JS camelCase keys -> Rust snake_case, so we
   always send camelCase from this file. */

export const api = {
  getStatus: () => invoke<{ recording: boolean }>("get_status"),
  setRecording: (on: boolean) => invoke<void>("set_recording", { on }),
  getConfig: () => invoke<AppConfig>("get_config"),
  setConfig: (config: AppConfig) => invoke<void>("set_config", { config }),
  getManagedLlamaStatus: () =>
    invoke<ManagedLlamaStatus[]>("get_managed_llama_status"),
  startManagedLlama: (args: {
    kind: "chat" | "embed";
    command: string;
    cwd?: string | null;
  }) => invoke<ManagedLlamaStatus>("start_managed_llama", args),
  startManagedLlamaBoth: () =>
    invoke<ManagedLlamaStartBothResult>("start_managed_llama_both"),
  stopManagedLlama: (kind: "chat" | "embed") =>
    invoke<ManagedLlamaStatus>("stop_managed_llama", { kind }),
  getManagedLlamaLogTail: (args: {
    kind: "chat" | "embed";
    stream?: "stdout" | "stderr";
    limit?: number;
  }) => invoke<string[]>("get_managed_llama_log_tail", args),
  getLaunchOnStartupStatus: () =>
    invoke<LaunchOnStartupStatus>("get_launch_on_startup_status"),
  setLaunchOnStartup: (enabled: boolean) =>
    invoke<LaunchOnStartupStatus>("set_launch_on_startup", { enabled }),
  testOllamaConnection: (ollamaUrl: string) =>
    invoke<LlmConnectionTest>("test_ollama_connection", { ollamaUrl }),
  testOpenaiChatConnection: (args: { baseUrl: string; apiKey: string }) =>
    invoke<LlmConnectionTest>("test_openai_chat_connection", args),
  testOpenaiEmbedConnection: (args: {
    baseUrl: string;
    apiKey: string;
    model: string;
  }) => invoke<LlmConnectionTest>("test_openai_embed_connection", args),
  loadChatUiState: () => invoke<string | null>("load_chat_ui_state"),
  saveChatUiState: (json: string) =>
    invoke<void>("save_chat_ui_state", { json }),
  checkDependencies: () => invoke<DependencyReport>("check_dependencies"),
  installTesseract: () => invoke<void>("install_tesseract"),
  installOllama: () => invoke<void>("install_ollama"),
  pullModel: (model: string) => invoke<void>("pull_model", { model }),
  completeSetup: () => invoke<void>("complete_setup"),
  getStats: () => invoke<Stats>("get_stats"),
  getHealthSnapshot: () => invoke<HealthSnapshot>("get_health_snapshot"),
  getWorkerQueueSnapshot: () =>
    invoke<WorkerQueueSnapshot>("get_worker_queue_snapshot"),

  listFrames: (args: { limit: number; beforeTs?: number | null }) =>
    invoke<Frame[]>("list_frames", args),

  search: (args: {
    query: string;
    limit: number;
    semantic: boolean;
    startTs?: number | null;
    endTs?: number | null;
  }) => invoke<SearchHit[]>("search", args),

  chat: (args: {
    prompt: string;
    sessionId: string;
    k: number;
    startTs?: number | null;
    endTs?: number | null;
  }) => invoke<void>("chat", args),

  chatCancel: (args: { sessionId: string }) =>
    invoke<void>("chat_cancel", args),

  openDataDir: () => invoke<void>("open_data_dir"),
  revealFrameInFolder: (path: string) =>
    invoke<void>("reveal_frame_in_folder", { path }),
  deleteAll: () => invoke<void>("delete_all"),
  windowMinimizeToTray: () => invoke<void>("window_minimize_to_tray"),
  windowQuitApp: () => invoke<void>("window_quit_app"),
  getPerfLogTail: (limit = 300) =>
    invoke<string[]>("get_perf_log_tail", { limit }),
  getPerfLogPath: () => invoke<string>("get_perf_log_path"),
  getRuntimeLogTail: (limit = 400) =>
    invoke<string[]>("get_runtime_log_tail", { limit }),
  getProcessLogTail: (limit = 400) =>
    invoke<string[]>("get_process_log_tail", { limit }),
  getProcessLogPath: () => invoke<string>("get_process_log_path"),

  getFrameOcr: (frameId: number) =>
    invoke<string | null>("get_frame_ocr", { frameId }),
  getFrameEmbeddingPreview: (frameId: number) =>
    invoke<EmbeddingPreview | null>("get_frame_embedding_preview", { frameId }),

  /** Reset OCR for frames with missing/pending/empty text and re-queue the OCR worker. */
  requeueOcrRerun: () => invoke<number>("requeue_ocr_rerun"),
  /** Reset embedding state for frames with OCR text but no vector. */
  requeueEmbedRerun: () => invoke<number>("requeue_embed_rerun"),

  // Helper: turn an absolute local path into a webview-loadable URL.
  assetUrl: (absolutePath: string): string => convertFileSrc(absolutePath),
};
