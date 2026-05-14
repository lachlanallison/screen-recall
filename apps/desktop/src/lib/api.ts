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
  static_until_ms: number;
  /** Path to archived video segment (null = still on disk as individual frame file). */
  video_path: string | null;
  /** Millisecond offset into video_path where this frame lives. */
  video_offset_ms: number | null;
  /** Unix ms when this frame was archived to video (null = not archived). */
  archivedAt: number | null;
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
  ocr_engine: "tesseract";
  setup_complete: boolean;
  /** Seconds between Timeline refreshes; 0 = manual only. */
  timeline_refresh_secs: number;
  /** When true, no capture while the session is locked (e.g. Win+L). Windows only. */
  pause_when_workstation_locked: boolean;
  /** Main-window close behavior: ask, minimize-to-tray, or fully quit. */
  close_behavior: "ask" | "minimize" | "quit";
  /** Whether video archiving is enabled at all. */
  archive_enabled: boolean;
  /** FFmpeg encoder preset for video archival (e.g. "h264", "h265", "av1_qsv"). */
  archive_codec: string;
  /** Seconds between archival runs. */
  archive_interval_secs: number;
  /** Duration of each video segment in seconds. */
  archive_segment_secs: number;
  /** How many days to keep individual frame files after archiving (0 = forever). */
  archive_keep_frames_days: number;
  /** Max lookback in seconds for the automatic archiver (0 = unlimited). */
  archive_max_lookback_secs: number;
  /** Max long edge for stored frames (0 = disable downscaling, keep full resolution). */
  capture_downscale_max_edge: number;
  /** Image format for stored frames. */
  capture_image_format: "jpeg" | "webp";
  /** JPEG quality (1–100). Only used when format is JPEG. */
  capture_jpeg_quality: number;
  /** Resize filter for frame downscaling. "nearest" (default, fastest) or "lanczos3" (better OCR accuracy). */
  capture_resize_filter: "nearest" | "lanczos3";
};

export type Stats = {
  frameCount: number;
  diskBytes: number;
  indexedCount: number;
  daysRecorded: number;
  unarchivedCount: number;
  archivedCount: number;
  pendingDeletionCount: number;
  pendingDeletionDiskBytes: number;
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
  archivedFrameCount: number;
  unarchivedFrameDiskBytes: number;
};

export type EncoderPreset = {
  id: string;
  label: string;
  ffmpegEncoder: string;
  description: string;
  compressionRatio: number;
  hardwareOnly: boolean;
};

export type EncoderAvailability = {
  ffmpegFound: boolean;
  ffmpegVersion: string;
  availableEncoders: string[];
  recommended: string;
};

export type ArchiverStatus = {
  enabled: boolean;
  running: boolean;
  lastRunTs: number | null;
  lastDurationMs: number | null;
  nextRunTs: number | null;
  totalArchived: number;
  totalSegments: number;
  totalSourceDeleted: number;
  lastError: string | null;
};

export type PerPhaseStats = {
  sampleCount: number;
  capture: StageTimingStats;
  downscale: StageTimingStats;
  save: StageTimingStats;
  total: StageTimingStats;
};

export type MonitorCapturePerf = {
  monitorId: number;
  monitorName: string;
  stats: PerPhaseStats;
};

export type CapturePerfSnapshot = {
  overall: PerPhaseStats;
  byMonitor: MonitorCapturePerf[];
  /** Stats for frames that were actually saved to disk (excludes skipped/unchanged). */
  savedOverall: PerPhaseStats;
  savedByMonitor: MonitorCapturePerf[];
  lastTickMs: number;
  lastTickEnumMs: number;
};

export type DiskStatus = {
  warning: boolean;
  stopped: boolean;
  freePct: number;
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
  installFfmpeg: () => invoke<void>("install_ffmpeg"),
  ensureFfmpegPath: () => invoke<string>("ensure_ffmpeg_path"),
  pullModel: (model: string) => invoke<void>("pull_model", { model }),
  completeSetup: () => invoke<void>("complete_setup"),
  getStats: () => invoke<Stats>("get_stats"),
  getHealthSnapshot: () => invoke<HealthSnapshot>("get_health_snapshot"),
  getWorkerQueueSnapshot: () =>
    invoke<WorkerQueueSnapshot>("get_worker_queue_snapshot"),
  getEncoderAvailability: () =>
    invoke<EncoderAvailability>("get_encoder_availability"),
  getKnownEncoders: () =>
    invoke<EncoderPreset[]>("get_known_encoders"),
  refreshEncoderAvailability: () =>
    invoke<EncoderAvailability>("refresh_encoder_availability"),
  getArchiverStatus: () =>
    invoke<ArchiverStatus>("get_archiver_status"),
  getCapturePerf: () =>
    invoke<CapturePerfSnapshot>("get_capture_perf"),
  getDiskStatus: () =>
    invoke<DiskStatus>("get_disk_status"),
  archiveHistoryNow: () =>
    invoke<[number, number, number]>("archive_history_now"),
  transcodeArchives: (target_codec: string) =>
    invoke<[number, number]>("transcode_archives", { targetCodec: target_codec }),

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
  restartApp: () => invoke<void>("restart_app"),
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
