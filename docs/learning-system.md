# Learning system

How the app learns from your corrections without fine-tuning anything and without sending learning data anywhere. See also: [architecture](architecture.md), [privacy and security](privacy-and-security.md), [README](../README.md).

```
your edits ──► corrections (labelled examples) ──► mining ──► proposed rules ──► you approve ──► active rules ─┐
     │                                                                                                         ├──► selection / prompts
     └──► small local models (ranker, rewrite need, JD classifier, ...) learn from the same signals ───────────┘
approved resumes ──► golden set ──► regression replay (no AI) checks every rule change
```

Everything here is local, deterministic and inspectable. Code: `crates/server/src/learn.rs` (corrections, playbook store, golden set, metrics), `crates/core/src/playbook.rs` (rule language, mining, seeds), `crates/server/src/mlsvc.rs` and `crates/core/src/ml/*` (models).

## Corrections as labelled examples

A correction is one row in `corrections`: what you changed relative to what the app proposed, with context.

| Field | Meaning |
|---|---|
| `area` | `certs`, `projects`, `roles` (experience), `skills`, `summary`, `bullets` |
| `action` | `add`, `remove`, `keep`, `reorder`, `edit` |
| `item_json` | the item as rules see it (`Cand`): id, kind, title, issuer, tier, tech, domain bucket, language-specific flag, frontend-only flag. Titles only, no personal data |
| `before_json` / `after_json` | selection state, skills order, or the edited line text |
| `reason_tags` | optional, from the fixed list below |
| `jd_family`, `seniority`, `company` | context of the job; the family is your confirmed label (`POST /api/jobs/:id/jd-class`) or the local classifier's best guess |
| `weight` | 1.0 for explicit actions, 0.5 for `keep` |
| `source` | `owner` |

How they are produced:

- **Plan overrides** (`POST /api/jobs/:id/overrides`, Review page): every project, cert or role you add or remove relative to the AI/local plan, plus one `reorder` when you change the skills order.
- **Line edits** (`POST /api/jobs/:id/edit`): `summary.<n>` or `e<i>.b<j>` replaced by your text (1 to 2000 characters, only on finished/failed/cancelled jobs). The edit is stored in `job_edits`, re-applied after every re-render and the job re-renders from `render_docx`. Recorded as an `edit` correction.
- **Bullet feedback** (`POST /api/jobs/:id/feedback`, action `keep|remove|add_back|edit`): recorded as `bullets` corrections and used to train the ranker.
- **Approval** (`POST /api/jobs/:id/approve`, finished jobs only): the final selection becomes a golden case, and each cert and project you left in place is stored as a `keep` correction (weight 0.5). These are the positive evidence that stops mining from proposing removals of things you consistently keep.

Reason tags (`POST /api/corrections/:id/reason`, or in the edit request): `irrelevant`, `too_generic`, `outdated`, `too_long`, `too_short`, `wrong_tone`, `duplicate`, `wrong_order`, `already_covered`, `other`. Unknown tags are rejected. Tags are metadata for you and for future mining; current mining does not condition on them.

Corrections store item titles (certs, projects, roles) and line text you edited, locally; none of it is sent to a provider. The PII guard still applies to any rule text that reaches a prompt.

## The playbook rule language

A rule (`Rule` in `playbook.rs`, table `playbook_rules`) is: scope + condition + matcher + action + human text.

| Part | Fields |
|---|---|
| `area` | `certs`, `projects`, `skills`, `roles`, `summary`, `bullets` |
| `scope` | `global`, `family:<jd family>`, `company:<name>` (families: backend, frontend, fullstack, data_eng, ml_ai, devops_cloud, mobile, security, embedded, qa, management) |
| `condition` | `jd_family`, `jd_family_not`, `seniority` (junior, mid, senior, lead), `company`, `keywords_any`, `unless_keywords_any` (whole-word match in the JD text) |
| `matcher` (all given fields must hold) | `kind` (cert, project, role, skill), `issuer_in`, `title_regex` (on "title issuer"), `tier`, `tech_any`, `domain_bucket`, `language_specific`, `frontend_only` |
| `action` (exactly one for certs/projects) | `boost` (-0.5 to 0.5, added to the item's score), `forbid`, `require`, `cap` (at most n matching items), `prefer_over` (matching items beat items matching another matcher) |
| `text` | 1-300 characters; shown in the UI and, for `summary`/`bullets` areas, appended to the writer prompt ("Owner rules (must follow)"); for certs/projects/skills/roles it is appended to the plan-review prompt. Always redacted and guarded |

Constraints (`validate`): `roles` rules can only `forbid` (the latest full-time and latest internship role are locked and can never be dropped); `skills` rules can only `boost`; the matcher must have at least one field; regexes must compile; text may not contain emails, links or phone-like numbers or any of your private values. `summary`/`bullets` rules are text-only instructions. Rules can only re-rank, drop or require items the selection already validates; they never touch the PII guard, grounding, caps or locked roles.

Examples (fake):

```json
{
  "area": "certs", "scope": "family:devops_cloud",
  "matcher": { "kind": "cert", "issuer_in": ["aws", "azure"] },
  "action": { "boost": 0.25 },
  "text": "For cloud jobs prefer recognised cloud certificates."
}
```

```json
{
  "area": "projects", "scope": "global",
  "condition": { "jd_family_not": ["frontend", "fullstack"] },
  "matcher": { "kind": "project", "frontend_only": true },
  "action": { "forbid": true },
  "text": "Leave out frontend-only projects unless the job is frontend or fullstack."
}
```

```json
{
  "area": "certs", "scope": "global",
  "condition": { "unless_keywords_any": ["data structures"] },
  "matcher": { "kind": "cert", "title_regex": "(?i)(basics|bootcamp)" },
  "action": { "cap": 1 },
  "text": "Keep at most one beginner or bootcamp certificate."
}
```

Create with `POST /api/playbook/rules` (owner-written rules start `active`), edit with `PUT /api/playbook/rules/:id` (bumps `version`). Each applied rule is listed per job (`rules_applied`, with its effect, e.g. "avoided Java Basics") and counted (`hits`).

## Mining thresholds

After every line edit, approval and override batch, `mine_bg` runs `mine_rules` over corrections in `certs` and `projects` (`playbook.rs`):

1. Group corrections by (area, item signature, JD family). A signature is a kind of item, not a specific one: language-specific basics certs; certs by issuer; certs by domain bucket; frontend-only projects; projects by tier.
2. Propose a rule only if at least **3 distinct jobs** show the same action for the same signature and family, and nothing contradicts it: repeated `remove` with no `add` and no `keep` (proposes a `-0.5` boost, "avoid"), or repeated `add` with no `remove` (proposes `+0.25`, "prefer").
3. Never propose what an existing rule already covers, in any status. A rejected rule is not proposed again.
4. Scope is `family:<x>` when the JD family is known, else `global`. Text is generated, e.g. "For ml_ai JDs you removed language-specific basics certs 3 times: avoid them". The proposal stores up to 5 example job ids and a `support_count`.

Mining only proposes; it never activates anything. A "New rule suggestion" notification links to the Playbook page. Mined ids look like `mined-<uuid>`; owner rules `user-<uuid>`.

## Approval flow

```
proposed ──approve──► active ──disable──► disabled ──enable──► active
    └─────reject────► rejected ──enable──► active
```

`POST /api/playbook/rules/:id/{approve|reject|disable|enable}`. On approve or enable the server first runs the rule against the golden set and stores `impact` ("would change 2 of 9 approved resumes") before the rule goes live, then snapshots the playbook. Seed rules can be disabled but not deleted; other rules can be deleted. The phone allow-list includes approve/reject/disable/enable and reason tagging, so you can decide on a suggestion from your phone.

## Seed rules

Inserted once at startup by `seed_rules()` (never re-inserted if you disabled or edited one), badged "From your ... feedback" in the UI, origin `seed`, active:

| id | Effect |
|---|---|
| `seed-cert-basics` | forbid language-specific basics, training-programme, DSA and bootcamp certificates unless the JD mentions data structures/algorithms |
| `seed-cert-data`, `seed-cert-ml`, `seed-cert-cloud`, `seed-cert-security` | +0.25 for recognised-vendor certificates in the matching domain and JD families |
| `seed-proj-featured` | +0.15 for featured-tier projects |
| `seed-proj-frontend` | forbid frontend-only projects unless the job is frontend or fullstack |
| `seed-summary` | summary style: 3-4 lines of 22-32 words, key terms in bold, line 1 has the role label and the allowed years phrase |

Reverse-chronological order is enforced in code, not as a rule.

## Regression set

Approving a finished job stores a golden case in `golden`: JD text, family, seniority, company, and the final selection (projects, certs, roles, skills by title). `POST /api/regression/run` replays every case with the current rules and stores the result in `regression_runs` (`GET /api/regression/latest`). A replay is `parse_jd` + `select` + the local lint fixes + rule enforcement: **no AI, no rendering, no ML re-ranking**, so it is fast, free and deterministic. A case passes when projects, certs and roles match exactly (set and order); otherwise the diff lists missing and extra titles per area and whether only the order differs. The same replay is used to estimate a rule's impact before approval.

## ML models

All live in one JSON bundle (`settings.ml_bundle`, `ModelBundle` in `ml/registry.rs`), written in one statement. Every model is a prior (hand-tuned or synthetic) blended with your data, deterministic, serialisable, and reports a *model card* (`GET /api/ml/cards`): sample count, cross-validated metric against a trivial baseline, status, top features, learning curve.

**Status cut points** (`ml/core.rs`): fewer than 10 samples is `ColdStart` (prior only); 10 to 29 is `Learning`; 30 or more is `Ready` only when the cross-validated metric beats the trivial baseline, otherwise it stays `Learning`. The bullet ranker and the rewrite predictor additionally reduce their influence until data exists, as below.

| Model (file) | Predicts / used for | Features | Labels | Cold start |
|---|---|---|---|---|
| **Bullet ranker** (`ranker.rs`) | which experience bullets to keep inside each role's budget | `bm25_norm`, `max_cos`, `kw_hit`, `skill_prof`, `recency`, `has_metric`, `verb_class`, `length`, `position`, `is_fulltime`, `is_intern` | keep/add_back = 1, remove = 0 (edits ignored) | prior weights reproduce the hand-tuned select score; influence is 0 until 10 events, then `n/(n+40)`, capped at 60% of the final score |
| **Rewrite need** (`rewrite_need.rs`) | whether an AI rewrite of a bullet is worth its tokens; unneeded bullets are not sent | `potential_gain` (JD skills you have evidence for but the bullet lacks), `length`, `has_metric`, `verb_strength`, `keyword_hits` | accepted = text changed, passed grounding, not reverted | below 10 samples every bullet with gain > 0 is rewritten; afterwards rewrite when P(accepted) >= 0.35; zero gain is never rewritten. The job log shows tokens saved |
| **JD classifier** (`jd_classifier.rs`) | JD family (11) and seniority (4) for rules, context and analytics | token counts (naive Bayes with idf, online updates) | your confirmed labels (weight 3) | seeded with synthetic documents from editable keyword lists in the source; card reports prequential accuracy |
| **Fit model** (`fit_model.rs`) | rendered line count and lines per page for each layout, to skip trial renders | analytic line estimate, bullet count, character count, section count; per-layout capacity | measured per-page line counts of PDFs you actually render | analytic page-geometry estimate, confidence about 0.15; used to pick the starting layout only at confidence >= 0.6 (and still verified by a real render) |
| **Outcome model** (`outcome.rs`) | P(company responds) for an application, with a ~90% bootstrap interval; funnel with Wilson intervals | `coverage`, `semantic_share`, `required_missing`, `years_gap`, `pages`, `family_match_prob`, `seniority_match` | stage: Response/Interview/Offer/Rejected = responded, Ghosted = no response; Applied (pending) is excluded | no prediction at all below 25 labelled applications |
| **PII candidates** (`pii_candidate.rs`) | which capitalised or contact-like tokens in a JD or resume to suggest for redaction on the PII page | capitalised, cue word before (by, with, contact, manager ...), in JD, length, email, phone, domain, has digit, previous capitalised | accept = should redact, dismiss = not PII | prior encodes the heuristic (emails/phones/domains strong, capitalised after a cue moderate); suggests at score >= 0.5. |

Support components that are not learned models: `recommend.rs` (what to learn next and what to add to your skills section, from skill demand and PMI co-occurrence across your own JD history; it only suggests skills you have real evidence for), `neighbors.rs` (nearest past JDs by cosine; embedder vectors when available, hashed unigrams otherwise), `cluster.rs` (near-duplicate bullets).

Training signals: bullet feedback and overrides (ranker), restore results (rewrite need), PII accept/dismiss, JD label confirmations, application stage changes, real PDF renders (fit). Models refit in the background (`retrain_bg`, a notification says "Models updated"); `POST /api/ml/retrain` forces it. Lock order is always DB then ML bundle.

## Learning metrics

`GET /api/learning/metrics` (Learning page):

- `corrections_per_job_trend` and per-job `jobs` (time, company, correction count excluding `keep`, approved, AI tokens in/out): the number to watch; it should fall as rules and models adapt.
- `approval_rate` (approved / finished jobs), `active_rules`, `proposed_rules`, `rule_hits` (times rules fired across stored selections).
- `regression` (cases, passing) from the latest run.
- `tokens_saved_est`: estimate from bullets not sent for rewriting plus tokens avoided by cached AI replies.

Other: `GET /api/stats` (jobs per day, tokens, average coverage), `GET /api/applications/funnel`, model cards, the Lab page.

## Inspect and roll back

- **See what a rule did:** job detail `rules_applied` and the Match/Review pages; `hits` and `impact` per rule in `GET /api/playbook`.
- **See why a correction exists:** `GET /api/jobs/:id/corrections`.
- **Playbook history:** every create, edit, approve, reject, disable, enable and delete writes a full snapshot to `playbook_versions` with a note ("approve rule mined-...") and bumps the rule `version`. `POST /api/playbook/rollback {"version": n}` restores that snapshot (rules mined after it that are still proposed or rejected are kept), and records the rollback itself as a new version. Run the regression set afterwards to confirm.
- **Quick off-switch for one rule:** disable it. Seed rules can only be disabled.
- **Models:** retrain (`POST /api/ml/retrain`); to reset to cold-start priors stop the app and delete the bundle (`DELETE FROM settings WHERE key='ml_bundle';` in `app.db`). A missing or corrupt bundle loads as cold-start defaults. Reverting an individual correction is not supported; corrections are append-only labels.
- **Heuristics:** `GET/PUT/DELETE /api/heuristics` (or edit `heuristics.json`); DELETE restores defaults.
- **Per-job owner overrides** can be cleared with `reset` on the overrides endpoint, which re-runs the job from `select`.
