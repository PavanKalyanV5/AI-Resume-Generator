import { demoApi, demoRemote } from './demo'
import { demoLearn } from './demoLearn'
import { demoApps, demoHeur, ML, RECS, PROFILE } from './demoMl'
import type { AppStage, Application, DriveState, Funnel, HeurVals, Heuristics, MlCards, ProfileFacts, Recs, ImportResult, JdFetch, JobDetail, JobEvent, JobMeta, KeyProvider, Keys, LearnMetrics, LibraryItem, Correction, Playbook, PlaybookRule, RegressionRun, RuleBody, LinkState, NewJob, Notif, Overrides, PiiData, Resume, Stats, RemoteStatus, RemoteDevice, RemoteAudit, PairCode } from './types'

export interface CompanionStatus { paired: boolean; host: string | null; device_name: string | null; fingerprint_short: string | null }
/** `companion` is set when this page is served by the phone app's local proxy (GET /companion/status answers JSON). */
export const mode: { demo: boolean; companion: CompanionStatus | null } = { demo: false, companion: null }
const demoCo = (paired: boolean): CompanionStatus => ({ paired, host: paired ? '192.168.1.20' : null, device_name: paired ? 'Pixel 9 (demo)' : null, fingerprint_short: paired ? '3f9a1c7be2d84056' : null })

export async function initMode(): Promise<boolean> {
  const q = new URLSearchParams(location.search)
  if (q.get('demo') === '1') {
    if (q.get('companion') === '1') mode.companion = demoCo(q.get('paired') !== '0')
    return (mode.demo = true)
  }
  try {
    const r = await fetch('/companion/status') // the SPA fallback answers HTML on normal builds
    if (r.ok && (r.headers.get('content-type') ?? '').includes('json')) { mode.companion = await r.json(); return false }
  } catch { /* not a companion build */ }
  try {
    const ctl = new AbortController()
    const t = setTimeout(() => ctl.abort(), 2500)
    const r = await fetch('/api/health', { signal: ctl.signal })
    clearTimeout(t)
    mode.demo = !r.ok || (r.headers.get('content-type') ?? '').includes('text/html')
  } catch { mode.demo = true }
  return mode.demo
}

export class ApiError extends Error { constructor(public status: number, msg: string, public code?: string, public hint?: string) { super(msg) } }
async function http<T>(path: string, init?: RequestInit): Promise<T> {
  const r = await fetch((path.startsWith('/companion') ? '' : '/api') + path, { ...init, headers: { ...(init?.body && typeof init.body === 'string' ? { 'Content-Type': 'application/json' } : {}), ...(init?.headers as Record<string, string> | undefined) } })
  if (!r.ok) {
    let msg = r.statusText, code: string | undefined, hint: string | undefined
    const raw = await r.text() // the server answers some errors with plain text
    try {
      const b = JSON.parse(raw), e = typeof b.error === 'object' && b.error ? b.error : b
      msg = (typeof b.error === 'string' ? b.error : e.message) ?? b.message ?? msg; code = e.code ?? b.code; hint = e.hint ?? b.hint
    } catch { msg = raw.trim().slice(0, 300) || msg }
    throw new ApiError(r.status, msg || `Request failed (${r.status})`, code, hint)
  }
  return r.status === 204 || r.status === 202 ? (undefined as T) : r.json()
}
const body = (b: unknown, method = 'POST'): RequestInit => ({ method, body: JSON.stringify(b) })

function sse(id: string, onEvent: (e: JobEvent) => void, onState: (s: LinkState) => void, stop: () => boolean) {
  let last = 0, es: EventSource | null = null, closed = false, tries = 0, timer = 0
  const open = () => {
    // The browser resends Last-Event-ID itself on its own retries. After a hard close we reopen
    // manually and pass the id as a query parameter as well.
    es = new EventSource(`/api/jobs/${id}/events${last ? `?last_event_id=${last}` : ''}`)
    es.onopen = () => { tries = 0; onState('live') }
    es.onmessage = (m) => {
      try { const e = JSON.parse(m.data) as JobEvent; if (e.seq > last) { last = e.seq; onEvent(e) } } catch { /* ignore malformed */ }
    }
    es.onerror = () => {
      if (closed) return
      if (stop()) { es?.close(); return }
      onState('reconnecting')
      if (es?.readyState === EventSource.CLOSED) timer = window.setTimeout(open, Math.min(1000 * 2 ** tries++, 15000))
    }
  }
  open()
  return () => { closed = true; clearTimeout(timer); es?.close() }
}

export const api = {
  getResume: (): Promise<Resume> => (mode.demo ? demoApi.getResume() : http('/resume')),
  putResume: (r: Resume): Promise<void> => (mode.demo ? demoApi.putResume(r) : http('/resume', body(r, 'PUT'))),
  createJob: (j: NewJob): Promise<{ id: string }> => (mode.demo ? demoApi.createJob(j) : http('/jobs', body(j))),
  listJobs: (): Promise<JobMeta[]> => (mode.demo ? demoApi.listJobs() : http('/jobs')),
  getJob: (id: string): Promise<JobDetail> => (mode.demo ? demoApi.getJob(id) : http<JobDetail>(`/jobs/${id}`).then(normalizeJob)),
  retry: (id: string, from?: string): Promise<void> => (mode.demo ? demoApi.retry(id, from) : http(`/jobs/${id}/retry${from ? `?from=${from}` : ''}`, { method: 'POST' })),
  setOverrides: (id: string, o: Overrides): Promise<void> => (mode.demo ? demoApi.setOverrides(id, o) : http(`/jobs/${id}/overrides`, body(o))),
  cancel: (id: string): Promise<void> => (mode.demo ? demoApi.cancel(id) : http(`/jobs/${id}/cancel`, { method: 'POST' })),
  subscribe: (id: string, onEvent: (e: JobEvent) => void, onState: (s: LinkState) => void, stop: () => boolean) =>
    mode.demo ? demoApi.subscribe(id, onEvent) : sse(id, onEvent, onState, stop),
  fileUrl: (id: string, name: string) => `/api/jobs/${id}/files/${name}`,
  setKey: (p: KeyProvider, key: string): Promise<void> => (mode.demo ? demoApi.setKey(p, key) : http(`/keys/${p}`, body({ key }, 'PUT'))),
  deleteKey: (p: KeyProvider): Promise<void> => (mode.demo ? demoApi.deleteKey(p) : http(`/keys/${p}`, { method: 'DELETE' })),
  getKeys: (): Promise<Keys> => (mode.demo ? demoApi.getKeys() : http<Keys>('/keys')),
  fetchJd: (url: string): Promise<JdFetch> => (mode.demo ? demoApi.fetchJd(url) : http('/jd/fetch', body({ url }))),
  getLibrary: (): Promise<LibraryItem[]> => (mode.demo ? demoApi.getLibrary() : http<LibraryItem[]>('/library').then((l) => l.map((i) => ({ ...i, created_at: typeof i.created_at === 'number' ? new Date(i.created_at).toISOString() : i.created_at })))),
  importResume: (f: File): Promise<ImportResult> => (mode.demo ? demoApi.importResume(f) : http('/import', { method: 'POST', body: f, headers: { 'x-filename': encodeURIComponent(f.name) } })),
  getPii: (): Promise<PiiData> => (mode.demo ? demoApi.getPii() : http<PiiData>('/pii')),
  /** The only call that returns a real value. Explicit header so it cannot be triggered by a stray request. */
  revealPii: (id: string): Promise<{ value: string }> => (mode.demo ? demoApi.revealPii(id) : http('/pii/reveal', { method: 'POST', body: JSON.stringify({ id }), headers: { 'x-confirm': 'reveal' } })),
  addAlways: (value: string, kind?: string): Promise<{ id: string }> => (mode.demo ? demoApi.addAlways(value, kind) : http('/pii/always', body({ value, ...(kind ? { kind } : {}) }))),
  removeAlways: (id: string): Promise<void> => (mode.demo ? demoApi.removeAlways(id) : http(`/pii/always/${encodeURIComponent(id)}`, { method: 'DELETE' })),
  acceptCandidate: (id: string): Promise<void> => (mode.demo ? demoApi.acceptCandidate(id) : http(`/pii/candidates/${encodeURIComponent(id)}/accept`, { method: 'POST' })),
  dismissCandidate: (id: string): Promise<void> => (mode.demo ? demoApi.dismissCandidate(id) : http(`/pii/candidates/${encodeURIComponent(id)}/dismiss`, { method: 'POST' })),
  getNotifications: (): Promise<Notif[]> => (mode.demo ? demoApi.getNotifications() : http<Notif[]>('/notifications').then((l) => l.map((n) => ({ ...n, ts: typeof n.ts === 'number' ? new Date(n.ts).toISOString() : n.ts })))),
  markNotificationsRead: (upto_seq: number): Promise<void> => (mode.demo ? demoApi.markRead(upto_seq) : http('/notifications/read', body({ upto_seq }))),
  getDrive: (): Promise<DriveState> => (mode.demo ? demoApi.getDrive() : http('/drive')),
  putDrive: (folder_name: string): Promise<void> => (mode.demo ? demoApi.putDrive(folder_name) : http('/drive', body({ folder_name }, 'PUT'))),
  putDriveOauth: (client_id: string, client_secret: string): Promise<void> => (mode.demo ? demoApi.putDriveOauth() : http('/drive/oauth', body({ client_id, client_secret }, 'PUT'))),
  driveConnect: (): Promise<{ auth_url: string }> => (mode.demo ? demoApi.driveConnect() : http('/drive/connect', { method: 'POST' })),
  driveDisconnect: (): Promise<void> => (mode.demo ? demoApi.driveDisconnect() : http('/drive/disconnect', { method: 'POST' })),
  getMlCards: (): Promise<MlCards> => (mode.demo ? Promise.resolve(structuredClone(ML)) : http('/ml/cards')),
  retrain: (): Promise<void> => (mode.demo ? Promise.resolve() : http('/ml/retrain', { method: 'POST' })),
  getRecs: (): Promise<Recs> => (mode.demo ? Promise.resolve(structuredClone(RECS)) : http('/ml/recommendations')),
  getApps: (): Promise<Application[]> => (mode.demo ? demoApps.list() : http('/applications')),
  trackApp: (job_id: string, stage: AppStage, note?: string): Promise<void> => (mode.demo ? demoApps.upsert(job_id, stage, note) : http('/applications', body({ job_id, stage, ...(note !== undefined ? { note } : {}) }))),
  getFunnel: (): Promise<Funnel> => (mode.demo ? demoApps.funnel() : http('/applications/funnel')),
  getProfileFacts: (): Promise<ProfileFacts> => (mode.demo ? Promise.resolve(structuredClone(PROFILE)) : http('/profile/facts')),
  feedback: (id: string, bullet_id: string, action: 'keep' | 'remove' | 'add_back' | 'edit'): Promise<void> => (mode.demo ? Promise.resolve() : http(`/jobs/${id}/feedback`, body({ bullet_id, action }))),
  setJdClass: (id: string, family: string, seniority: string): Promise<void> => (mode.demo ? Promise.resolve() : http(`/jobs/${id}/jd-class`, body({ family, seniority }))),
  getHeuristics: (): Promise<Heuristics> => (mode.demo ? demoHeur.get() : http('/heuristics')),
  putHeuristics: (patch: HeurVals): Promise<Heuristics> => (mode.demo ? demoHeur.put(patch) : http('/heuristics', body(patch, 'PUT'))),
  resetHeuristics: (): Promise<Heuristics> => (mode.demo ? demoHeur.reset() : http('/heuristics', { method: 'DELETE' })),
  getPriorPages: (): Promise<number | null> => (mode.demo ? demoHeur.getSettings().then((s) => s.prior_resume_pages) : http<{ prior_resume_pages: number | null }>('/settings').then((s) => s.prior_resume_pages)),
  setPriorPages: (n: number | null): Promise<void> => (mode.demo ? demoHeur.putSettings(n) : http('/settings', body({ prior_resume_pages: n }, 'PUT'))),
  getPlaybook: (): Promise<Playbook> => (mode.demo ? demoLearn.playbook() : http('/playbook')),
  ruleOp: (id: string, op: 'approve' | 'reject' | 'disable' | 'enable'): Promise<{ rule: PlaybookRule; impact: PlaybookRule['impact'] }> => (mode.demo ? demoLearn.op(id, op) : http(`/playbook/rules/${encodeURIComponent(id)}/${op}`, { method: 'POST' })),
  createRule: (b: RuleBody): Promise<PlaybookRule> => (mode.demo ? demoLearn.create(b) : http('/playbook/rules', body(b))),
  putRule: (id: string, b: RuleBody): Promise<PlaybookRule> => (mode.demo ? demoLearn.put(id, b) : http(`/playbook/rules/${encodeURIComponent(id)}`, body(b, 'PUT'))),
  deleteRule: (id: string): Promise<unknown> => (mode.demo ? demoLearn.del(id) : http(`/playbook/rules/${encodeURIComponent(id)}`, { method: 'DELETE' })),
  rollback: (version: number): Promise<Playbook> => (mode.demo ? demoLearn.rollback(version) : http('/playbook/rollback', body({ version }))),
  setReason: (id: number, tags: string[]): Promise<unknown> => (mode.demo ? demoLearn.reason(id, tags) : http(`/corrections/${id}/reason`, body({ tags }))),
  getCorrections: (job: string): Promise<Correction[]> => (mode.demo ? demoLearn.corrections(job) : http(`/jobs/${job}/corrections`)),
  editLine: (job: string, path: string, text: string, reason_tags?: string[]): Promise<{ correction_id: number }> => (mode.demo ? demoLearn.edit(job, path, text, reason_tags) : http(`/jobs/${job}/edit`, body({ path, text, ...(reason_tags?.length ? { reason_tags } : {}) }))),
  approveJob: (job: string): Promise<unknown> => (mode.demo ? demoLearn.approve(job) : http(`/jobs/${job}/approve`, { method: 'POST' })),
  runRegression: (): Promise<RegressionRun> => (mode.demo ? demoLearn.run() : http('/regression/run', { method: 'POST' })),
  latestRegression: (): Promise<RegressionRun | null> => (mode.demo ? demoLearn.latest() : http('/regression/latest')),
  getLearning: (): Promise<LearnMetrics> => (mode.demo ? demoLearn.metrics() : http('/learning/metrics')),
  remoteStatus: (): Promise<RemoteStatus> => (mode.demo ? demoRemote.status() : http('/remote')),
  remoteToggle: (enabled: boolean): Promise<RemoteStatus> => (mode.demo ? demoRemote.toggle(enabled) : http('/remote', body({ enabled }))),
  remotePairCode: (): Promise<PairCode> => (mode.demo ? demoRemote.pairCode() : http('/remote/pair-code', { method: 'POST' })),
  remoteDevices: (): Promise<RemoteDevice[]> => (mode.demo ? demoRemote.devices() : http('/remote/devices')),
  remoteRevoke: (id: string): Promise<void> => (mode.demo ? demoRemote.revoke(id) : http(`/remote/devices/${encodeURIComponent(id)}`, { method: 'DELETE' })),
  remoteAudit: (): Promise<RemoteAudit[]> => (mode.demo ? demoRemote.audit() : http('/remote/audit?limit=15')),
  companionPair: async (b: { payload?: string; host?: string; port?: string; fp?: string; code?: string; device_name: string }): Promise<CompanionStatus> => {
    if (mode.demo) { await new Promise((r) => setTimeout(r, 500)); if (b.code?.toUpperCase() === 'WRONGCOD') throw new ApiError(403, 'x', 'pair.rejected'); return (mode.companion = demoCo(true)) }
    const r = await http<CompanionStatus>('/companion/pair', body(b)); return (mode.companion = r)
  },
  companionUnpair: async (): Promise<void> => { if (!mode.demo) await http('/companion/unpair', { method: 'DELETE' }); mode.companion = demoCo(false) },
  /** Text shared into the app from the Android share sheet; handed out once. */
  companionShare: async (): Promise<string | null> => {
    if (mode.demo) return new URLSearchParams(location.search).get('share')
    return (await http<{ text: string | null }>('/companion/share')).text
  },
  /** Cheap reachability probe for the paired desktop. Returns the error code, or null when fine. */
  companionHealth: async (): Promise<string | null> => {
    if (mode.demo) return new URLSearchParams(location.search).get('down') === '1' ? 'server.unreachable' : null
    try {
      const r = await fetch('/api/health')
      if (r.ok) return null
      if (r.status === 401) return 'companion.revoked'
      return (await r.json().catch(() => ({}))).code ?? 'server.unreachable'
    } catch { return 'server.unreachable' }
  },
  getStats: (): Promise<Stats> => (mode.demo ? demoApi.getStats() : http<Stats>('/stats').then(normalizeStats)),
}

// The Rust API serialises enum variants PascalCase; the charts use lowercase kinds.
const NODE_KIND: Record<string, string> = { JdRequirement: 'requirement', Client: 'org', Certification: 'cert' }
const EDGE_KIND: Record<string, string> = { AtOrg: 'at', ForClient: 'at', Supports: 'mentions' }
const lower = (s: string) => s.toLowerCase()
function normalizeJob(j: JobDetail): JobDetail {
  const files = ((j.files ?? []) as unknown[]).map((f) => (typeof f === 'string' ? f : String((f as { name?: string }).name ?? '')))
  const coverage = { ...j.coverage, semantic: j.coverage?.semantic ?? [] }
  const base = { ...j, files, coverage, drive: j.drive ?? null, clusters: j.clusters ?? null }
  if (!j.graph) return base
  return {
    ...base,
    graph: {
      nodes: j.graph.nodes.map((n) => ({ ...n, kind: NODE_KIND[n.kind] ?? lower(n.kind) })),
      edges: j.graph.edges.map((e) => ({ ...e, kind: EDGE_KIND[e.kind] ?? lower(e.kind) })),
    },
  }
}

// Server sends jobs_by_day as {day: count}; the UI wants [{day, count}].
function normalizeStats(s: Stats): Stats {
  const d = s.jobs_by_day as unknown
  return Array.isArray(d) ? s : { ...s, jobs_by_day: Object.entries(d as Record<string, number>).sort().map(([day, count]) => ({ day, count })) }
}
