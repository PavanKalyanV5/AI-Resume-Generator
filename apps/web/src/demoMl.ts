// Demo-mode fixtures for the ML, applications, profile and heuristics endpoints. Fake data only (Jane Doe).
import type { AppStage, Application, Funnel, Heuristics, HeurVals, JobDetail, MlCards, ProfileFacts, Recs } from './types'

const iso = (d: number) => new Date(Date.now() - d * 864e5).toISOString()
const curve = (a: number, b: number): [number, number][] => [10, 15, 20, 25, 30, 38].map((n, i) => [n, a + ((b - a) * i) / 5 + (i % 2 ? 0.01 : -0.01)])
const card = (name: string, status: 'ColdStart' | 'Learning' | 'Ready', n: number, metric_name: string, cv: number | null, baseline: number | null, top: [string, number][], notes: string, lc: [number, number][] = []) =>
  ({ name, status, n_samples: n, metric_name, metric_cv: cv, baseline, top_features: top, notes, learning_curve: lc, updated_at: n ? iso(1) : '' })

export const ML: MlCards = {
  cards: [
    card('bullet_ranker', 'Learning', 14, 'AUC (CV)', 0.71, 0.5, [['bm25_norm', 1.9], ['has_metric', 0.8], ['skill_prof', 0.6], ['verb_class', -0.3]], 'Learns keep/remove taste from explicit feedback; prior = hand-tuned select score. Blend share capped.', curve(0.55, 0.71)),
    card('rewrite_need', 'Ready', 36, 'Brier (CV, lower better)', 0.11, 0.25, [['potential_gain', 0.9], ['verb_strength', -0.5], ['length', -0.2]], 'Skips rewrites unlikely to be accepted; zero-gain bullets are always skipped.', curve(0.22, 0.11)),
    card('fit_model', 'ColdStart', 1, 'MAE lines (CV)', null, null, [['analytic_lines', 0.89], ['bullets', 0.01]], 'Predicts rendered line count; baseline = analytic estimate. Capacity per layout learned separately.'),
    card('jd_family', 'ColdStart', 0, 'online top-1 accuracy', null, 0.09, [], 'Seeded from keyword lists; n counts user confirmations/corrections; accuracy is measured before each update.'),
    card('outcome', 'ColdStart', 3, 'AUC (CV)', null, 0.5, [['coverage', 0.4], ['pages', 0.1]], 'Estimates response odds from your tracked applications. Needs about 25 to say anything useful.'),
  ],
  signals: { feedback: 14, applications: 3, jd_corpus: 6, job_vectors: 6, rewrite_samples: 36, ranker_samples: 14, pii_samples: 0, fit_observations: 1, outcome_labelled: 3, jd_corrections: 0 },
}

const months = ['2026-05', '2026-06', '2026-07', '2026-08', '2026-09', '2026-10']
export const RECS: Recs = {
  learn_next: [
    { skill: 'graphql', score: 0.44, demand_share: 0.67, adjacent_to: ['python', 'kubernetes'], why: 'In 67% of your saved JDs and often appears with python, kubernetes' },
    { skill: 'terraform', score: 0.41, demand_share: 0.5, adjacent_to: ['docker', 'kubernetes'], why: 'In 50% of your saved JDs and often appears with docker, kubernetes' },
    { skill: 'cuda', score: 0.22, demand_share: 0.33, adjacent_to: ['c++'], why: 'In 33% of your saved JDs and often appears with c++' },
  ],
  trends: [['graphql', months.map((m, i) => [m, [0, 1, 1, 2, 2, 3][i]])], ['terraform', months.map((m, i) => [m, [1, 1, 0, 2, 2, 2][i]])], ['rust', months.map((m, i) => [m, [0, 0, 1, 1, 2, 2][i]])], ['cuda', months.map((m, i) => [m, [0, 0, 0, 1, 1, 1][i]])]],
  resume_additions: [{ skill: 'ros 2', demand_share: 0.83, evidence: [{ kind: 'Role', index: 0, bullet: 3 }, { kind: 'Cert', index: 2, bullet: null }] }, { skill: 'slam', demand_share: 0.5, evidence: [{ kind: 'Role', index: 1, bullet: 0 }] }],
}

let apps: Application[] = [
  ['demo-seed-1', 'Globex Logistics', 'Senior Robotics Software Engineer', 'interview', 'Phone screen booked for Thursday.', 9],
  ['demo-seed-2', 'Northwind Automation', 'Robotics Platform Lead', 'applied', null, 4],
  ['demo-seed-3', 'Initech Labs', 'Staff Perception Engineer', 'rejected', 'Wanted more CUDA.', 21],
  ['demo-seed-5', 'Acme Robotics', 'Controls Software Engineer', 'ghosted', null, 40],
].map(([job_id, company, role, stage, note, d]) => ({ job_id, company, role, stage, note, applied_at: iso(d as number), updated_at: iso((d as number) / 2), history: [{ stage: 'applied', note: null, ts: iso(d as number) }, ...(stage !== 'applied' ? [{ stage, note, ts: iso((d as number) / 2) }] : [])] }) as Application)

export const demoApps = {
  list: async () => structuredClone(apps),
  upsert: async (job_id: string, stage: AppStage, note?: string) => {
    const a = apps.find((x) => x.job_id === job_id)
    const ts = new Date().toISOString()
    if (a) { a.stage = stage; a.note = note ?? a.note; a.updated_at = ts; a.history.push({ stage, note: note ?? null, ts }) }
    else apps = [{ job_id, company: 'Globex Logistics', role: 'Senior Robotics Software Engineer', stage, note: note ?? null, applied_at: ts, updated_at: ts, history: [{ stage, note: note ?? null, ts }] }, ...apps]
  },
  funnel: async (): Promise<Funnel> => {
    const c = (...s: AppStage[]) => apps.filter((a) => s.includes(a.stage)).length
    const total = apps.length, pending = c('applied'), resolved = total - pending
    const [responded, interviews, offers] = [c('response', 'interview', 'offer'), c('interview', 'offer'), c('offer')]
    const r = (n: number) => (resolved ? n / resolved : 0)
    return { total, pending, responded, interviews, offers, rejected: c('rejected'), ghosted: c('ghosted'), response_rate: r(responded), response_ci: resolved ? [Math.max(0, r(responded) - 0.3), Math.min(1, r(responded) + 0.3)] : [0, 1], interview_rate: r(interviews), offer_rate: r(offers) }
  },
}

const PF_SKILLS: [string, string, 'Beginner' | 'Working' | 'Strong' | 'Expert', number, number][] = [
  ['c++', 'Languages', 'Expert', 101, 0.88], ['python', 'Languages', 'Expert', 71, 0.84], ['kubernetes', 'Infra', 'Strong', 68, 0.74], ['ros 2', 'Robotics', 'Strong', 52, 0.66], ['docker', 'Infra', 'Strong', 45, 0.61], ['slam', 'Robotics', 'Working', 33, 0.48], ['rust', 'Languages', 'Working', 12, 0.33], ['aws', 'Infra', 'Beginner', 0, 0.16],
]
const ym = (y: number, m: number): [number, number] => [y, m]
export const PROFILE: ProfileFacts = {
  headline: 'Senior Robotics Software Engineer',
  timeline: [
    { idx: 0, kind: 'FullTime', role: 'Senior Robotics Software Engineer', org: 'Acme Robotics', start: ym(2021, 3), end: ym(2026, 10), months: 68, is_current: true },
    { idx: 1, kind: 'FullTime', role: 'Robotics Software Engineer', org: 'Northwind Automation', start: ym(2018, 6), end: ym(2021, 2), months: 33, is_current: false },
    { idx: 2, kind: 'Internship', role: 'Software Engineering Intern', org: 'Initech Labs', start: ym(2017, 6), end: ym(2017, 8), months: 3, is_current: false },
  ],
  years: { fulltime: 8.4, internship: 0.25, other: 0, total_effective: 8.5 },
  skills: PF_SKILLS.map(([canonical, category, level, months_used, proficiency]) => ({ canonical, category, level, months_used, proficiency, mentions: 4 })),
  achievements: [
    { role_idx: 0, bullet_idx: 0, text: 'Cut path planning latency by 42% by rewriting the C++ planner for 250+ robots.', impact_score: 0.76, metrics: [{ raw: '42%', unit: 'Percent' }, { raw: '250+', unit: 'Count' }] },
    { role_idx: 0, bullet_idx: 2, text: 'Built a Python telemetry pipeline processing 3M events per day.', impact_score: 0.8, metrics: [{ raw: '3M', unit: 'Count' }] },
    { role_idx: 1, bullet_idx: 1, text: 'Automated CI with Docker and GitHub Actions, reducing release time by 60%.', impact_score: 0.49, metrics: [{ raw: '60%', unit: 'Percent' }] },
  ],
  domains: [{ name: 'Languages', skills: ['c++', 'python', 'rust'], weight: 0.46 }, { name: 'Infra', skills: ['kubernetes', 'docker', 'aws'], weight: 0.34 }, { name: 'Robotics', skills: ['ros 2', 'slam'], weight: 0.2 }],
  gaps_in_timeline: [[ym(2017, 9), ym(2018, 5), 9]],
  summary_stats: { roles: 3, bullets: 10, metrics: 7 },
  skill_timeline: [['c++', 2018, 6, 2026, 10], ['python', 2017, 6, 2017, 8], ['python', 2021, 3, 2026, 10], ['kubernetes', 2021, 3, 2026, 10], ['ros 2', 2020, 2, 2026, 10], ['docker', 2018, 6, 2021, 2], ['slam', 2018, 6, 2021, 2], ['rust', 2023, 1, 2023, 12]]
    .map(([skill, a, b, c, d]) => ({ skill: skill as string, start: ym(a as number, b as number), end: ym(c as number, d as number), months: ((c as number) - (a as number)) * 12 + (d as number) - (b as number) + 1 })),
  skill_matrix: PF_SKILLS.map(([skill, , , , proficiency]) => ({ skill, proficiency })),
}

const H: HeurVals = { bias: -0.3, page_threshold: 0.5, w_prior_pages: 0.45, w_years: 0.04, years_cap: 10, w_seniority: 0.15, w_signal: 0.35, signal_hits_full: 3, certs_two: 5, certs_one: 3, projects_two: 3, projects_one: 2, primary_bullets_two: 8, primary_bullets_one: 6, summary_two: 3, summary_one: 2, older_roles_two: 0.05, older_roles_one: 0.1, compact_volume: 14, fit_primary_floor: 5, fit_certs_floor: 3, fit_max_renders: 8 }
let heur: HeurVals = { ...H }
let prior: number | null = null
export const demoHeur = {
  get: async (): Promise<Heuristics> => ({ effective: { ...heur }, defaults: { ...H } }),
  put: async (patch: HeurVals): Promise<Heuristics> => {
    for (const [k, v] of Object.entries(patch)) {
      if (!(k in H)) throw new Error(`unknown key ${k}`)
      if (!Number.isFinite(v) || Math.abs(v) > 100) throw new Error(`${k} out of range (-100..=100)`)
    }
    heur = { ...heur, ...patch }
    return demoHeur.get()
  },
  reset: async () => { heur = { ...H }; return demoHeur.get() },
  getSettings: async () => ({ prior_resume_pages: prior }),
  putSettings: async (p: number | null) => { prior = p },
}

export const jobExtras = (ok: boolean): Partial<JobDetail> => !ok ? {} : {
  decisions: { why: [
    { decision: 'target_pages', score: 0.92, outcome: 'score 0.92 >= 0.50 => 2 pages', contributions: [['bias', -0.3], ['experience_years', 0.34], ['jd_seniority', 0.19], ['jd_senior_signal', 0.23], ['role_count', 0.09]] },
    { decision: 'max_certs', score: 0.92, outcome: '5 for a 2-page target', contributions: [['target_pages', 2]] },
    { decision: 'max_projects', score: 0.92, outcome: '3 for a 2-page target', contributions: [['target_pages', 2]] },
  ] },
  fit: { pages: 2, layout_level: 1, steps: [], dropped: [] },
  grounding: [{ bullet_id: 'e0.b2', violations: [{ kind: 'number', detail: 'introduced unsupported number 4M' }] }],
  jd_class: { family: [['devops_cloud', 0.62], ['backend', 0.25], ['embedded', 0.08], ['ml_ai', 0.03], ['management', 0.02]], seniority: [['senior', 0.8], ['lead', 0.12], ['mid', 0.06], ['junior', 0.02]], evidence: ['kubernetes', 'ros 2', 'lead'], confirmed: null },
  similar_jobs: [{ id: 'demo-seed-2', company: 'Northwind Automation', role: 'Robotics Platform Lead', similarity: 0.74 }, { id: 'demo-seed-3', company: 'Initech Labs', role: 'Staff Perception Engineer', similarity: 0.41 }],
  outcome: null,
  near_duplicates: [{ a: 'e0.b1', b: 'e0.b3', similarity: 0.93 }],
  redaction_summary: { counts: { person: 3, org: 9, email: 1, phone: 1 }, total: 14 },
  facts_summary: { years: 8.5, top_skills: [{ canonical: 'c++', level: 'Expert', months_used: 101 }, { canonical: 'python', level: 'Expert', months_used: 71 }, { canonical: 'docker', level: 'Strong', months_used: 45 }] },
}
