# Privacy and security

What the app protects, how, and what it does not. See also: [architecture](architecture.md) (diagrams of the redaction flow and phone pairing), [learning system](learning-system.md), [README](../README.md).

All examples use fake data (Jane Doe, Acme Robotics).

## Threat model

Protected assets: the owner's identity and contact details, employers and end clients, the master resume, API keys, Google Drive credentials, the learning history.

| Threat | Mitigation |
|---|---|
| LLM provider (or anyone reading its logs) learns who you are | Redact before sending; fail-closed leak guard; only redacted, link-free, id-addressed text is sent |
| LLM invents facts that end up on your resume | Grounding guard checks every rewrite against facts derived from your resume |
| Another local user reads secrets from disk | Keys encrypted with AES-256-GCM; master key file mode 0600 and refused if group/other readable; remote TLS key 0600; companion config 0600 |
| Another device on the LAN uses the app | Phone listener is opt-in; TLS with a pinned self-signed cert; one-time pairing code; per-device bearer token (hash stored only); default-deny route allow-list; rate limits; audit log |
| Web page in your browser attacks the local server (CSRF, DNS rebinding) | CORS only for the Vite dev origins; WebSocket `Origin` allow-list; admin routes require a localhost `Host`; confirm headers on destructive/sensitive calls |
| Malicious job URL makes the app hit your network (SSRF) | Public-address-only guard on every redirect hop, DNS result pinned, size and redirect caps |
| Malicious app on the phone abuses the stored token | Companion proxy requires a per-launch secret and loopback Host/Origin |
| Lost or stolen phone | Revoke the device (live streams end) or rotate the certificate (revokes all) from the desktop |
| Supply-chain / committed secrets | `data/`, `artifacts/`, `out/`, `release-artifacts/`, `models/` are gitignored; optional pre-commit hook `scripts/pii-check.py` |

Out of scope: malware running as your user (it can read your files and the master key), a compromised desktop, a malicious LLM provider that infers identity from the remaining content (see [residual risks](#residual-risks)).

## What is redacted

Built by `Vault::from_resume` in `crates/core/src/redact.rs`:

| Kind | Token | Source | Variants also caught |
|---|---|---|---|
| Person | `[PERSON_n]` | `profile.name` | full name and each word of 3+ letters ("Jane", "Doe") |
| Email | `[EMAIL_n]` | `profile.email` | local part when 4+ alphanumerics (`jane.doe`) |
| Phone | `[PHONE_n]` | `profile.phone` | any formatting; matched on the last 10 digits |
| Org | `[ORG_n]` | each `experience[].organization`; per-job "extra redact" list; always-redact entries | legal-suffix-stripped form ("Acme Robotics Pvt Ltd" also matches "Acme Robotics"); separators and case are flexible ("acme-robotics") |
| Client | `[CLIENT_n]` | `experience[].client` (end client when the employer is a consultancy) | same as Org |

Also: URLs are stripped from text before it is sent (`payload.rs`), so social and portfolio links never leave. Entries shorter than 3 normalised characters (7 digits for phones) are ignored. The always-redact list (PII page, `pii_always` table) lets you add person/email/phone/org/client/custom values; it is stored encrypted and snapshotted per job so token numbers stay stable across retries.

## What is never sent

- The master resume as a whole, the stored JSON, or `GET /api/resume` content.
- Real name, email, phone, employer and client names (replaced by tokens).
- Profile links and any URL inside bullets.
- API keys and Drive tokens (they authenticate the request but are not payload content).
- The full JD text: the model gets the role label and the top requirement terms (20 in the rewrite call, 12 in the review call), redacted. The JD itself stays local; JD URLs are fetched by the app, not by the provider.
- Learning data: corrections, playbook history, ML models, application outcomes. Rule text is appended to prompts only after being redacted and guarded, and rule text is rejected if it contains your private values, emails, links or phone-like numbers.
- The embedding model runs locally (ONNX); no text goes to a third party for matching.

What is sent in a rewrite: the selected bullets and project descriptions (redacted), skill rows, requirement terms, role label and years phrase, and your existing summary lines (redacted). The Drive upload (only when requested) sends the final resume files to your own Drive, with the `drive.file` scope (the app can only see files it created).

## The guard

`Vault::guard` runs on every outbound prompt (rewrite, plan review, repair) and on playbook rule text. It fails the whole step on any residue and reports only the kind and token (`Person:[PERSON_1]`), never the value; the job error is classified as `redact.leak` so the raw message is not stored.

- **Long values (6+ normalised characters)**: the payload is normalised (lowercase, letters and digits only) and checked with a substring match. This catches obfuscation such as `Acme-Robotics`, `acme robotics` or `A.c.m.e Robotics` for the entry "Acme Robotics".
- **Short fragments (under 6 normalised characters)**, such as "Doe": a substring match would false-positive inside normal words ("does", "doer"), so they use a word-boundary regex instead. `"it does what a doer does"` passes, `"ask Doe about it"` is blocked.
- **Phones**: any digit run that looks like a phone number is compared on its last 10 digits.

`redact` is the producer and `guard` is a stricter, independent check; a regression in redaction is caught by the guard. Tests: `crates/core/src/redact.rs` (round trip, idempotence, word boundaries, determinism) and `crates/server/tests/fit_pii.rs`.

## Grounding guard

`crates/core/src/grounding.rs` verifies each rewritten line against `ProfileFacts` (built from your resume by `profile_model.rs`). Violation kinds: `NewNumber`, `NewTechnology`, `YearsClaim`, `NewOrgOrClient`, `Superlative`, `CopiedFromSource`, plus summary shape checks (`Line1`, `WordCount`). Anything already in the original text is allowed; generic concept phrases do not trigger `NewTechnology`; concrete tools (Kubernetes, GraphQL) do.

Violating lines are reverted to the original (summary lines fall back to a deterministic facts-based summary) or, if the model repairs them in the one bounded repair call, kept after being re-checked. Violation details carry only the offending token, passed through the vault again before storage. Style-only issues (length, copying, line-1 shape) are reported but not counted as reverted.

## Key storage

- `crates/server/src/keys.rs`: AES-256-GCM, blob = 12-byte random nonce followed by ciphertext, with the provider name (or purpose label such as `pii_always`, `job_always`) as additional authenticated data, so a blob cannot be swapped between slots.
- The 32-byte master key lives in `master.key` beside the DB (override with `RESUME_MASTER_KEY_FILE`). It is created with mode 0600 using `create_new` (race safe) and the app refuses to use it if group/other can access it, or if it is not exactly 32 bytes.
- Stored in the `keys` table: Anthropic and Gemini API keys, Google OAuth client, Google refresh token. Access tokens are memory only. `GET /api/keys` returns only `configured` and `source` (`stored` or `env`).
- Always-redact values and the per-job snapshot are encrypted the same way. The PII ledger API returns masked values and salted, non-reversible ids; revealing a value needs an explicit `x-confirm: reveal` header, and the route is not reachable from the phone.
- Env keys are read at call time; `ANTHROPIC_BASE_URL` is deliberately ignored (it may point at a different proxy).
- Provider clients never put keys in URLs (Gemini uses the `x-goog-api-key` header), and `Debug` output shows `<redacted>`.
- Backups: the DB is useless without `master.key`; keep them separate.

## Remote access (phone)

See the sequence diagram in [architecture, section 7](architecture.md#7-mobile-companion-pairing-and-proxy). Summary of the controls in `crates/server/src/remote.rs` and `crates/companion/src/lib.rs`:

- **Opt-in, TLS.** Listener binds `0.0.0.0:$REMOTE_PORT` (default 8788) only after you enable it. Self-signed certificate (825 days, SANs for hostname, localhost and the current LAN/Tailscale IPs), generated once so its fingerprint stays valid. Private key mode 0600.
- **Pinning.** The phone trusts exactly one certificate by SHA-256 fingerprint carried in the pairing QR. No CA or hostname validation, so a rogue Wi-Fi cannot present another certificate.
- **Pairing.** 8-character code from an alphabet without lookalike characters (no I, O, 0, 1), valid 5 minutes, single use, stored hashed, compared in constant time; 5 attempts per minute per IP then a 10 minute lockout.
- **Device tokens.** 256 random bits per device; only `sha256(token)` is stored. Revoke one device or rotate the certificate (revokes all, needs `x-confirm: rotate`). Revocation cancels live WebSocket and SSE streams.
- **Default-deny allow-list.** `allowed()` lists exactly which method and path templates a phone may call (jobs, events, notifications, library, JD fetch, corrections, playbook rule decisions, applications, learning metrics, regression result). Not reachable from the phone: `/api/resume`, `/api/pii*`, `/api/keys*`, `/api/drive*`, `/api/settings`, `/api/heuristics`, `/api/import`, `/api/remote*`, `/api/profile/facts`, and everything not listed.
- **Localhost-only admin.** `/api/remote*` is registered only on the loopback router and also requires a `Host` of `localhost`/`127.0.0.1`/`[::1]` (DNS-rebinding guard). The remote listener answers 404 for it.
- **No PII reveal over the network.** PII ledger and reveal routes are outside the allow-list.
- **Rate limits and audit.** 300 requests per minute per device; each request recorded in `device_audit` (device, method, path template, status), visible with `GET /api/remote/audit` on localhost.
- **Response hardening.** `Cache-Control: no-store` on API responses, `X-Content-Type-Options: nosniff`, `frame-ancestors 'none'`; the UI is served with a restrictive CSP.
- **Phone side.** The proxy binds 127.0.0.1 on a random port, requires a per-launch secret (query once, then HttpOnly SameSite=Strict cookie, or `x-companion-key` header for the native share handler), checks `Host` and `Origin`, strips `Authorization`/`Cookie`/`Origin`/`Referer` from forwarded requests and adds its own bearer token. Pairing details are stored in `companion.json` (0600). Android allows cleartext only to 127.0.0.1.
- **Loopback launch secret.** The secret and port are written to `companion.port` (0600) for the Android share handler, so the secret is never in a URL beyond the first loopback redirect.

## SSRF protections on JD fetch

`crates/server/src/fetch.rs`, used by `POST /api/jd/fetch`:

- Only `http` and `https`, URL must have a host.
- The host is resolved first and every resolved address must be public: loopback, private, link-local, CGNAT (Tailscale range), documentation, multicast, broadcast, unspecified, reserved v4; v6 loopback, ULA, link-local, multicast, IPv4-mapped and NAT64 forms are mapped back to v4 and re-checked. `localhost` and `*.localhost` are refused by name.
- The checked address is pinned for the connection (`resolve`) so DNS cannot change between check and connect (rebinding).
- Redirects are followed manually (max 5) and each hop is re-validated; proxies are ignored; 15 s timeout; 2 MB cap; the response is parsed as text, not executed.
- `RESUME_ALLOW_PRIVATE_FETCH=1` disables the guard and exists only for tests (`crates/server/tests/fetch.rs`, `ssrf.rs`).
- Fetching a JD URL leaks your IP and the URL to that site, like any browser visit; paste the text instead if that matters.

## Logging policy

- No resume text, JD text, prompts, replies or secrets are written to stdout/stderr. The only `eprintln!`s are lifecycle messages (listening address, worker error text, model status, remote listener start/failure, mining errors).
- Provider and Drive error messages carry only status or kind (`gemini: HTTP 429`), never URLs, headers or bodies.
- Job events and notifications (stored in SQLite) contain counts and step names, flagged `local_only` or not. Leak errors list kind and token only. Grounding event details are passed through the vault.
- The "Privacy" tab of a job (`audit` in the job detail) shows exactly what was sent: the redacted system and user text and token counts by kind. The stored payload is the redacted one; the vault itself is never persisted.
- PII API responses are masked (first characters or last digits only) with `Cache-Control: no-store`.

## Residual risks

- **Unredacted categories.** Only name, email, phone, orgs, clients and your always-redact entries are tokenised. Schools, locations, project and certificate titles, product names, free-text mentions of colleagues, and unique achievements are sent as written. A model (or anyone with the prompt) could re-identify you from unique content. Add sensitive strings to the always-redact list (PII page) or the per-job extra list; check the Privacy tab before sending real jobs.
- **Guard is exact-match-based.** It cannot catch paraphrases or typos of a value ("Acmé Robotix"), only normalised matches. Short fragments use word boundaries, so a short name embedded in a longer word would pass.
- **Third-party processing.** The provider sees redacted but meaningful content under its own retention terms; Google sees files you upload to Drive. Drive uploads contain your real name and contact details by design (it is the final resume).
- **The phone sees real data.** Job detail and file downloads over the phone link return the restored resume (real name, contact info) and the tailored text; only the PII ledger, keys, settings and master resume endpoints are walled off. A paired, unrevoked phone is trusted with finished resumes.
- **LAN exposure when remote is on.** The TLS listener is reachable by any host that can route to the port; protection rests on the unguessable token and the pinned certificate. Turn it off when not in use.
- **Self-signed cert lifetime and IP changes.** When your IPs change the certificate no longer lists them (the UI says "certificate stale"); rotating revokes every device.
- **Local compromise.** Anyone with your user account can read the DB and master key together, and `data/out/` holds finished resumes in clear.
- **Linux-specific hardening.** File-mode checks use Unix permissions; other platforms are untested.
- **Prompt injection from JD content.** A malicious JD could try to steer the model. Impact is bounded by the guard, grounding and the lack of tools or network access for the model, but rewritten text is still model output; review before sending.
- **Demo mode.** If the UI cannot reach `/api/health` it shows in-browser demo fixtures (fake data); it never invents a connection to real data.
