# ScreenRecall

**Screen Recall** is the working title for this project; the name/branding **ScreenRecall** here is provisional and subject to change.

A private, local-first **screen recall** desktop app, similar in spirit to Windows Recall or [Windrecorder](https://github.com/yuka-friends/Windrecorder), but offline-first, cross-platform, and built on [Tauri 2](https://tauri.app/).

ScreenRecall periodically captures your monitors, OCRs and embeds content locally, and lets you:

- scrub a **timeline** of your day,
- **search** your history with full-text + semantic search,
- **chat with your history** via a local LLM (Ollama) or any OpenAI-compatible endpoint.

Nothing leaves your machine unless you point it at a remote LLM yourself.

> Status: early / v0.1. Core pipeline works end-to-end on Windows + X11 Linux; macOS needs the usual Screen Recording permission.

---

## Features

- **Cross-platform capture** via `[xcap](https://crates.io/crates/xcap)` (Windows, macOS, Linux X11).
- **Smart diffing**: perceptual hash (dHash) skips unchanged frames so idle time costs little.
- **Private storage**: WebP frames under `~/.screenrecall/frames/` and SQLite (`~/.screenrecall/screenrecall.db`) with FTS5 + embeddings.
- **Pluggable OCR**: Tesseract by default; other engines available in Settings.
- **BYOK LLM**: Ollama by default, or any OpenAI-compatible API (including local `llama-server`).
- **Privacy controls**: per-process / per-window exclude list, pause, tray integration.

---

## Roadmap

- Linux release
- macOS release
- First-run wizard polish
- App polish
- Server to store frames from multiple devices with search and chat across devices
- Explore encryption

---

## One-shot setup (Windows)

```powershell
powershell -ExecutionPolicy Bypass -File scripts\setup-windows.ps1
```

The script prompts for optional **Tesseract** and **Ollama** installs (skip
Ollama if you only use OpenAI-compatible APIs). See `scripts/README.md` for
flags (`-SkipModels`, `-SkipBuildTools`, `-Tesseract`, `-Ollama`).

## Manual requirements

- **Rust** (stable, 1.77+): [https://rustup.rs](https://rustup.rs)  
- **Node 20+** and **pnpm 9+**  
- **Tauri 2 prerequisites**: [https://v2.tauri.app/start/prerequisites/](https://v2.tauri.app/start/prerequisites/)  
- **Tesseract**: needed for the default offline OCR path (install via your OS
or the Windows setup script prompt). Choose another OCR engine in Settings if
you prefer.  
- **Ollama** (or another OpenAI-compatible host): optional if you route chat
/ embeddings through a remote or alternate local API instead of Ollama.

If you use Ollama locally, typical default models (adjust in Settings):

```sh
ollama pull llama3.2
ollama pull nomic-embed-text
```

## Development

```sh
pnpm install
pnpm --filter desktop tauri dev
```

### Release build

```sh
pnpm --filter desktop tauri build
```

## Project structure

```
screen-recall/
  apps/desktop/
    src/                  React + Vite (Timeline, Search, Chat, Settings)
    src-tauri/            Rust backend (capture, OCR, embed, LLM, SQLite)
```

---

## Privacy

- Capture, OCR, embedding, and retrieval run locally by default.  
- Remote LLM endpoints receive only what you send (queries + retrieved OCR snippets).  
- Data lives under `**~/.screenrecall**` unless you change **Data directory** in Settings. Delete that folder to wipe local history.

---

## Contributing

See `[CONTRIBUTING.md](CONTRIBUTING.md)`.

---

## Security

See `[SECURITY.md](SECURITY.md)`.

---

## License

MIT. See `[LICENSE](LICENSE)`.