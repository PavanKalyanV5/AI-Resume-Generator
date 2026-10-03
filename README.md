# AI Resume Generator

## Packaging

Build the UI first: `cd apps/web && npm ci && npm run build` (embedded into the binaries).

- **Standalone web app:** `cargo run -p resume-server --release` -> http://127.0.0.1:8787/ (`RESUME_UI_DIR` serves the UI from disk instead; build with `--no-default-features --features embeddings` to skip embedding).
- **Linux desktop (Tauri 2):** `cd apps/desktop/src-tauri && cargo tauri build` (deb/rpm/AppImage in `target/<profile>/bundle`). The server runs in-process on a free 127.0.0.1 port; data lives in the platform app-data dir. `resume-desktop --serve-only` runs headless (`RESUME_DATA_DIR`, `PORT`).
- **Android:** needs a full JDK (17+), SDK 34, NDK 26, `NDK_HOME`/`ANDROID_HOME`/`JAVA_HOME`. `cd apps/desktop/src-tauri && cargo tauri android build --debug --target aarch64 --apk`. No ONNX embeddings (keyword scoring) and no LaTeX: PDFs use the built-in renderer (`resume_core::native_pdf`), also used on desktop whenever `tectonic` is not on PATH or `RESUME_PDF_ENGINE=native`.
