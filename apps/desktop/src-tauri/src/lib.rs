//! ScreenRecall — private, local-first screen recall.
//!
//! The `run()` function here is the single entry point used by the binary.
//! It wires up Tauri, the SQLite store, the capture / OCR / embedding
//! workers, the tray menu and the command surface.

mod archive;
mod capture;
mod capture_perf;
mod config;
mod embed;
mod llm;
mod ocr;
mod search;
mod state;
mod store;
mod util;
mod worker_activity;

use util::workstation_lock;

pub mod commands;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tauri::{
    menu::{Menu, MenuEvent, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, RunEvent,
};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::state::{AppState, OcrQueue};
use crate::util::process_log;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Set up tracing first so early errors are visible.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "screenrecall=info,screenrecall_lib=info".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();

            // Build state (loads config, opens DB, creates data dir).
            let state =
                tauri::async_runtime::block_on(async { AppState::new(handle.clone()).await })
                    .expect("failed to initialize ScreenRecall state");
            let state = Arc::new(state);
            app.manage(state.clone());

            // Background refresh of on-disk stats so UI stays non-blocking.
            {
                let state = state.clone();
                tauri::async_runtime::spawn(async move {
                    async fn refresh_stats(state: &Arc<AppState>) {
                        let frames_dir = state.frames_dir();
                        let bytes = tokio::task::spawn_blocking({
                            let p = frames_dir.clone();
                            move || dir_size_bytes(&p)
                        })
                        .await
                        .unwrap_or(0);
                        state.set_cached_disk_bytes(bytes);
                        if let Ok((frames, indexed)) = state.store.stats() {
                            state.set_cached_counts(frames, indexed);
                        }
                        if let Ok(archived) = state.store.archived_frame_count() {
                            state.set_cached_archived_count(archived);
                        }
                        if let Ok(unarchived) = state.store.unarchived_frame_disk_bytes() {
                            state.set_cached_unarchived_bytes(unarchived);
                        }
                        if let Ok(unarchived_count) = state.store.unarchived_frame_count() {
                            state.set_cached_unarchived_count(unarchived_count);
                        }
                        let _ = state.store.wal_checkpoint_passive();
                    }

                    // Run once immediately so stats aren't 0 on first paint.
                    refresh_stats(&state).await;

                    loop {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        refresh_stats(&state).await;
                    }
                });
            }

            // Background disk-space monitor: warn at <10%, stop recording at <5%.
            {
                let state = state.clone();
                let app = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    use std::sync::atomic::AtomicBool;
                    let warned: AtomicBool = AtomicBool::new(false);

                    async fn check_disk(
                        state: &Arc<AppState>,
                        app: &tauri::AppHandle,
                        warned: &AtomicBool,
                    ) {
                        let data_dir = state.config().data_dir.clone();
                        let pct = tokio::task::spawn_blocking(move || {
                            crate::util::disk::free_space_pct(&data_dir)
                        })
                        .await
                        .unwrap_or(None);

                        if let Some(pct) = pct {
                            let warning = pct < 10.0;
                            let stopped = pct < 5.0;
                            let was_stopped = state.disk_stopped();
                            let was_warning = state.disk_warning();
                            state.set_disk_status(warning, stopped, pct);

                            if stopped && !was_stopped {
                                state.set_recording(false);
                                warn!(free_pct = pct, "disk critically low — recording paused");
                                let _ = app.emit(
                                    "screenrecall:disk-status",
                                    serde_json::json!({
                                        "warning": true,
                                        "stopped": true,
                                        "freePct": pct,
                                    }),
                                );
                            } else if warning && !was_warning && !stopped {
                                if !warned.load(Ordering::Relaxed) {
                                    warned.store(true, Ordering::Relaxed);
                                    warn!(free_pct = pct, "disk space low — warning issued");
                                    let _ = app.emit(
                                        "screenrecall:disk-status",
                                        serde_json::json!({
                                            "warning": true,
                                            "stopped": false,
                                            "freePct": pct,
                                        }),
                                    );
                                }
                            } else if !warning && !stopped {
                                warned.store(false, Ordering::Relaxed);
                            }
                        }
                    }

                    check_disk(&state, &app, &warned).await;

                    loop {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        check_disk(&state, &app, &warned).await;
                    }
                });
            }

            // Optional managed local llama.cpp auto-start (chat/embed).
            // Only start when the backend is OpenAI-compatible; Ollama backend uses the
            // system Ollama service instead of managed llama-server processes.
            {
                let state = state.clone();
                tauri::async_runtime::spawn(async move {
                    let cfg = state.config();
                    let use_managed = matches!(cfg.llm_backend, config::LlmBackend::Openai);
                    if !use_managed {
                        return;
                    }
                    let cwd = cfg.managed_server_working_dir.clone();

                    for (kind, command, autostart) in [
                        (
                            "chat",
                            cfg.managed_chat_server_command.as_deref(),
                            cfg.managed_chat_server_autostart,
                        ),
                        (
                            "embed",
                            cfg.managed_embed_server_command.as_deref(),
                            cfg.managed_embed_server_autostart,
                        ),
                    ] {
                        if !autostart {
                            continue;
                        }
                        let Some(cmd) = command.filter(|s| !s.trim().is_empty()) else {
                            continue;
                        };
                        let out_dir = state.config().data_dir.join("managed-llama");
                        let stdout_path = out_dir.join(format!("{kind}.stdout.log"));
                        let stderr_path = out_dir.join(format!("{kind}.stderr.log"));
                        process_log::record(
                            &state,
                            "managed_llama_autostart",
                            "custom_command",
                            &[cmd.to_string()],
                            cwd.as_deref(),
                        );
                        match crate::state::shell_spawn(
                            cmd,
                            cwd.as_deref(),
                            &stdout_path,
                            &stderr_path,
                        ) {
                            Ok(child) => {
                                state.managed_llama.lock().await.insert(
                                    kind.to_string(),
                                    crate::state::ManagedLlamaProcess {
                                        command: cmd.to_string(),
                                        cwd: cwd.clone(),
                                        started_at: std::time::Instant::now(),
                                        stdout_path: stdout_path.to_string_lossy().to_string(),
                                        stderr_path: stderr_path.to_string_lossy().to_string(),
                                        child,
                                    },
                                );
                                info!("managed {kind} server auto-started");
                            }
                            Err(err) => warn!(?err, "managed {kind} server auto-start failed"),
                        }
                    }
                });
            }

            // Ensure the main window gets the app icon in dev/prod so Windows taskbar uses it.
            if let (Some(win), Some(icon)) = (
                app.get_webview_window("main"),
                app.default_window_icon().cloned(),
            ) {
                let _ = win.set_icon(icon);
            }

            let (ocr_tx, ocr_rx) = mpsc::unbounded_channel::<i64>();
            app.manage(OcrQueue(ocr_tx.clone()));

            workstation_lock::spawn_watcher(state.clone());

            // Build tray menu.
            let pause_item =
                MenuItem::with_id(app, "pause", "Pause recording", true, None::<&str>)?;
            let resume_item =
                MenuItem::with_id(app, "resume", "Resume recording", true, None::<&str>)?;
            let open_item =
                MenuItem::with_id(app, "open", "Open ScreenRecall", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open_item, &pause_item, &resume_item, &quit_item])?;

            let _tray = TrayIconBuilder::with_id("tray")
                .icon(
                    app.default_window_icon()
                        .cloned()
                        .expect("default window icon missing"),
                )
                .tooltip("ScreenRecall")
                .menu(&menu)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::DoubleClick { button, .. } = event {
                        if button == MouseButton::Left {
                            let app = tray.app_handle();
                            if let Some(win) = app.get_webview_window("main") {
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                        }
                    }
                })
                .on_menu_event(move |app, event: MenuEvent| {
                    let state: tauri::State<'_, Arc<AppState>> = app.state();
                    match event.id.as_ref() {
                        "pause" => {
                            state.set_recording(false);
                        }
                        "resume" => {
                            state.set_recording(true);
                        }
                        "open" => {
                            if let Some(win) = app.get_webview_window("main") {
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            // Channel from capture -> workers.
            let (tx, rx) = mpsc::unbounded_channel::<i64>();

            // Spin up capture scheduler.
            {
                let state = state.clone();
                let tx = tx.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = capture::run_scheduler(state, tx).await {
                        warn!(?err, "capture scheduler exited with error");
                    }
                });
            }

            // OCR + embed workers (`ocr_tx` is also `manage`d for `requeue_ocr_rerun`).
            {
                let state = state.clone();
                // Fan-out: every new frame id goes to both OCR and embed.
                let (embed_tx, embed_rx) = mpsc::unbounded_channel::<i64>();
                tauri::async_runtime::spawn(fanout(rx, ocr_tx, embed_tx));

                tauri::async_runtime::spawn({
                    let state = state.clone();
                    async move {
                        if let Err(err) = ocr::run_worker(state, ocr_rx).await {
                            warn!(?err, "OCR worker exited");
                        }
                    }
                });
                tauri::async_runtime::spawn({
                    let state = state.clone();
                    async move {
                        if let Err(err) = embed::run_worker(state, embed_rx).await {
                            warn!(?err, "embedding worker exited");
                        }
                    }
                });
                tauri::async_runtime::spawn({
                    let state = state.clone();
                    async move {
                        if let Err(err) = archive::run_worker(state).await {
                            warn!(?err, "archiver worker exited");
                        }
                    }
                });
            }

            info!("ScreenRecall initialized; capture + OCR + embedding + archiver workers running");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::set_recording,
            commands::get_config,
            commands::set_config,
            commands::get_managed_llama_status,
            commands::start_managed_llama,
            commands::start_managed_llama_both,
            commands::stop_managed_llama,
            commands::get_managed_llama_log_tail,
            commands::test_ollama_connection,
            commands::test_openai_chat_connection,
            commands::test_openai_embed_connection,
            commands::load_chat_ui_state,
            commands::save_chat_ui_state,
            commands::check_dependencies,
            commands::install_tesseract,
            commands::install_ollama,
            commands::install_ffmpeg,
            commands::ensure_ffmpeg_path,
            commands::pull_model,
            commands::complete_setup,
            commands::get_stats,
            commands::get_health_snapshot,
            commands::get_worker_queue_snapshot,
            commands::get_capture_perf,
            commands::get_disk_status,
            commands::get_encoder_availability,
            commands::get_known_encoders,
            commands::refresh_encoder_availability,
            commands::get_archiver_status,
            commands::archive_history_now,
            commands::transcode_archives,
            commands::list_frames,
            commands::list_videos,
            commands::get_frame_ocr,
            commands::get_frame_embedding_preview,
            commands::requeue_ocr_rerun,
            commands::requeue_embed_rerun,
            commands::search,
            commands::chat,
            commands::chat_cancel,
            commands::open_data_dir,
            commands::reveal_frame_in_folder,
            commands::delete_all,
            commands::window_minimize_to_tray,
            commands::window_quit_app,
            commands::restart_app,
            commands::get_perf_log_tail,
            commands::get_perf_log_path,
            commands::get_runtime_log_tail,
            commands::get_process_log_tail,
            commands::get_process_log_path,
            commands::get_launch_on_startup_status,
            commands::set_launch_on_startup,
            commands::get_adaptive_state,
        ])
        .build(tauri::generate_context!())
        .expect("error while building ScreenRecall")
        .run(|app, event| {
            // Catch all exit paths (tray quit, window close, app.exit, etc.)
            // and terminate managed llama.cpp child processes.
            if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
                if let Some(state) = app.try_state::<Arc<AppState>>() {
                    let stopped = state.stop_all_managed_llama_blocking();
                    if stopped > 0 {
                        info!(stopped, "stopped managed llama servers on app shutdown");
                    }
                }
            }
        });
}

async fn fanout(
    mut rx: mpsc::UnboundedReceiver<i64>,
    a: mpsc::UnboundedSender<i64>,
    b: mpsc::UnboundedSender<i64>,
) {
    while let Some(id) = rx.recv().await {
        let _ = a.send(id);
        let _ = b.send(id);
    }
}

fn dir_size_bytes(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&p) {
            for entry in rd.flatten() {
                let sub = entry.path();
                if sub.is_dir() {
                    stack.push(sub);
                } else if let Ok(md) = std::fs::metadata(&sub) {
                    if md.is_file() {
                        total += md.len();
                    }
                }
            }
        }
    }
    total
}
