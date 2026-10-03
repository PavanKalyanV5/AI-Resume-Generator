export type Provider = 'mock' | 'gemini' | 'anthropic'
export interface Profile { name: string; email: string; phone: string; role: string; location: string }
export interface Experience { id: string; role: string; organization: string; client?: string; location?: string; dateLabel?: string; bullets: string[] }
export interface SkillGroup { label: string; skills: string[] }
export interface Resume {
  profile: Profile; socials: unknown[]; summary: string[]; experience: Experience[]
  education: unknown[]; skills: SkillGroup[]; projects: unknown[]; certifications: unknown[]
}
export interface DriveLink { name: string; url: string }
export interface JobError { code: string; title: string; message: string; hint?: string; retryable?: boolean; step?: string }
export interface JobMeta {
  id: string; status: string; provider: string; created_at: string; updated_at: string; error: string | JobError | null
  company?: string | null; role?: string | null
}
export interface Step { name: string; status: string; attempts: number; error: string | null; started_at: string | null; finished_at: string | null }
export interface GraphNode { id: string; kind: string; label: string; required?: boolean }
export interface GraphEdge { from: string; to: string; kind: string; weight: number }
export interface ClusterPoint { id: string; label: string; x: number; y: number; cluster: string }
export interface ClusterInfo { id: string; label: string; size: number }
export interface Clusters { points: ClusterPoint[]; clusters: ClusterInfo[] }
export interface JobDetail {
  job: JobMeta; steps: Step[]
  coverage: { score: number; covered: string[]; missing: string[]; semantic?: string[] }
  gaps: unknown[]
  graph: { nodes: GraphNode[]; edges: GraphEdge[] }
  clusters?: Clusters | null
  audit: { redacted_payload: { system: string; user: string }; token_counts: Record<string, number> }
  files: string[]; drive?: DriveLink[] | null
  original: Resume; tailored: Resume | null; selection?: { bullets?: number[][] } | null
  decisions?: { why?: Why[] } | null
  fit?: { pages: number; layout_level: number; steps: unknown[]; dropped: unknown[] } | null
  grounding?: { bullet_id: string; repaired?: boolean; violations: { kind: string; detail: string }[] }[]
  jd_class?: JdClass | null
  similar_jobs?: { id: string; company: string; role: string; similarity: number }[]
  outcome?: { p: number; lo: number; hi: number } | null
  near_duplicates?: { a: string; b: string; similarity: number }[]
  redaction_summary?: { counts: Record<string, number>; total: number } | null
  pool?: Pool | null; review?: Review | null
  facts_summary?: { years: number; top_skills: { canonical: string; level: SkillLevel; months_used: number }[] } | null
  corrections?: Correction[]; rules_applied?: { rule_id: string; text: string; effect: string }[]; approved?: boolean
}
export const REASONS: [string, string][] = [['irrelevant', 'Irrelevant'], ['too_generic', 'Too generic'], ['outdated', 'Outdated'], ['too_long', 'Too long'], ['too_short', 'Too short'], ['wrong_tone', 'Wrong tone'], ['duplicate', 'Duplicate'], ['wrong_order', 'Wrong order'], ['already_covered', 'Already covered'], ['other', 'Other']]
/** One owner correction. `action` is add | remove | keep | reorder | edit; for edits `item_id` is the edit path (summary.0, e1.b2). */
export interface Correction {
  id: number; job_id: string; ts: number; area: string; action: string; item_id: string
  item: { title?: string; kind?: string; path?: string }; before: { text?: string; selected?: boolean } | string[] | null; after: { text?: string; selected?: boolean } | string[] | null
  reason_tags: string[]; jd_family: string; seniority: string; company: string; weight: number; source: string
}
export interface Matcher { kind?: string | null; issuer_in?: string[]; title_regex?: string | null; tier?: string | null; tech_any?: string[]; domain_bucket?: string | null; language_specific?: boolean | null; frontend_only?: boolean | null }
export interface RuleAction { boost?: number | null; forbid?: boolean; require?: boolean; cap?: number | null; prefer_over?: Matcher | null }
export interface RuleCond { jd_family?: string[]; jd_family_not?: string[]; seniority?: string[]; company?: string | null; keywords_any?: string[]; unless_keywords_any?: string[] }
export interface Impact { would_change: number; of: number; cases: string[]; summary: string }
export type RuleStatus = 'proposed' | 'active' | 'disabled' | 'rejected'
export interface PlaybookRule {
  id: string; status: RuleStatus; scope: string; area: string; condition: RuleCond; matcher: Matcher; action: RuleAction; text: string
  origin: 'seed' | 'mined' | 'user'; support_count: number; version: number; examples: string[]; impact: Impact | null
  created_at: number; updated_at: number; hits: number; badge: string | null
}
export interface Playbook { rules: PlaybookRule[]; groups: Record<RuleStatus, string[]>; versions: { version: number; ts: number; note: string }[] }
export interface RuleBody { text: string; area: string; scope: string; matcher: Matcher; action: RuleAction }
export interface RegressionRun { ran_at: number; cases: number; passing: number; results: { job_id: string; pass: boolean; diff: Record<string, { missing: string[]; extra: string[]; order_only: boolean }> }[] }
export interface LearnMetrics {
  corrections_per_job_trend: number[]
  jobs: { job_id: string; ts: number; company: string; corrections: number; approved: boolean; tokens_in: number; tokens_out: number }[]
  approval_rate: number; active_rules: number; proposed_rules: number; rule_hits: number
  regression: { cases: number; passing: number }; tokens_saved_est: number
}
export interface Pool {
  caps?: { projects: number; certs: number }
  projects: { id: string; name: string; tech: string[]; tier: string; score: number; selected: boolean }[]
  certs: { id: string; title: string; issuer: string; date: string; featured: boolean; score: number; selected: boolean }[]
  roles: { id: string; role: string; dates: string; kind: 'FullTime' | 'Internship' | 'Other'; selected: boolean; locked: boolean }[]
  skills: { label: string; skills: string[]; score: number }[]
}
export type ReviewArea = 'projects' | 'certs' | 'skills' | 'experience' | 'summary' | 'pages' | 'order'
export interface ReviewIssue {
  id: string; area: ReviewArea; severity: 'info' | 'warn' | 'error'; message: string; source: 'ai' | 'lint'
  change?: { kind: string; from?: string; to?: string; ids?: string[] }; status: 'applied' | 'suggested' | 'rejected'; reason?: string
}
export interface Overrides { projects?: string[]; certs?: string[]; roles?: string[]; skills_order?: string[]; note?: string; force?: boolean; reset?: boolean }
export interface Review {
  ran: boolean; provider: string; tokens_in: number; tokens_out: number; confidence: number; issues: ReviewIssue[]
  overrides_applied: { projects: string[]; certs: string[]; roles: string[]; skills_order: string[] }; user_overrides: Overrides | null
}
/** Issues the owner should look at: not yet applied, and either a suggestion or more than a note. */
export const needsAttention = (r?: Review | null) => (r?.issues ?? []).filter((i) => i.status !== 'applied' && (i.status === 'suggested' || i.severity !== 'info'))
export interface Why { decision: string; score: number; contributions: [string, number][]; outcome: string }
export interface JdClass { family: [string, number][]; seniority: [string, number][]; evidence: string[]; confirmed: { family: string; seniority: string } | null }
export type ModelStatus = 'ColdStart' | 'Learning' | 'Ready'
export interface ModelCard {
  name: string; n_samples: number; metric_name: string; metric_cv: number | null; baseline: number | null; status: ModelStatus
  updated_at: string; top_features: [string, number][]; learning_curve: [number, number][]; notes: string
}
export interface MlCards { cards: ModelCard[]; signals: Record<string, number> }
export interface Recs {
  learn_next: { skill: string; score: number; demand_share: number; adjacent_to: string[]; why: string }[]
  trends: [string, [string, number][]][]
  resume_additions: { skill: string; demand_share: number; evidence: unknown }[]
}
export type AppStage = 'applied' | 'response' | 'interview' | 'offer' | 'rejected' | 'ghosted'
export const STAGES: AppStage[] = ['applied', 'response', 'interview', 'offer', 'rejected', 'ghosted']
export interface Application {
  job_id: string; company: string; role: string; stage: AppStage; note: string | null; applied_at: string; updated_at: string
  history: { stage: AppStage; note: string | null; ts: string }[]
}
export interface Funnel {
  total: number; pending: number; responded: number; interviews: number; offers: number; rejected: number; ghosted: number
  response_rate: number; response_ci: [number, number]; interview_rate: number; offer_rate: number
}
export type SkillLevel = 'Beginner' | 'Working' | 'Strong' | 'Expert'
export type Ym = [number, number]
export interface ProfileFacts {
  headline: string
  timeline: { idx: number; kind: 'FullTime' | 'Internship' | 'Other'; role: string; org: string; start: Ym | null; end: Ym | null; months: number; is_current: boolean }[]
  years: { fulltime: number; internship: number; other: number; total_effective: number }
  skills: { canonical: string; category: string; level: SkillLevel; months_used: number; proficiency: number; mentions: number }[]
  achievements: { role_idx: number; bullet_idx: number; text: string; impact_score: number; metrics: { raw: string; unit: string }[] }[]
  domains: { name: string; skills: string[]; weight: number }[]
  gaps_in_timeline: [Ym, Ym, number][]
  summary_stats: Record<string, number>
  skill_timeline: { skill: string; start: Ym; end: Ym; months: number }[]
  skill_matrix: { skill: string; proficiency: number }[]
}
export type HeurVals = Record<string, number>
export interface Heuristics { effective: HeurVals; defaults: HeurVals }
export interface JobEvent { seq: number; step: string; level: string; message: string; local_only: boolean; ts: string }
export interface Stats {
  jobs_by_day: { day: string; count: number }[]; tokens_in: number; tokens_out: number
  avg_coverage: number; by_status: Record<string, number>
}
export interface NewJob {
  jd_text: string; provider: Provider; extra_redact: string[]; max_tokens?: number
  company?: string; role?: string; upload_drive?: boolean; pages?: 1 | 2
}
export interface JdFetch { text: string; title: string; company: string; source: string }
export interface LibraryItem {
  id: string; company: string; role: string; created_at: string
  files: { name: string; display_name: string }[]; drive: DriveLink[] | null; coverage: number | null
}
export type KeyProvider = 'gemini' | 'anthropic'
export type KeySource = 'stored' | 'env' | null
export type Keys = Record<KeyProvider, { configured: boolean; source?: KeySource }>
export interface DriveState { configured: boolean; connected: boolean; folder_name: string; account_email?: string | null; redirect_uri: string }
export interface ImportResult { resume: Resume; confidence: number; warnings: string[] }
export type LinkState = 'live' | 'reconnecting'

export type Zone = 'local' | 'ai' | 'drive'
export const STEPS = ['parse_jd', 'select', 'ai_review', 'build_payload', 'ai_tailor', 'restore', 'render_docx', 'render_pdf', 'finalize', 'upload_drive'] as const
export const BASE_STEPS = STEPS.filter((s) => s !== 'upload_drive')
export const STEP_INFO: Record<string, { title: string; blurb: string; zone: Zone; tag: string }> = {
  parse_jd: { title: 'Read the job description', blurb: 'Pulls out the requirements and keywords.', zone: 'local', tag: 'Local' },
  select: { title: 'Pick relevant experience', blurb: 'Ranks your bullets against each requirement.', zone: 'local', tag: 'Local' },
  ai_review: { title: 'Review the plan', blurb: 'A second AI check of what was picked, then local rules fix leftovers.', zone: 'ai', tag: 'Sent to AI (redacted)' },
  build_payload: { title: 'Redact and package', blurb: 'Swaps identifying values for tokens.', zone: 'local', tag: 'Local' },
  ai_tailor: { title: 'Tailor with the AI provider', blurb: 'The only step that sends your text to an AI.', zone: 'ai', tag: 'Sent to AI (redacted)' },
  restore: { title: 'Restore your details', blurb: 'Puts the real values back, locally.', zone: 'local', tag: 'Local' },
  render_docx: { title: 'Build the DOCX', blurb: 'Lays out the Word version.', zone: 'local', tag: 'Local' },
  render_pdf: { title: 'Build the PDF', blurb: 'Lays out the PDF version.', zone: 'local', tag: 'Local' },
  finalize: { title: 'Finish up', blurb: 'Saves files and the match report.', zone: 'local', tag: 'Local' },
  upload_drive: { title: 'Save to Google Drive', blurb: 'Uploads the finished files to your own Drive folder.', zone: 'drive', tag: 'Sent to Google Drive (your account)' },
}
export const isActive = (s: string) => ['queued', 'pending', 'running'].includes(s)
export const isDone = (s: string) => ['succeeded', 'success', 'done', 'completed', 'finished'].includes(s)
export const isFailed = (s: string) => ['failed', 'error'].includes(s)

export type PiiKind = 'person' | 'email' | 'phone' | 'org' | 'client' | 'custom'
export type PiiArea = 'experience' | 'projects' | 'education' | 'skills' | 'certifications' | 'summary' | 'jd'
export interface PiiEntry {
  id: string; kind: PiiKind; token: string; masked: string; length: number
  source: 'profile' | 'experience' | 'always' | 'job'
  occurrences: { area: PiiArea; count: number }[]; total: number
}
export interface PiiCandidate { id: string; kind: 'email' | 'phone' | 'name' | 'url_handle'; masked: string; where: PiiArea; count: number }
export interface PiiAlways { id: string; kind: string; masked: string }
export interface PiiData { entries: PiiEntry[]; candidates: PiiCandidate[]; always: PiiAlways[] }

export type Level = 'info' | 'success' | 'warn' | 'error'
export interface NotifAction { label: string; action: 'retry' | 'open' | 'settings' | 'dismiss'; target?: string }
export interface RtEvent {
  seq: number; scope: 'job' | 'system'; job_id?: string; kind: 'step' | 'job_status' | 'notice'; level: Level; code?: string
  title: string; message: string; hint?: string; retryable?: boolean; actions?: NotifAction[]; step?: string; local_only?: boolean
  progress?: { done: number; total: number }; ts: string
}
export interface Notif { seq: number; level: Level; title: string; message: string; hint?: string; code?: string; job_id?: string; step?: string; retryable?: boolean; actions?: NotifAction[]; ts: string; read: boolean }
export type ConnState = 'live' | 'reconnecting' | 'offline' | 'polling'
export interface ConnInfo { state: ConnState; via: 'ws' | 'sse' | 'poll'; retryAt?: number }

export interface RemoteStatus { enabled: boolean; port: number | null; fingerprint: string | null; hostname: string | null; addresses: { ip: string; kind: 'lan' | 'tailscale' }[]; devices: number; cert_stale: boolean }
export interface RemoteDevice { id: string; name: string; created_at: number; last_seen: number | null }
export interface RemoteAudit { ts: number; device_id: string; device: string; method: string; path: string; status: number }
export interface PairCode { code: string; expires_at: number; qr_payload: { v: 1; hosts: string[]; port: number; fp: string; code: string } }
