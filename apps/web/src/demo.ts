// In-browser stand-in for the backend. Everything is fake and lives in memory.
import { jobExtras } from './demoMl'
import { AUDIT, CLUSTERS, DEMO_JD, GAPS, GRAPH, LIBRARY, ORIGINAL, REQ_COVERED, REQ_MISSING, SEMANTIC, TAILORED } from './fixtures'
import { reviewFor } from './demoReview'
import { learnExtras, recordOverrides, withEdits } from './demoLearn'
import { ApiError } from './api'
import { friendly } from './lib/errors'
import { BASE_STEPS, type DriveState, type ImportResult, type JdFetch, type JobDetail, type JobEvent, type JobMeta, type KeyProvider, type NewJob, type Overrides, type JobError, type Notif, type RtEvent, type PiiArea, type PiiData, type PiiEntry, type PiiKind, type Resume, type Stats, type Step } from './types'

interface DJob { fail?: { code: string; step: string } | null; meta: JobMeta; steps: Step[]; events: JobEvent[]; failNext: boolean; timers: number[]; cancelled: boolean }
const jobs = new Map<string, DJob>()
const overrides = new Map<string, Overrides>()
const subs = new Map<string, Set<(e: JobEvent) => void>>()
let resume: Resume = ORIGINAL
const keys: Record<KeyProvider, { configured: boolean; source: 'stored' | 'env' | null }> = { gemini: { configured: false, source: null }, anthropic: { configured: true, source: 'env' } }
const drive: DriveState = { configured: true, connected: true, folder_name: 'Resumes (Veil)', account_email: 'jane.doe@example.test', redirect_uri: 'http://127.0.0.1:8787/api/drive/callback' }
const now = () => new Date().toISOString()
const DAY = 864e5

const TIMING: Record<string, number> = { parse_jd: 900, select: 1100, ai_review: 1800, build_payload: 900, ai_tailor: 3200, restore: 700, render_docx: 900, render_pdf: 1100, finalize: 600, upload_drive: 1400 }
const START: Record<string, (p: string) => string> = {
  parse_jd: () => 'Reading the job description and pulling out the requirements.',
  select: () => 'Matching your bullets to each requirement.',
  ai_review: (p) => `Sending the redacted plan to ${p} for a second check.`,
  build_payload: () => 'Replacing your name, contact details and employers with tokens.',
  ai_tailor: (p) => `Sending the redacted text to ${p}. Nothing else leaves your machine.`,
  restore: () => 'Putting your real name, contact details and employers back.',
  render_docx: () => 'Laying out the Word document.',
  render_pdf: () => 'Laying out the PDF.',
  finalize: () => 'Saving the files and the match report.',
  upload_drive: () => 'Uploading the PDF and DOCX to your Google Drive folder.',
}
const DONE: Record<string, string> = {
  parse_jd: 'Found 11 requirements, 7 marked required.',
  select: 'Picked 8 bullets across 3 roles. 7 of 11 requirements are covered.',
  ai_review: 'The check swapped 2 projects, and local rules fixed 2 more things. 1 suggestion is waiting for you.',
  build_payload: 'Redacted 10 values into tokens. The payload is ready for review.',
  ai_tailor: 'The provider returned reworded bullets and a new summary.',
  restore: 'All 10 tokens were restored locally.',
  render_docx: 'resume.docx is ready.',
  render_pdf: 'resume.pdf is ready.',
  finalize: 'Done. Your tailored resume is ready to review.',
  upload_drive: 'Saved 2 files to your Google Drive folder.',
}

const blankSteps = (withDrive = false): Step[] => [...BASE_STEPS, ...(withDrive ? ['upload_drive' as const] : [])].map((name) => ({ name, status: 'pending', attempts: 0, error: null, started_at: null, finished_at: null }))

function emit(j: DJob, step: string, level: string, message: string) {
  const e: JobEvent = { seq: j.events.length + 1, step, level, message, local_only: step !== 'ai_tailor' && step !== 'ai_review', ts: now() }
  j.events.push(e)
  subs.get(j.meta.id)?.forEach((f) => f(e))
}
const touch = (j: DJob, status?: string, error: string | JobError | null = null) => { j.meta.updated_at = now(); if (status) { j.meta.status = status; j.meta.error = error } }
const later = (j: DJob, ms: number, f: () => void) => { j.timers.push(window.setTimeout(f, ms)) }

const FAILS: [RegExp, string, string][] = [
  [/\b(ratelimit|fail)\b/i, 'ai.rate_limited', 'ai_tailor'], [/\bauth\b/i, 'ai.auth', 'ai_tailor'], [/\bbudget\b/i, 'ai.budget_exceeded', 'ai_tailor'],
  [/\btimeout\b/i, 'ai.timeout', 'ai_tailor'], [/\bbadreply\b/i, 'ai.bad_reply', 'ai_tailor'], [/\bleak\b/i, 'redact.leak', 'build_payload'],
  [/\bfitfail\b/i, 'render.fit_failed', 'render_pdf'], [/\btexfail\b/i, 'render.tex_failed', 'render_pdf'], [/\bquota\b/i, 'drive.quota', 'upload_drive'], [/\bnodrive\b/i, 'drive.not_connected', 'upload_drive'],
]
const jobLabel = (j: DJob) => [j.meta.role, j.meta.company].filter(Boolean).join(' at ') || 'Your resume'
const pub = (j: DJob, e: Partial<RtEvent> & Pick<RtEvent, 'kind' | 'level' | 'title' | 'message'>) => bus.publish({ scope: 'job', job_id: j.meta.id, ...e })

function run(j: DJob, i = 0) {
  if (j.cancelled) return
  if (i >= j.steps.length) {
    touch(j, 'succeeded')
    return pub(j, { kind: 'job_status', level: 'success', title: 'Resume tailored', message: jobLabel(j), actions: [{ label: 'Open job', action: 'open' }] })
  }
  const st = j.steps[i], name = st.name
  st.status = 'running'; st.attempts++; st.started_at = now(); st.error = null; st.finished_at = null
  touch(j, 'running')
  emit(j, name, 'info', START[name](j.meta.provider === 'mock' ? 'the mock provider' : j.meta.provider))
  if (name === 'ai_tailor') {
    const T = TIMING[name]
    ;[1, 2].forEach((n) => later(j, (T * n) / 3, () => { emit(j, name, 'info', n === 1 ? 'Waiting for the provider to answer.' : 'The provider is writing the reworded bullets.'); pub(j, { kind: 'step', level: 'info', step: name, title: 'Tailoring with the AI', message: 'Waiting on the provider', progress: { done: n, total: 3 } }) }))
  }
  later(j, TIMING[name], () => {
    st.finished_at = now()
    if (j.fail && j.fail.step === name) {
      const f = friendly(j.fail.code, undefined, j.meta.id, name)
      j.fail = null
      st.status = 'failed'; st.error = f.message
      touch(j, 'failed', { code: f.code, title: f.title, message: f.message, hint: f.hint, retryable: f.retryable, step: name })
      emit(j, name, 'error', f.message)
      return pub(j, { kind: 'job_status', level: f.level === 'info' ? 'info' : f.level === 'warn' ? 'warn' : 'error', code: f.code, step: name, title: f.title, message: f.message, hint: f.hint, retryable: f.retryable, actions: f.actions })
    }
    st.status = 'succeeded'
    emit(j, name, 'info', DONE[name])
    pub(j, { kind: 'step', level: 'info', step: name, title: DONE[name], message: DONE[name], ...(name === 'ai_tailor' ? { progress: { done: 3, total: 3 } } : {}) })
    if (name === 'restore') pub(j, { kind: 'notice', level: 'info', code: 'grounding.reverted', title: 'Kept your original wording on one bullet', message: 'A rewrite added a claim that is not in your resume, so it was reverted.', hint: 'Compare the Result tab to see which bullet stayed as written.', actions: [{ label: 'Open job', action: 'open' }] })
    run(j, i + 1)
  })
}

// ---- fake realtime bus: same frames as the real /api/ws and /api/events ----
const FLAKY = new URLSearchParams(location.search).get('flaky')
const BASE = Math.floor(Date.now() / 1000) * 100
const bus = {
  log: [] as RtEvent[], subs: new Set<(e: RtEvent) => void>(),
  publish(e: Partial<RtEvent> & Pick<RtEvent, 'kind' | 'level' | 'title' | 'message'>) {
    const full: RtEvent = { scope: 'system', ts: now(), ...e, seq: BASE + bus.log.length + 1 }
    bus.log.push(full); bus.subs.forEach((f) => f(full)); persistNotif(full)
  },
}
let failsLeft = 0
class FakeSocket {
  onopen: (() => void) | null = null; onmessage: ((e: { data: string }) => void) | null = null; onclose: (() => void) | null = null; onerror: (() => void) | null = null
  private timers: number[] = []; private off = () => {}; private dead = false
  constructor(since: number) {
    const t = (f: () => void, ms: number) => this.timers.push(window.setTimeout(f, ms))
    const fail = FLAKY === 'sse' || (FLAKY === '1' && failsLeft > 0 && failsLeft--)
    t(() => {
      if (fail) return this.die()
      this.onopen?.()
      this.send0({ t: 'hello', seq_head: BASE + bus.log.length })
      bus.log.filter((e) => e.seq > since).forEach((e) => this.send0({ t: 'event', ...e }))
      const f = (e: RtEvent) => this.send0({ t: 'event', ...e })
      bus.subs.add(f); this.off = () => bus.subs.delete(f)
      const ping = window.setInterval(() => this.send0({ t: 'ping' }), 15000); this.timers.push(ping)
      if (FLAKY === '1') t(() => { failsLeft = 1; this.die() }, 9000 + Math.random() * 4000)
    }, 350)
  }
  private send0(o: unknown) { if (!this.dead) this.onmessage?.({ data: JSON.stringify(o) }) }
  private die() { if (this.dead) return; this.dead = true; this.cleanup(); this.onclose?.() }
  private cleanup() { this.off(); this.timers.forEach((x) => { clearTimeout(x); clearInterval(x) }) }
  send() {}
  close() { this.dead = true; this.cleanup() }
}
class FakeEvents {
  onopen: (() => void) | null = null; onmessage: ((e: { data: string }) => void) | null = null; onerror: (() => void) | null = null
  private t = 0; private off = () => {}
  constructor(since: number) {
    this.t = window.setTimeout(() => {
      this.onopen?.()
      bus.log.filter((e) => e.seq > since).forEach((e) => this.onmessage?.({ data: JSON.stringify(e) }))
      const f = (e: RtEvent) => this.onmessage?.({ data: JSON.stringify(e) })
      bus.subs.add(f); this.off = () => bus.subs.delete(f)
    }, 300)
  }
  close() { clearTimeout(this.t); this.off() }
}

// ---- notification center persistence (demo only: localStorage, fake data) ----
const NK = 'veil.demo.notifs'
const day = 864e5
const seedNotifs = (): Notif[] => [
  { seq: -3, level: 'success', title: 'Resume tailored', message: 'Senior Robotics Software Engineer at Globex Logistics', job_id: 'demo-seed-1', ts: new Date(Date.now() - 1.2 * 36e5).toISOString(), read: false, actions: [{ label: 'Open job', action: 'open' }] },
  { seq: -2, level: 'warn', code: 'ai.rate_limited', title: 'The AI provider is busy', message: 'It rate-limited this request. Nothing was lost.', hint: 'Wait a minute and retry from this step, or switch provider.', job_id: 'demo-seed-4', step: 'ai_tailor', retryable: true, ts: new Date(Date.now() - 3 * day).toISOString(), read: false, actions: [{ label: 'Retry from this step', action: 'retry', target: 'ai_tailor' }, { label: 'Open job', action: 'open' }] },
  { seq: -4, level: 'info', code: 'playbook.proposed', title: 'New rule suggestion', message: 'For devops_cloud JDs you removed Acme Cloud Academy certs 3 times: avoid them', ts: new Date(Date.now() - 0.5 * 36e5).toISOString(), read: false, actions: [{ label: 'Open playbook', action: 'open', target: '/playbook' }] },
  { seq: -1, level: 'info', code: 'grounding.reverted', title: 'Kept your original wording on one bullet', message: 'A rewrite added a claim that is not in your resume, so it was reverted.', job_id: 'demo-seed-2', ts: new Date(Date.now() - 1.1 * day).toISOString(), read: true },
]
const loadNotifs = (): Notif[] => { try { const r = localStorage.getItem(NK); if (r) return JSON.parse(r) } catch { /* ignore */ } return seedNotifs() }
let notifs = loadNotifs()
const saveNotifs = () => { try { localStorage.setItem(NK, JSON.stringify(notifs.slice(-200))) } catch { /* private mode */ } }
function persistNotif(e: RtEvent) {
  if (e.kind === 'step' || notifs.some((n) => n.seq === e.seq)) return
  notifs = [...notifs, { seq: e.seq, level: e.level, title: e.title, message: e.message, hint: e.hint, code: e.code, job_id: e.job_id, step: e.step, retryable: e.retryable, actions: e.actions, ts: e.ts, read: false }]
  saveNotifs()
}

function seed() {
  const specs: [string, string, number, string][] = [
    ['demo-seed-1', 'anthropic', 1.2, 'succeeded'], ['demo-seed-2', 'gemini', 26, 'succeeded'], ['demo-seed-3', 'mock', 50, 'succeeded'],
    ['demo-seed-4', 'anthropic', 74, 'failed'], ['demo-seed-5', 'anthropic', 98, 'succeeded'], ['demo-seed-6', 'gemini', 150, 'succeeded'],
  ]
  for (const [id, provider, hrs, status] of specs) {
    const lib = LIBRARY.find((l) => l.id === id)
    const steps = blankSteps(!!lib?.drive).filter((s) => !(id === 'demo-seed-3' && s.name === 'ai_review')) // an older job from before the review step
    const t0 = Date.now() - hrs * 36e5
    steps.forEach((s, i) => {
      const failed = status === 'failed' && s.name === 'ai_tailor'
      const pending = status === 'failed' && i > steps.findIndex((x) => x.name === 'ai_tailor')
      if (pending) return
      s.status = failed ? 'failed' : 'succeeded'; s.attempts = failed ? 2 : 1
      s.error = failed ? 'Provider returned HTTP 529 (overloaded) twice.' : null
      s.started_at = new Date(t0 + i * 1500).toISOString(); s.finished_at = new Date(t0 + i * 1500 + 1200).toISOString()
    })
    const j: DJob = { meta: { id, status, provider, company: lib?.company ?? null, role: lib?.role ?? null, created_at: new Date(t0).toISOString(), updated_at: new Date(t0 + 14000).toISOString(), error: status === 'failed' ? { code: 'ai.rate_limited', title: friendly('ai.rate_limited').title, message: friendly('ai.rate_limited').message, hint: friendly('ai.rate_limited').hint, retryable: true, step: 'ai_tailor' } : null }, steps, events: [], failNext: false, timers: [], cancelled: false }
    steps.forEach(({ name: n }, i) => { if (steps[i].status !== 'pending') j.events.push({ seq: j.events.length + 1, step: n, level: steps[i].status === 'failed' ? 'error' : 'info', message: steps[i].error ?? DONE[n], local_only: n !== 'ai_tailor' && n !== 'ai_review', ts: steps[i].finished_at! }) })
    jobs.set(id, j)
  }
}
seed()

export function detail(id: string): JobDetail {
  const j = jobs.get(id)
  if (!j) throw new Error('Job not found')
  const ok = (n: string) => j.steps.find((s) => s.name === n)?.status === 'succeeded'
  const sel = ok('select')
  return {
    job: { ...j.meta }, steps: j.steps.map((s) => ({ ...s })),
    coverage: sel ? { score: 0.68, covered: REQ_COVERED, missing: REQ_MISSING, semantic: SEMANTIC } : { score: 0, covered: [], missing: [], semantic: [] },
    gaps: sel ? GAPS : [], graph: sel ? GRAPH : { nodes: [], edges: [] }, clusters: sel ? CLUSTERS : null,
    drive: ok('upload_drive') ? (LIBRARY.find((l) => l.id === id)?.drive ?? [{ name: 'Jane Doe - resume.pdf', url: 'https://drive.google.com/file/d/demo' }, { name: 'Jane Doe - resume.docx', url: 'https://drive.google.com/file/d/demo2' }]) : null,
    audit: ok('build_payload') ? AUDIT : { redacted_payload: { system: '', user: '' }, token_counts: {} },
    files: [...(ok('render_docx') ? ['resume.docx'] : []), ...(ok('render_pdf') ? ['resume.pdf'] : [])],
    original: resume, tailored: ok('restore') ? withEdits(id, TAILORED) : null, selection: null, ...learnExtras(id), ...jobExtras(ok('restore')), ...(ok('ai_review') ? reviewFor(overrides.get(id)) : {}),
  }
}

// PII ledger. The real values live only here, as the server would hold them, and leave only through revealPii.
const mask = (v: string) => v.split(' ').map((w) => (w.length <= 2 ? '••' : w.slice(0, 2) + '••••')).join(' ')
const realPii = new Map<string, string>()
const entry = (id: string, kind: PiiKind, token: string, value: string, source: PiiEntry['source'], occ: [PiiArea, number][]): PiiEntry => {
  realPii.set(id, value)
  return { id, kind, token, masked: mask(value), length: value.length, source, occurrences: occ.map(([area, count]) => ({ area, count })), total: occ.reduce((a, [, c]) => a + c, 0) }
}
const pii: PiiData = {
  entries: [
    entry('p1', 'person', 'PERSON_1', ORIGINAL.profile.name, 'profile', [['summary', 1], ['experience', 2], ['jd', 1]]),
    entry('p2', 'email', 'EMAIL_1', ORIGINAL.profile.email, 'profile', [['summary', 1]]),
    entry('p3', 'phone', 'PHONE_1', ORIGINAL.profile.phone, 'profile', [['summary', 1]]),
    entry('p4', 'org', 'ORG_1', 'Acme Robotics', 'experience', [['experience', 7], ['jd', 1]]),
    entry('p5', 'org', 'ORG_2', 'Northwind Automation', 'experience', [['experience', 2]]),
    entry('p6', 'client', 'CLIENT_1', 'Globex Logistics', 'experience', [['experience', 5], ['projects', 1], ['jd', 3]]),
  ],
  candidates: [], always: [],
}
const cands: { id: string; kind: 'email' | 'phone' | 'name' | 'url_handle'; value: string; where: PiiArea; count: number }[] = [
  { id: 'c1', kind: 'email', value: 'j.doe.consulting@example.test', where: 'experience', count: 1 },
  { id: 'c2', kind: 'phone', value: '+1 555 0142', where: 'experience', count: 1 },
]
pii.candidates = cands.map((c) => { realPii.set(c.id, c.value); return { id: c.id, kind: c.kind, masked: mask(c.value), where: c.where, count: c.count } })
const lag = () => new Promise((r) => setTimeout(r, 350))

export const demoApi = {
  openSocket: (since: number) => new FakeSocket(since),
  openEvents: (since: number) => new FakeEvents(since),
  getNotifications: async (): Promise<Notif[]> => notifs.map((n) => ({ ...n })),
  markRead: async (upto: number) => { notifs = notifs.map((n) => (n.seq <= upto ? { ...n, read: true } : n)); saveNotifs() },
  getPii: async (): Promise<PiiData> => { await lag(); return structuredClone(pii) },
  revealPii: async (id: string) => { await lag(); return { value: realPii.get(id) ?? '' } },
  addAlways: async (value: string, kind?: string) => {
    await lag()
    const id = 'a' + (pii.always.length + pii.entries.length + 1)
    realPii.set(id, value)
    pii.always.push({ id, kind: kind ?? 'custom', masked: mask(value) })
    const k = (['person', 'email', 'phone', 'org', 'client', 'custom'].includes(kind ?? '') ? kind : 'custom') as PiiKind
    pii.entries.push(entry(id, k, `${k === 'custom' ? 'CUSTOM' : k.toUpperCase()}_${pii.entries.filter((e) => e.kind === k).length + 1}`, value, 'always', [['experience', 1]]))
    return { id }
  },
  removeAlways: async (id: string) => { await lag(); pii.always = pii.always.filter((a) => a.id !== id); pii.entries = pii.entries.filter((e) => e.id !== id); realPii.delete(id) },
  acceptCandidate: async (id: string) => {
    await lag()
    const c = pii.candidates.find((x) => x.id === id); if (!c) return
    pii.candidates = pii.candidates.filter((x) => x.id !== id)
    const k: PiiKind = c.kind === 'email' ? 'email' : c.kind === 'phone' ? 'phone' : c.kind === 'name' ? 'person' : 'custom'
    pii.always.push({ id, kind: k, masked: c.masked })
    pii.entries.push({ id, kind: k, token: `${k === 'custom' ? 'CUSTOM' : k.toUpperCase()}_${pii.entries.filter((e) => e.kind === k).length + 1}`, masked: c.masked, length: (realPii.get(id) ?? '').length, source: 'always', occurrences: [{ area: c.where, count: c.count }], total: c.count })
  },
  dismissCandidate: async (id: string) => { await lag(); pii.candidates = pii.candidates.filter((x) => x.id !== id); realPii.delete(id) },
  getResume: async () => resume,
  putResume: async (r: Resume) => { resume = r },
  createJob: async (req: NewJob) => {
    const id = 'demo-' + Math.random().toString(36).slice(2, 8)
    const j: DJob = { meta: { id, status: 'queued', provider: req.provider, company: req.company || null, role: req.role || null, created_at: now(), updated_at: now(), error: null }, fail: (() => { const m = FAILS.find(([re]) => re.test(req.jd_text)); return m ? { code: m[1], step: m[2] } : null })(), steps: blankSteps(!!req.upload_drive || /\b(quota|nodrive)\b/i.test(req.jd_text)), events: [], failNext: false, timers: [], cancelled: false }
    jobs.set(id, j)
    later(j, 250, () => run(j))
    return { id }
  },
  listJobs: async () => [...jobs.values()].map((j) => ({ ...j.meta })).sort((a, b) => b.created_at.localeCompare(a.created_at)),
  getJob: async (id: string) => detail(id),
  retry: async (id: string, from?: string) => {
    const j = jobs.get(id)!
    const first = from ? j.steps.findIndex((s) => s.name === from) : j.steps.findIndex((s) => s.status === 'failed')
    const i = Math.max(0, first)
    j.cancelled = false; j.failNext = false; j.fail = null
    j.steps.forEach((s, k) => { if (k >= i) { s.status = 'pending'; s.error = null; s.started_at = s.finished_at = null } })
    touch(j, 'queued')
    later(j, 150, () => run(j, i))
  },
  setOverrides: async (id: string, o: Overrides) => {
    const j = jobs.get(id)!
    overrides.set(id, o); recordOverrides(id, o)
    j.timers.forEach(clearTimeout); j.timers = []; j.cancelled = false; j.fail = null
    const i = Math.max(0, j.steps.findIndex((s) => s.name === 'select'))
    j.steps.forEach((s, k) => { if (k >= i) { s.status = 'pending'; s.error = null; s.started_at = s.finished_at = null } })
    touch(j, 'queued')
    later(j, 150, () => run(j, i))
  },
  cancel: async (id: string) => {
    const j = jobs.get(id)!
    j.cancelled = true; j.timers.forEach(clearTimeout); j.timers = []
    j.steps.forEach((s) => { if (s.status === 'running') { s.status = 'failed'; s.error = 'Cancelled.' } })
    touch(j, 'cancelled')
    emit(j, 'finalize', 'warn', 'Job cancelled. Nothing further will be sent.')
  },
  subscribe(id: string, cb: (e: JobEvent) => void) {
    const j = jobs.get(id)
    if (!j) return () => {}
    j.events.forEach(cb) // replay, like Last-Event-ID: 0
    if (!subs.has(id)) subs.set(id, new Set())
    subs.get(id)!.add(cb)
    return () => { subs.get(id)?.delete(cb) }
  },
  setKey: async (p: KeyProvider, key: string) => { keys[p] = key ? { configured: true, source: 'stored' } : { configured: false, source: null } },
  deleteKey: async (p: KeyProvider) => { keys[p] = p === 'anthropic' ? { configured: true, source: 'env' } : { configured: false, source: null } },
  getKeys: async () => ({ gemini: { ...keys.gemini }, anthropic: { ...keys.anthropic } }),
  fetchJd: async (url: string): Promise<JdFetch> => {
    await new Promise((r) => setTimeout(r, 900))
    if (/login|linkedin|private/i.test(url)) throw new ApiError(422, 'login required')
    let host = 'example.test'
    try { host = new URL(url).hostname.replace(/^www\./, '') } catch { throw new ApiError(400, 'That does not look like a web address.') }
    return { text: DEMO_JD, title: 'Senior Robotics Software Engineer, Fleet Navigation', company: 'Globex Logistics', source: host }
  },
  getLibrary: async () => LIBRARY.map((l) => ({ ...l })).concat([...jobs.values()].filter((j) => !j.meta.id.startsWith('demo-seed') && j.meta.status === 'succeeded').map((j) => ({
    id: j.meta.id, company: j.meta.company ?? 'Untitled company', role: j.meta.role ?? 'Untitled role', created_at: j.meta.created_at, coverage: 0.68,
    files: [{ name: 'resume.pdf', display_name: 'Jane Doe.pdf' }, { name: 'resume.docx', display_name: 'Jane Doe.docx' }], drive: j.steps.some((s) => s.name === 'upload_drive') ? [{ name: 'Jane Doe.pdf', url: 'https://drive.google.com/file/d/demo' }] : null,
  }))).sort((a, b) => b.created_at.localeCompare(a.created_at)),
  importResume: async (f: File): Promise<ImportResult> => {
    await new Promise((r) => setTimeout(r, 1100))
    const low = /\.pdf$/i.test(f.name)
    const r = structuredClone(ORIGINAL)
    if (low) { r.profile.phone = ''; r.experience[2].dateLabel = ''; r.experience[0].bullets[3] = r.experience[0].bullets[3].slice(0, 48) }
    return low
      ? { resume: r, confidence: 0.54, warnings: ['No phone number was found.', 'The dates for Controls Engineer, Northwind Automation could not be read.', 'A bullet under Senior Robotics Software Engineer looks cut off at a page break.', 'Skills were found but not grouped. They were sorted into three groups by best guess.'] }
      : { resume: r, confidence: 0.88, warnings: ['Two-column layout detected. Reading order was inferred.'] }
  },
  getDrive: async () => ({ ...drive }),
  putDrive: async (name: string) => { drive.folder_name = name },
  putDriveOauth: async () => { drive.configured = true },
  driveConnect: async () => { setTimeout(() => { drive.connected = true; drive.account_email = 'jane.doe@example.test' }, 3500); return { auth_url: 'about:blank' } },
  driveDisconnect: async () => { drive.connected = false; drive.account_email = null },
  getStats: async (): Promise<Stats> => {
    const counts = [1, 0, 2, 1, 3, 2, 0, 1, 4, 2, 3, 1, 2, 0]
    const all = [...jobs.values()]
    const fresh = all.filter((j) => !j.meta.id.startsWith('demo-seed')).length
    const by_status: Record<string, number> = {}
    all.forEach((j) => { by_status[j.meta.status] = (by_status[j.meta.status] ?? 0) + 1 })
    by_status.succeeded = (by_status.succeeded ?? 0) + 17; by_status.failed = (by_status.failed ?? 0) + 2; by_status.cancelled = 1
    return {
      jobs_by_day: counts.map((c, i) => ({ day: new Date(Date.now() - (13 - i) * DAY).toISOString().slice(0, 10), count: c + (i === 13 ? fresh : 0) })),
      tokens_in: 148_320 + fresh * 9_400, tokens_out: 36_870 + fresh * 2_100, avg_coverage: 0.64, by_status,
    }
  },
}

// Mobile companion (fake data only)
const FP = '3f9a1c7be2d84056a1b3c9d0e7f2481a5c6b0d9e8f7a6b5c4d3e2f1a0b9c8d7e'
const rem = { on: false, devices: [{ id: 'demo-dev-1', name: 'Pixel 9 (demo)', created_at: Date.now() - 3 * DAY, last_seen: Date.now() - 120_000 }] }
const remStatus = () => ({ enabled: rem.on, port: rem.on ? 8788 : null, fingerprint: FP, hostname: 'demo-desktop', addresses: [{ ip: '192.168.1.20', kind: 'lan' as const }, { ip: '100.101.102.103', kind: 'tailscale' as const }], devices: rem.devices.length, cert_stale: false })
export const demoRemote = {
  status: async () => remStatus(),
  toggle: async (on: boolean) => { rem.on = on; return remStatus() },
  pairCode: async () => { const code = 'K7QM4XD9'; return { code, expires_at: Date.now() + 300_000, qr_payload: { v: 1 as const, hosts: ['192.168.1.20', '100.101.102.103', 'demo-desktop'], port: 8788, fp: FP, code } } },
  devices: async () => rem.devices.map((d) => ({ ...d })),
  revoke: async (id: string) => { rem.devices = rem.devices.filter((d) => d.id !== id) },
  audit: async () => [
    { ts: Date.now() - 120_000, device_id: 'demo-dev-1', device: 'Pixel 9 (demo)', method: 'GET', path: '/api/jobs', status: 200 },
    { ts: Date.now() - 125_000, device_id: 'demo-dev-1', device: 'Pixel 9 (demo)', method: 'POST', path: '/api/jobs/:id/approve', status: 200 },
    { ts: Date.now() - 400_000, device_id: 'demo-dev-1', device: 'Pixel 9 (demo)', method: 'GET', path: '(denied)', status: 403 },
  ],
}
