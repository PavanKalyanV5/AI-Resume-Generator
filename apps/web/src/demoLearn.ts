// Fake learning fixtures (Jane Doe, Acme Robotics): playbook, corrections, approvals, regression, metrics. In memory only.
import { ApiError } from './api'
import { TAILORED } from './fixtures'
import { reviewFor } from './demoReview'
import type { Correction, LearnMetrics, Overrides, Playbook, PlaybookRule, RegressionRun, Resume, RuleBody, RuleStatus } from './types'

const T0 = Date.now() - 6 * 864e5
const seed = (id: string, area: string, text: string, action: PlaybookRule['action'], matcher: PlaybookRule['matcher'], hits: number): PlaybookRule => ({
  id, status: 'active', scope: 'global', area, condition: {}, matcher, action, text, origin: 'seed', support_count: 0, version: 1, examples: [], impact: null, created_at: T0, updated_at: T0, hits, badge: 'From your 2026-10-03 feedback',
})
let rules: PlaybookRule[] = [
  seed('seed-cert-basics', 'certs', 'Avoid language-specific basics, training-programme, DSA and bootcamp certificates unless the job description asks for them.', { forbid: true }, { kind: 'cert', title_regex: '(?i)(basic|pointers|dsa|bootcamp)' }, 4),
  seed('seed-cert-cloud', 'certs', 'For cloud and DevOps jobs prefer recognised cloud certificates (AWS, Google Cloud, Azure).', { boost: 0.25 }, { kind: 'cert', issuer_in: ['aws', 'google', 'azure'], domain_bucket: 'cloud' }, 3),
  seed('seed-proj-featured', 'projects', 'Prefer featured-tier projects.', { boost: 0.15 }, { kind: 'project', tier: 'featured' }, 6),
  seed('seed-proj-frontend', 'projects', 'Leave out compact UI-clone and frontend-only projects unless the job is frontend or fullstack.', { forbid: true }, { kind: 'project', frontend_only: true }, 2),
  seed('seed-summary', 'summary', 'Summary: 3-4 lines of 22-32 words each, with the key terms in bold (**term**); line 1 contains the role label and the allowed years phrase.', {}, {}, 0),
  { id: 'mined-demo1', status: 'proposed', scope: 'family:devops_cloud', area: 'certs', condition: {}, matcher: { kind: 'cert', issuer_in: ['acme cloud academy'] }, action: { boost: -0.5 }, text: 'For devops_cloud JDs you removed Acme Cloud Academy certs 3 times: avoid them', origin: 'mined', support_count: 3, version: 1, examples: ['demo-seed-1', 'demo-seed-2', 'demo-seed-5'], impact: null, created_at: T0 + 5 * 864e5, updated_at: T0 + 5 * 864e5, hits: 0, badge: null },
  { id: 'user-demo1', status: 'disabled', scope: 'global', area: 'projects', condition: {}, matcher: { kind: 'project', tech_any: ['kubernetes'] }, action: { boost: 0.2 }, text: 'Prefer Kubernetes projects.', origin: 'user', support_count: 0, version: 2, examples: [], impact: null, created_at: T0 + 2 * 864e5, updated_at: T0 + 3 * 864e5, hits: 0, badge: null },
]
let versions = [{ version: 3, ts: T0 + 3 * 864e5, note: 'disable rule user-demo1' }, { version: 2, ts: T0 + 2 * 864e5, note: 'added rule user-demo1' }, { version: 1, ts: T0, note: 'seed rules' }]
const snap: Record<number, PlaybookRule[]> = { 1: rules.slice(0, 5), 2: rules.slice(0, 5).concat(rules[6]), 3: rules.slice(0, 5).concat(rules[6]) }
const state = (): Playbook => ({ rules: structuredClone(rules), groups: (['proposed', 'active', 'disabled', 'rejected'] as RuleStatus[]).reduce((g, s) => ({ ...g, [s]: rules.filter((r) => r.status === s).map((r) => r.id) }), {} as Playbook['groups']), versions: [...versions] })
const bump = (note: string) => { const v = (versions[0]?.version ?? 0) + 1; versions = [{ version: v, ts: Date.now(), note }, ...versions]; snap[v] = structuredClone(rules) }
const find = (id: string) => rules.find((r) => r.id === id) ?? (() => { throw new ApiError(404, 'no such rule') })()
const check = (b: RuleBody) => {
  if (!b.text.trim() || b.text.length > 300) throw new ApiError(400, 'text must be 1-300 characters')
  if (/@|https?:\/\/|(\d[\s-]?){7,}/.test(b.text)) throw new ApiError(400, 'rule text must not contain emails, links or phone-like numbers')
  if (b.matcher.title_regex) try { new RegExp(b.matcher.title_regex.replace(/^\(\?i\)/, '')) } catch { throw new ApiError(400, 'title_regex does not compile') }
  if (b.area !== 'summary' && b.area !== 'bullets') {
    const a = b.action
    if ([a.boost != null, a.forbid, a.require, a.cap != null].filter(Boolean).length !== 1) throw new ApiError(400, 'action needs exactly one of boost, forbid, require, cap, prefer_over')
    if (Object.values({ ...b.matcher, kind: null }).every((v) => v == null || (Array.isArray(v) && !v.length))) throw new ApiError(400, 'matcher is too broad: give at least one field')
  }
}

// ---- per-job corrections, edits, approvals ----
let cid = 10
const corr: Correction[] = [
  { id: 1, job_id: 'demo-seed-1', ts: T0 + 5e8, area: 'certs', action: 'remove', item_id: 'c4', item: { kind: 'cert', title: 'Docker Certified Associate' }, before: { selected: true }, after: { selected: false }, reason_tags: ['already_covered'], jd_family: 'devops_cloud', seniority: 'senior', company: 'Globex Logistics', weight: 1, source: 'owner' },
  { id: 2, job_id: 'demo-seed-1', ts: T0 + 5e8, area: 'projects', action: 'add', item_id: 'p4', item: { kind: 'project', title: 'ROS 2 Nav Tuning Kit' }, before: { selected: false }, after: { selected: true }, reason_tags: [], jd_family: 'devops_cloud', seniority: 'senior', company: 'Globex Logistics', weight: 1, source: 'owner' },
]
const edits = new Map<string, Map<string, string>>()
const approved = new Set<string>(['demo-seed-2', 'demo-seed-3'])
const titles: Record<string, string> = { p1: 'Fleet Rollout Operator', p2: 'Support Agent with Tool Use', p3: 'RAG over Robot Logs', p4: 'ROS 2 Nav Tuning Kit', p5: 'Kanban Board Clone', p6: 'Music Player UI Clone', c1: 'Certified Kubernetes Administrator', c2: 'AWS Solutions Architect Associate', c3: 'ROS 2 Developer', c4: 'Docker Certified Associate', c5: 'Python Professional', c6: 'Terraform Associate', c7: 'Agile Practitioner' }

export function withEdits(id: string, r: Resume): Resume {
  const e = edits.get(id)
  if (!e) return r
  const c = structuredClone(r)
  e.forEach((t, p) => { const m = /^e(\d+)\.b(\d+)$/.exec(p); if (p.startsWith('summary.')) c.summary[+p.slice(8)] = t; else if (m) c.experience[+m[1]].bullets[+m[2]] = t })
  return c
}
export const learnExtras = (id: string) => ({ corrections: corr.filter((c) => c.job_id === id).map((c) => ({ ...c })), rules_applied: id === 'demo-seed-3' ? [] : [{ rule_id: 'seed-proj-frontend', text: rules[3].text, effect: 'avoided Kanban Board Clone; avoided Music Player UI Clone' }, { rule_id: 'seed-cert-cloud', text: rules[1].text, effect: '+0.25 score: AWS Solutions Architect Associate' }], approved: approved.has(id) })
export function recordOverrides(id: string, o: Overrides) {
  const base = reviewFor(undefined).pool
  for (const [area, list, key] of [['projects', base.projects, 'projects'], ['certs', base.certs, 'certs']] as const) {
    const want = o[key]
    if (!want) continue
    for (const x of list) {
      const now = want.includes(x.id)
      if (now !== x.selected) corr.push({ id: ++cid, job_id: id, ts: Date.now(), area, action: now ? 'add' : 'remove', item_id: x.id, item: { kind: area === 'certs' ? 'cert' : 'project', title: titles[x.id] }, before: { selected: x.selected }, after: { selected: now }, reason_tags: [], jd_family: 'devops_cloud', seniority: 'senior', company: '', weight: 1, source: 'owner' })
    }
  }
}

const lag = () => new Promise((r) => setTimeout(r, 200))
const IMPACT = { would_change: 1, of: 3, cases: ['demo-seed-2'], summary: 'would change 1 of 3 approved resumes' }
export const demoLearn = {
  playbook: async () => (await lag(), state()),
  op: async (id: string, op: string) => {
    await lag()
    const r = find(id)
    const to = ({ approve: 'active', reject: 'rejected', disable: 'disabled', enable: 'active' } as Record<string, RuleStatus>)[op]
    if (!to) throw new ApiError(404, 'unknown operation')
    const ok = (op === 'approve' && r.status === 'proposed') || (op === 'reject' && r.status === 'proposed') || (op === 'disable' && r.status === 'active') || (op === 'enable' && (r.status === 'disabled' || r.status === 'rejected'))
    if (!ok) throw new ApiError(409, `cannot ${op} a ${r.status} rule`)
    r.status = to; r.version++
    if (to === 'active') r.impact = { ...IMPACT }
    bump(`${op} rule ${id}`)
    return { rule: structuredClone(r), impact: to === 'active' ? r.impact : null }
  },
  create: async (b: RuleBody) => {
    await lag(); check(b)
    const r: PlaybookRule = { ...b, id: 'user-' + Math.random().toString(36).slice(2, 8), status: 'active', condition: {}, origin: 'user', support_count: 0, version: 1, examples: [], impact: null, created_at: Date.now(), updated_at: Date.now(), hits: 0, badge: null }
    rules = [...rules, r]; bump(`added rule ${r.id}`)
    return structuredClone(r)
  },
  put: async (id: string, b: RuleBody) => { await lag(); check(b); const r = find(id); Object.assign(r, b, { version: r.version + 1, updated_at: Date.now() }); bump(`edited rule ${id}`); return structuredClone(r) },
  del: async (id: string) => { await lag(); if (find(id).origin === 'seed') throw new ApiError(409, 'seed rules can be disabled, not deleted'); rules = rules.filter((r) => r.id !== id); bump(`deleted rule ${id}`) },
  rollback: async (version: number) => {
    await lag()
    const s = snap[version]; if (!s) throw new ApiError(404, 'no such version')
    rules = [...structuredClone(s), ...rules.filter((r) => (r.status === 'proposed' || r.status === 'rejected') && !s.some((x) => x.id === r.id))]
    bump(`rolled back to v${version}`)
    return state()
  },
  reason: async (id: number, tags: string[]) => { const c = corr.find((x) => x.id === id); if (!c) throw new ApiError(404, 'no such correction'); c.reason_tags = tags; return { id, reason_tags: tags } },
  corrections: async (job: string) => corr.filter((c) => c.job_id === job).map((c) => ({ ...c })),
  edit: async (job: string, path: string, text: string, reason_tags: string[] = []) => {
    await lag()
    if (!/^(summary\.\d+|e\d+\.b\d+)$/.test(path)) throw new ApiError(400, 'path must be summary.<n> or e<i>.b<j>')
    if (!text.trim()) throw new ApiError(400, 'text must be 1-2000 characters')
    const m0 = /^e(\d+)\.b(\d+)$/.exec(path)
    const t = withEdits(job, TAILORED)
    const old = (m0 ? t.experience[+m0[1]]?.bullets[+m0[2]] : t.summary[+path.slice(8)]) ?? ''
    const m = edits.get(job) ?? new Map(); m.set(path, text.trim()); edits.set(job, m)
    const id = ++cid
    corr.push({ id, job_id: job, ts: Date.now(), area: path.startsWith('summary') ? 'summary' : 'bullets', action: 'edit', item_id: path, item: { kind: 'summary', path }, before: { text: old }, after: { text }, reason_tags, jd_family: 'devops_cloud', seniority: 'senior', company: '', weight: 1, source: 'owner' })
    return { correction_id: id, path, before: old, after: text.trim() }
  },
  approve: async (job: string) => { await lag(); approved.add(job); return { approved: true, golden: {}, keep_corrections: 5 } },
  latest: async (): Promise<RegressionRun | null> => reg,
  run: async (): Promise<RegressionRun> => { await new Promise((r) => setTimeout(r, 600)); return (reg = { ...REG, ran_at: Date.now() }) },
  metrics: async (): Promise<LearnMetrics> => {
    const per = [4, 3, 3, 1, 1, 0]
    return { corrections_per_job_trend: per, jobs: per.map((c, i) => ({ job_id: `demo-seed-${i + 1}`, ts: T0 + i * 864e5, company: ['Globex Logistics', 'Initech', 'Northwind Automation', 'Globex Logistics', 'Acme Cloud', 'Initech'][i], corrections: c, approved: i < 3 || i === 5, tokens_in: 4200, tokens_out: 900 })),
      approval_rate: 4 / 6, active_rules: rules.filter((r) => r.status === 'active').length, proposed_rules: rules.filter((r) => r.status === 'proposed').length, rule_hits: 15, regression: { cases: 3, passing: 2 }, tokens_saved_est: 18400 }
  },
}
const REG: RegressionRun = { ran_at: 0, cases: 3, passing: 2, results: [
  { job_id: 'demo-seed-2', pass: true, diff: {} }, { job_id: 'demo-seed-3', pass: true, diff: {} },
  { job_id: 'demo-seed-1', pass: false, diff: { certs: { missing: ['Docker Certified Associate'], extra: ['Terraform Associate'], order_only: false } } },
] }
let reg: RegressionRun | null = { ...REG, ran_at: Date.now() - 36e5 }
