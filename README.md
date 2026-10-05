# AI Resume Generator

Local-first AI resume tailoring. You keep one master resume; for each job description (JD) the app picks the most relevant content, asks an LLM to reword it, and renders a one- or two-page PDF and DOCX.

- **Private by construction.** Names, emails, phones, employers and clients are replaced by placeholders (`[ORG_1]`) before anything leaves the machine. A guard blocks the request if any real value is still present. Real values are put back locally. See [docs/privacy-and-security.md](docs/privacy-and-security.md).
- **Honest.** Every AI rewrite is checked against facts derived from your own resume (numbers, technologies, years, employers). Rewrites that claim something your resume does not support are reverted, or repaired with one bounded retry.
- **Learns from you.** Your edits and add/remove decisions become labelled corrections. They feed small local models and a rule "playbook" you approve. No fine-tuning, no data leaves the machine. See [docs/learning-system.md](docs/learning-system.md).
- **Runs where you are.** One Rust server, embedded web UI, a Tauri desktop app and an Android companion that talks to your desktop over pinned TLS.

Design and diagrams: [docs/architecture.md](docs/architecture.md).

## Features

- Import a resume from `.docx`, `.pdf` or `.json` (deterministic parser, no AI; unplaceable content is reported as warnings).
- JD input by paste or URL fetch (SSRF-guarded).
- Local JD parsing, BM25 plus optional local embeddings (BGE-small, ONNX) for selection, ATS keyword coverage, knowledge graph, gaps, near-duplicate clusters.
- Automatic 1 vs 2 page decision from explainable heuristics (`heuristics.json`), overridable per job.
- Optional AI plan review (projects, certs, roles, skills), with local lint fixes and your overrides winning.
- AI rewrite with grounding guard and one-shot repair, then local restore of real details.
- DOCX and PDF output fitted to the page target. PDF via LaTeX (`tectonic`) when installed, else a built-in renderer.
- Optional upload to your own Google Drive folder (`drive.file` scope, OAuth PKCE).
- Durable job pipeline: SQLite saga with leases, retries, resumable steps, live events (WebSocket with replay, SSE fallback), notifications.
- Library of finished resumes, application funnel tracking, analytics, PII ledger, playbook, learning dashboard.
- Mobile companion: share a JD to the phone app, tailor on your desktop, download the result.
- API keys encrypted at rest (AES-256-GCM).

## Quick start

All targets need the web UI built once (it is embedded into the binaries):

```bash
cd apps/web && npm ci && npm run build
```

### Server binary (browser)

```bash
cargo run -p resume-server --release          # http://127.0.0.1:8787/
# or with explicit settings
RESUME_DB=$HOME/resume/app.db PORT=8787 RESUME_MODEL_DIR=$HOME/resume/models \
  ANTHROPIC_API_KEY=... cargo run -p resume-server --release
```

- `RESUME_DB` (default `data/app.db`) also decides where `master.key`, `heuristics.json`, `remote-cert.pem` and `remote-key.pem` live (the same directory). Generated files go to `data/out/<job-id>/` relative to the working directory (not configurable).
- `RESUME_UI_DIR=apps/web/dist` serves the UI from disk instead of the embedded copy. Build with `--no-default-features --features embeddings` to skip embedding the UI.
- The server listens on 127.0.0.1 only. The optional phone listener (port 8788) is separate and off until you enable it in Settings.
- Add an API key in Settings (stored encrypted) or via env (`ANTHROPIC_API_KEY`, `GEMINI_API_KEY`). The `mock` provider needs no key and is for trying the pipeline offline.

### Desktop app (Linux, Tauri 2)

Install a bundle from `release-artifacts/` (not tracked in git) or build one (see below):

```bash
chmod +x "Resume Generator_0.1.0_amd64.AppImage" && ./"Resume Generator_0.1.0_amd64.AppImage"
sudo dnf install ./"Resume Generator-0.1.0-1.x86_64.rpm"      # Fedora/RHEL
sudo apt install ./"Resume Generator_0.1.0_amd64.deb"         # Debian/Ubuntu
```

The server runs in-process on a free 127.0.0.1 port and the window points at it. Data lives in the platform app-data dir (`app.db`, `out/`, `models/`). Headless: `resume-desktop --serve-only` (`RESUME_DATA_DIR`, `PORT`). A second launch just focuses the existing window.

### Android companion

The APK is the same Tauri shell but runs only the companion proxy; the real work happens on your desktop.

1. On the desktop, start the app or server and open Settings, Mobile card. Turn remote access on.
2. Allow the port through the firewall (Fedora): `sudo firewall-cmd --add-port=8788/tcp` (add `--permanent` to keep it; change the number if you set `REMOTE_PORT`).
3. Phone and desktop must be on the same LAN, or both on Tailscale (the app advertises LAN addresses and Tailscale addresses in the CGNAT range 100.64.0.0/10). Guest Wi-Fi with client isolation will not work.
4. Install the APK (`adb install ResumeCompanion.apk` or open it on the phone; allow installs from the source).
5. Click "New pairing code" on the desktop (single use, valid 5 minutes). Scan the QR in the phone app (camera permission) or type host, port, fingerprint and code.
6. The phone pins the desktop certificate by SHA-256 fingerprint and stores a device token. Revoke devices or rotate the certificate (revokes everyone) from the same card. If the desktop's IP set changes, the card shows "certificate stale": rotate and re-pair.

Sharing a JD from another Android app to this one opens it in the intake page.

## Configuration

Everything is optional. Defaults in the second column.

| Variable | Default | Used by | Meaning |
|---|---|---|---|
| `RESUME_DB` | `data/app.db` | server bin | SQLite file. Sibling files (`master.key`, `heuristics.json`, remote cert/key, `companion.json`) live beside it. |
| `PORT` | `8787` server, random on desktop | server, desktop | Local HTTP port (127.0.0.1). Also used in the Google OAuth redirect URI `http://127.0.0.1:$PORT/api/drive/callback` (defaults to 8787 there). |
| `RESUME_UI_DIR` | embedded | `resume-ui` | Serve the web UI from this directory. |
| `RESUME_MODEL_DIR` | `data/models`; desktop sets `<app-data>/models` | core | Cache for the ONNX embedding model (downloaded on first use; falls back to keyword scoring if unavailable). |
| `RESUME_DATA_DIR` | `resume-data` | desktop `--serve-only` | Data dir for the headless desktop server. |
| `RESUME_MASTER_KEY_FILE` | `master.key` beside the DB | server | 32-byte AES key file (created mode 0600; refused if group/other readable). |
| `RESUME_HEURISTICS` | `heuristics.json` beside the DB | server | Partial JSON overrides for page and sizing heuristics. |
| `ANTHROPIC_API_KEY` | none | server | Anthropic key (a key stored in Settings wins). |
| `GEMINI_API_KEY` / `GOOGLE_API_KEY` | none | server | Gemini key, first one found. |
| `RESUME_ANTHROPIC_MODEL` | `claude-haiku-4-5-20251001` | server | Model id. |
| `RESUME_GEMINI_MODEL` | `gemini-flash-latest` | server | Model id. Set this if you see a model 404. |
| `RESUME_ANTHROPIC_URL`, `RESUME_GEMINI_URL` | vendor URLs | server | Base URL overrides (proxies, tests). |
| `GOOGLE_OAUTH_CLIENT_ID` / `GOOGLE_OAUTH_CLIENT_SECRET` | none | server | OAuth desktop client for Drive (or paste them in Settings; stored encrypted). |
| `RESUME_GOOGLE_AUTH_URL`, `_TOKEN_URL`, `_API_URL`, `_UPLOAD_URL`, `_REVOKE_URL` | Google endpoints | server | Test overrides. |
| `REMOTE_PORT` | `8788` | server | Phone listener (TLS, binds 0.0.0.0). |
| `RESUME_PDF_ENGINE` | auto | core | `native` forces the built-in PDF renderer even if `tectonic` is on PATH. |
| `RESUME_ALLOW_PRIVATE_FETCH` | unset | server | `1` disables the SSRF guard on JD fetch. Tests only; never set it. |

## How a job runs

Submitting a JD creates a row in SQLite and ten steps run in order on one of two background workers: `parse_jd` (local) -> `select` (local: BM25/embeddings, ATS coverage, page decision, playbook rules) -> `ai_review` (optional, sends a redacted plan summary) -> `build_payload` (local: redact, leak guard) -> `ai_tailor` (sends the redacted payload) -> `restore` (local: parse reply, grounding guard, optional one-line-batch repair call, put real values back) -> `render_docx` and `render_pdf` (local, fitted to the page target) -> `upload_drive` (only if requested) -> `finalize`. Each step stores its output and an input hash, so a crash or restart resumes at the first unfinished step and a finished AI reply is never paid for twice. Details: [docs/architecture.md](docs/architecture.md#3-job-saga).

## Repository layout

```
.
├── Cargo.toml                  workspace (core, server, ui, companion, desktop)
├── crates/
│   ├── core/                   pure logic, no network: schema, redact, jd, select, arrange, graph, profile_model,
│   │   │                       grounding, payload, heuristics, fit, latex, native_pdf, docx, import, review,
│   │   │                       playbook, emphasis, embed, seniority, cluster
│   │   ├── src/ml/             small local models (ranker, rewrite_need, jd_classifier, fit_model, outcome,
│   │   │                       pii_candidate, recommend, neighbors, registry)
│   │   ├── src/bin/            render, pipeline_check, import_check, vault_audit (dev tools)
│   │   ├── templates/          LaTeX preamble
│   │   └── data/skills.txt     skill taxonomy
│   ├── server/                 axum API + job engine (lib.rs), providers, drive, fetch, keys, remote, realtime,
│   │                           learn, review, mlsvc, pii, heur; tests/ integration tests
│   ├── ui/                     serves the built web UI (embedded or $RESUME_UI_DIR)
│   └── companion/              phone-side pinned-TLS proxy
├── apps/
│   ├── web/                    React + Vite + TypeScript UI
│   └── desktop/src-tauri/      Tauri 2 shell (desktop: full server; Android: companion only)
├── docs/                       architecture, privacy-and-security, learning-system
├── scripts/                    export-ts.mjs (one-off importer), pii-check.py (pre-commit PII literal check)
├── data/  artifacts/  out/  release-artifacts/   local only, gitignored (may hold private data)
```

## Developer setup

Prerequisites: Rust (rustup, stable), Node 20+ with npm, `cargo install tauri-cli --version "^2"`, a C/C++ toolchain (`gcc`, `gcc-c++`; the ONNX runtime needs libstdc++). Optional: `tectonic` for LaTeX PDFs.

For Android: JDK 21, Android SDK (platform 34, build-tools) and NDK, plus `rustup target add aarch64-linux-android`, and `ANDROID_HOME`, `NDK_HOME`, `JAVA_HOME` exported.

Linking note: if the linker cannot find `libstdc++` (error mentioning `-lstdc++`), install `gcc-c++` (Fedora) / `g++` (Debian) and, if it is still not found, point the linker at it:

```bash
export LIBRARY_PATH="$(dirname "$(gcc -print-file-name=libstdc++.so)"):$LIBRARY_PATH"
```

Build and run:

```bash
# web (dev server on :5173 proxies /api to 127.0.0.1:8787)
cd apps/web && npm ci && npm run dev
npm run build                                   # tsc --noEmit && vite build -> apps/web/dist

# server (run from repo root so data/ is local)
cargo run -p resume-server                      # debug
cargo run -p resume-server --release

# desktop bundles: deb, rpm, AppImage in target/<profile>/bundle
cd apps/desktop/src-tauri && cargo tauri build

# android (needs the env vars above)
cd apps/desktop/src-tauri && cargo tauri android build --target aarch64 --apk
```

Signing an Android APK (unsigned release APKs do not install). Use your own keystore, never commit it:

```bash
BT=$ANDROID_HOME/build-tools/<version>
$BT/zipalign -p -f 4 app-unsigned.apk app-aligned.apk
$BT/apksigner sign --ks my-release.jks --out ResumeCompanion.apk app-aligned.apk
$BT/apksigner verify ResumeCompanion.apk
```

The APK produced by Gradle is under `apps/desktop/src-tauri/gen/android/app/build/outputs/apk/`. A `--debug` build is signed with the debug key and installs as is.

Tests:

```bash
cargo test --workspace
RESUME_PDF_ENGINE=native cargo test --workspace   # recommended: avoids slow tectonic renders
```

Integration tests live in `crates/server/tests/` (saga crash/resume faults, redaction and fit, SSRF, remote pairing, realtime, learning, ML, vendors with mock HTTP servers) and `crates/companion/tests/proxy.rs`. The tests that render PDFs are slow with LaTeX; the native engine is fast and what Android uses.

Optional pre-commit hook: `scripts/pii-check.py` blocks commits whose staged content contains any literal from a local rules file (`~/.claude/redaction/redact-rules.json`); it prints file names and counts, never the values.

## Troubleshooting

- **Blank page.** Open the browser console (F12). Most often the UI was not built (`web UI not built` message: run `npm run build` in `apps/web`), or a stale `dist` is embedded; rebuild and `cargo build` again. With `RESUME_UI_DIR`, check the path.
- **UI shows demo data.** The UI falls back to an in-browser demo when `/api/health` fails. Check the server is running and the port matches.
- **Model 404 / "AI model is no longer available".** The default model was retired. Set `RESUME_GEMINI_MODEL` or `RESUME_ANTHROPIC_MODEL` to a current model id and restart.
- **"no API key configured".** Add a key in Settings or set the env var, then retry from the failed step.
- **"Blocked: private details would have been sent".** The leak guard stopped a request; nothing was sent. Add the value in the PII page (always-redact) and run again.
- **Connect Google Drive.** Create an OAuth client of type "Desktop app" in Google Cloud, enable the Drive API, put the client id/secret in Settings (or env), make sure `http://127.0.0.1:<PORT>/api/drive/callback` is an allowed redirect URI (default port 8787; on the desktop app set `PORT` so it is not random), click Connect, approve, and if Google returns no refresh token remove the app under your Google account permissions and retry. Uploads are idempotent per job and file kind.
- **Phone cannot pair.** Same LAN or Tailscale; open the firewall (`sudo firewall-cmd --add-port=8788/tcp`); try each advertised address (the phone tries them all). VPN clients that route all traffic or isolate you from the LAN will block it; disconnect the VPN or use its LAN-bypass. "Certificate does not match" means the desktop certificate was rotated: pair again.
- **No semantic matching.** The embedding model downloads on first start; if offline you get keyword scoring and a notification. Set `RESUME_MODEL_DIR` to a writable path.
- **PDF looks different from expected.** Without `tectonic` the built-in renderer is used (a banner in the job events says which). Install tectonic for LaTeX typesetting.
- **Stuck job after a crash.** Leases expire in about 30 seconds; the sweeper requeues the job and it resumes at the first unfinished step. All running jobs are requeued at startup.

## Security notes

Redaction is fail-closed and reversible only in memory; secrets are encrypted at rest; the phone listener is opt-in, pinned-TLS, default-deny and cannot reach PII or key endpoints; admin endpoints answer only to localhost. Residual risks (for example, schools, locations and project names are not redacted unless you add them) are listed in [docs/privacy-and-security.md](docs/privacy-and-security.md#residual-risks).

## License

To be decided. Placeholder: all rights reserved until a license file is added.

## Known limitations

- **Desktop app and the phone listener:** the Tauri shell does not call `remote::autostart`, so after restarting the desktop app, re-enable remote access in Settings → Mobile companion before pairing or using the phone. The standalone `resume-server` binary does restore it at boot.
- **Desktop app and Google Drive connect:** the desktop app listens on a random local port, but the OAuth redirect URI assumes `PORT` (default 8787). Connect Drive once from the standalone server (or run the desktop app with a fixed `PORT`); the stored refresh token is then reused for uploads, so you only hit this when re-authorising.
- **On-device paths untested:** the Android share-sheet hand-off, QR scan and the pairing flow have been exercised only against local test servers, not on a physical phone.
- **Tests:** the server tests default to the built-in PDF renderer for speed; only two tests exercise real LaTeX (`tectonic`).
- **Generated resumes cover one JD at a time:** the learning system needs a handful of approved resumes and corrections before its rules and models show a measurable effect.

## Building on Windows

Windows builds must be made on Windows (Tauri cannot cross-compile to it from Linux). Untested so far: the Unix-only file-permission and `/dev/urandom` code was made portable, and everything still compiles on Linux, but no Windows build has been run.

1. Install Git, Node 20+, Rust (rustup, MSVC toolchain) and the Visual Studio Build Tools "Desktop development with C++" workload. Windows 11 ships the WebView2 runtime.
2. `cd apps\web; npm install; npm run build`
3. `cargo install tauri-cli --version "^2" --locked`, then in `apps\desktop\src-tauri` run `cargo tauri build`. Installers appear under `target\release\bundle`.
4. Without `tectonic.exe` on PATH the built-in PDF renderer is used. Set `GEMINI_API_KEY` as a user environment variable or paste the key in Settings.
5. To migrate data, copy `app.db` and `master.key` together from `~/.local/share/dev.resumegen.desktop/` into `%APPDATA%\dev.resumegen.desktop\`.

The Rust tests that assert Unix file modes (`tests/vendors.rs`, `tests/remote.rs`, companion `tests/proxy.rs`) are Unix-only and do not compile on Windows; run `cargo build`, not `cargo test`, there.
