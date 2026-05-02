# Contributing

Thanks for your interest in ScreenRecall!

## Local setup

1. Install the prerequisites in `README.md` (Rust, Node/pnpm, Tauri 2 build deps; add Tesseract / Ollama only if your OCR and LLM setup needs them).
2. `pnpm install` at the repo root.
3. `pnpm --filter desktop tauri dev`.

## Code style

- Rust: `cargo fmt` and `cargo clippy -- -D warnings`.
- TypeScript/React: `pnpm --filter desktop format` (Prettier).
- Prefer small, focused PRs. One feature or fix per branch.

## Commit messages

Conventional Commits are preferred but not required. Short imperative subject
lines are fine.

## Filing issues

Please include:

- Your OS + version
- Ollama version (or which OpenAI-compatible endpoint)
- Steps to reproduce
- Relevant log output (set `RUST_LOG=screenrecall=debug,screenrecall_lib=debug`)

## Security

If you find a security issue, please open a private GitHub security advisory
rather than a public issue.
