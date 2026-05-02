# scripts/

One-shot **Windows** dev-environment bootstrap. Installs Rust, Node, pnpm,
WebView2, optional Visual Studio Build Tools — and optionally Tesseract OCR
and Ollama (+ model pulls). macOS/Linux scripts were removed until they have
been tested.

```powershell
powershell -ExecutionPolicy Bypass -File scripts\setup-windows.ps1
```

During an interactive terminal session you are prompted whether to install:

- **Tesseract** — recommended if you rely on offline OCR defaults.
- **Ollama** — skip if you only use native / OpenAI-compatible API endpoints.

**Flags**

| Flag | Meaning |
| --- | --- |
| `-SkipModels` | Skip `ollama pull` (only applies if Ollama is installed). |
| `-SkipBuildTools` | Skip Visual Studio Build Tools — use if C++ workload is already installed. |
| `-Tesseract Install` \| `Skip` \| `Ask` | Default `Ask`. `Skip` / `Install` avoid prompts. |
| `-Ollama Install` \| `Skip` \| `Ask` | Default `Ask`. `Skip` / `Install` avoid prompts. |

If stdin is redirected or `CI` / `GITHUB_ACTIONS` is set, prompts are skipped:
Tesseract and Ollama are not installed unless you pass `-Tesseract Install` or
`-Ollama Install`.

## What may end up installed

- **Rust** (latest stable via rustup)
- **Node.js** LTS
- **pnpm** (latest)
- **Git** (if missing)
- **Tauri 2 on Windows**: WebView2; optional VS 2022 Build Tools (C++ workload)
- **Tesseract OCR** (optional)
- **Ollama** + `llama3.2` + `nomic-embed-text` (optional; models skipped with `-SkipModels`)

After the script finishes, close and reopen your terminal so `PATH` updates,
then:

```sh
cd screen-recall
pnpm install
pnpm --filter desktop tauri dev
```
