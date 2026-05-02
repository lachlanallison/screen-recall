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
    Native,
    Vision,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CloseBehavior {
    Ask,
    Minimize,
    Quit,
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
        }
    }

    pub fn load_or_default(path: &Path, default_data_dir: &Path) -> Result<Self> {
        if path.exists() {
            let text = std::fs::read_to_string(path)?;
            match serde_json::from_str::<AppConfig>(&text) {
                Ok(cfg) => Ok(cfg),
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
