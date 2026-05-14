use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    /// Interval between capture attempts, in seconds.
    pub capture_interval_secs: u64,
    /// How long to keep frames (0 = forever).
    pub retention_days: u64,
    /// Where to store frames + the SQLite DB.
    pub data_dir: PathBuf,
    /// Process executable names to skip (e.g. "1Password.exe").
    pub excluded_processes: Vec<String>,
    /// Substrings that, if present in the active window title, skip capture.
    pub excluded_window_substrings: Vec<String>,

    /// Which LLM/embedding backend to use.
    pub llm_backend: LlmBackend,
    pub ollama_url: String,
    pub openai_base_url: String,
    /// If set, embeddings use this OpenAI-compatible base (e.g. second `llama-server` on another
    /// port). Chat still uses `openai_base_url`.
    #[serde(default)]
    pub openai_embedding_base_url: Option<String>,
    pub openai_api_key: String,

    /// Chat model name (e.g. "llama3.2", "gpt-4o-mini").
    pub chat_model: String,
    /// Optional override for the assistant system prompt used in Chat (defaults to ScreenRecall's built-in prompt).
    #[serde(default)]
    pub chat_system_prompt: Option<String>,
    /// Embedding model name (e.g. "nomic-embed-text", "text-embedding-3-small").
    pub embed_model: String,
    /// Optional vision model for describing frames (Ollama "llava" / "moondream").
    pub vision_model: Option<String>,
    /// Optional one-line shell command to launch a local chat server (e.g. llama.cpp on :8080).
    #[serde(default)]
    pub managed_chat_server_command: Option<String>,
    /// Optional one-line shell command to launch a local embeddings server (e.g. llama.cpp on :8081).
    #[serde(default)]
    pub managed_embed_server_command: Option<String>,
    /// Optional working directory for managed server commands.
    #[serde(default)]
    pub managed_server_working_dir: Option<String>,
    /// Auto-start managed chat server command on app launch.
    #[serde(default)]
    pub managed_chat_server_autostart: bool,
    /// Auto-start managed embedding server command on app launch.
    #[serde(default)]
    pub managed_embed_server_autostart: bool,

    /// OCR engine choice.
    pub ocr_engine: OcrEngineKind,
    /// Whether first-run dependency setup has been completed.
    pub setup_complete: bool,

    /// How often the Timeline UI reloads the frame list from disk (0 = no auto-refresh).
    #[serde(default = "default_timeline_refresh_secs")]
    pub timeline_refresh_secs: u64,

    /// When true, skip screen capture while the Windows session is locked (Win+L).
    #[serde(default = "default_true")]
    pub pause_when_workstation_locked: bool,

    /// Behavior when clicking the main window close button.
    #[serde(default = "default_close_behavior")]
    pub close_behavior: CloseBehavior,

    /// Whether video archiving is enabled at all.
    #[serde(default = "default_archive_enabled")]
    pub archive_enabled: bool,
    /// FFmpeg encoder preset for video archival (e.g. "h264", "h265", "av1_qsv").
    #[serde(default = "default_archive_codec")]
    pub archive_codec: String,
    /// Seconds between archival runs.
    #[serde(default = "default_archive_interval_secs")]
    pub archive_interval_secs: u64,
    /// Duration of each video segment in seconds.
    #[serde(default = "default_archive_segment_secs")]
    pub archive_segment_secs: u64,
    /// How many days to keep individual frame files after they are archived to video.
    /// 0 = keep forever. 1 = delete frames archived more than 24 hours ago.
    #[serde(default = "default_archive_keep_frames_days")]
    pub archive_keep_frames_days: u64,
    /// Max lookback in seconds for the automatic archiver. Prevents backfilling
    /// large history on startup. 0 = unlimited (not recommended).
    #[serde(default = "default_archive_max_lookback_secs")]
    pub archive_max_lookback_secs: u64,

    /// Max long edge for stored frames (0 = disable downscaling, keep full resolution).
    #[serde(default = "default_capture_downscale_max_edge")]
    pub capture_downscale_max_edge: u32,
    /// Image format for stored frames.
    #[serde(default = "default_capture_image_format")]
    pub capture_image_format: CaptureImageFormat,
    /// JPEG quality (1–100). Only used when format is JPEG.
    #[serde(default = "default_capture_jpeg_quality")]
    pub capture_jpeg_quality: u8,
    /// Resize filter for frame downscaling. "nearest" (default, fastest) or "lanczos3" (better OCR accuracy).
    #[serde(default = "default_capture_resize_filter")]
    pub capture_resize_filter: CaptureResizeFilter,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LlmBackend {
    Ollama,
    Openai,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OcrEngineKind {
    Tesseract,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CaptureImageFormat {
    Webp,
    Jpeg,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CloseBehavior {
    Ask,
    Minimize,
    Quit,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CaptureResizeFilter {
    Nearest,
    Lanczos3,
}

fn default_timeline_refresh_secs() -> u64 {
    5
}

fn default_true() -> bool {
    true
}

fn default_close_behavior() -> CloseBehavior {
    CloseBehavior::Ask
}

fn default_archive_enabled() -> bool {
    true
}

fn default_archive_codec() -> String {
    "h264".to_string()
}

fn default_archive_interval_secs() -> u64 {
    900 // 15 minutes
}

fn default_archive_segment_secs() -> u64 {
    900 // 15 minutes per segment
}

fn default_archive_keep_frames_days() -> u64 {
    1 // keep frames for 1 day after archiving by default
}

fn default_archive_max_lookback_secs() -> u64 {
    0 // unlimited — archiver catches up from last successful run by default
}

fn default_capture_downscale_max_edge() -> u32 {
    1920
}

fn default_capture_image_format() -> CaptureImageFormat {
    CaptureImageFormat::Jpeg
}

fn default_capture_jpeg_quality() -> u8 {
    85
}

fn default_capture_resize_filter() -> CaptureResizeFilter {
    CaptureResizeFilter::Nearest
}

impl AppConfig {
    pub fn default_for(data_dir: PathBuf) -> Self {
        Self {
            capture_interval_secs: 3,
            retention_days: 30,
            data_dir,
            excluded_processes: vec![],
            excluded_window_substrings: vec!["Incognito".into(), "Private Browsing".into()],
            llm_backend: LlmBackend::Ollama,
            ollama_url: "http://localhost:11434".into(),
            openai_base_url: "https://api.openai.com/v1".into(),
            openai_embedding_base_url: None,
            openai_api_key: String::new(),
            chat_model: "llama3.2".into(),
            chat_system_prompt: None,
            embed_model: "nomic-embed-text".into(),
            vision_model: None,
            managed_chat_server_command: None,
            managed_embed_server_command: None,
            managed_server_working_dir: None,
            managed_chat_server_autostart: false,
            managed_embed_server_autostart: false,
            ocr_engine: OcrEngineKind::Tesseract,
            setup_complete: false,
            timeline_refresh_secs: default_timeline_refresh_secs(),
            pause_when_workstation_locked: true,
            close_behavior: default_close_behavior(),
            archive_enabled: default_archive_enabled(),
            archive_codec: default_archive_codec(),
            archive_interval_secs: default_archive_interval_secs(),
            archive_segment_secs: default_archive_segment_secs(),
            archive_keep_frames_days: default_archive_keep_frames_days(),
            archive_max_lookback_secs: default_archive_max_lookback_secs(),
            capture_downscale_max_edge: default_capture_downscale_max_edge(),
            capture_image_format: default_capture_image_format(),
            capture_jpeg_quality: default_capture_jpeg_quality(),
            capture_resize_filter: default_capture_resize_filter(),
        }
    }

    pub fn load_or_default(path: &Path, default_data_dir: &Path) -> Result<Self> {
        if path.exists() {
            let text = std::fs::read_to_string(path)?;
            match serde_json::from_str::<AppConfig>(&text) {
                Ok(mut cfg) => {
                    // Migration: older configs may not have archive_enabled.
                    // If archive_interval_secs was > 0, the user had archiving on.
                    if !text.contains("archive_enabled") {
                        cfg.archive_enabled = cfg.archive_interval_secs > 0;
                    }
                    // Migration: archive_delete_source (bool) → archive_keep_frames_days (u64).
                    if text.contains("archive_delete_source") {
                        // If the old field was true, default to 1 day. If false, 0 (keep forever).
                        cfg.archive_keep_frames_days = if text.contains("\"archive_delete_source\": true") { 1 } else { 0 };
                    }
                    Ok(cfg)
                }
                Err(err) => {
                    tracing::warn!(?err, "failed to parse config, using defaults");
                    Ok(AppConfig::default_for(default_data_dir.to_path_buf()))
                }
            }
        } else {
            Ok(AppConfig::default_for(default_data_dir.to_path_buf()))
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }
}
