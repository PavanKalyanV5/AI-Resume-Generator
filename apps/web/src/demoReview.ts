// Fake AI-review fixtures (Jane Doe, Acme Robotics). The demo AI swapped two compact UI-clone projects for agent/RAG ones.
import type { Overrides, Pool, Review } from './types'

const P: Pool['projects'] = [
  { id: 'p1', name: 'Fleet Rollout Operator', tech: ['Kubernetes', 'Go'], tier: 'A', score: 0.91, selected: true },
  { id: 'p2', name: 'Support Agent with Tool Use', tech: ['Python', 'LLM agents'], tier: 'A', score: 0.84, selected: true },
  { id: 'p3', name: 'RAG over Robot Logs', tech: ['Python', 'pgvector', 'RAG'], tier: 'A', score: 0.79, selected: true },
  { id: 'p4', name: 'ROS 2 Nav Tuning Kit', tech: ['C++', 'ROS 2'], tier: 'B', score: 0.55, selected: false },
  { id: 'p5', name: 'Kanban Board Clone', tech: ['React', 'CSS'], tier: 'C', score: 0.22, selected: false },
  { id: 'p6', name: 'Music Player UI Clone', tech: ['React'], tier: 'C', score: 0.18, selected: false },
]
const C: Pool['certs'] = [
  { id: 'c1', title: 'Certified Kubernetes Administrator', issuer: 'CNCF', date: '2024', featured: true, score: 0.9, selected: true },
  { id: 'c2', title: 'AWS Solutions Architect Associate', issuer: 'Acme Cloud Academy', date: '2023', featured: true, score: 0.82, selected: true },
  { id: 'c3', title: 'ROS 2 Developer', issuer: 'Open Robotics Training', date: '2023', featured: false, score: 0.7, selected: true },
  { id: 'c4', title: 'Docker Certified Associate', issuer: 'Acme Cloud Academy', date: '2022', featured: false, score: 0.52, selected: true },
  { id: 'c5', title: 'Python Professional', issuer: 'Acme Cloud Academy', date: '2021', featured: false, score: 0.48, selected: true },
  { id: 'c6', title: 'Terraform Associate', issuer: 'Acme Cloud Academy', date: '2024', featured: false, score: 0.61, selected: false },
  { id: 'c7', title: 'Agile Practitioner', issuer: 'Acme Cloud Academy', date: '2019', featured: false, score: 0.2, selected: false },
]
const R: Pool['roles'] = [
  { id: 'e0', role: 'Senior Software Engineer, Acme Robotics', dates: '2021 to now', kind: 'FullTime', selected: true, locked: true },
  { id: 'e1', role: 'Software Engineer, Acme Robotics', dates: '2018 to 2021', kind: 'FullTime', selected: true, locked: false },
  { id: 'e2', role: 'Robotics Intern, Acme Robotics', dates: '2017', kind: 'Internship', selected: true, locked: false },
  { id: 'e3', role: 'Freelance Web Developer', dates: '2016', kind: 'Other', selected: false, locked: false },
]
const S: Pool['skills'] = [
  { label: 'Platform & Cloud', skills: ['Kubernetes', 'Docker', 'AWS', 'Terraform'], score: 0.88 },
  { label: 'Robotics', skills: ['ROS 2', 'C++', 'Nav2'], score: 0.8 },
  { label: 'AI & Data', skills: ['Python', 'RAG', 'pgvector'], score: 0.66 },
  { label: 'Practices', skills: ['CI/CD', 'Code review'], score: 0.3 },
]

const issues: Review['issues'] = [
  { id: 'i1', area: 'projects', severity: 'info', source: 'ai', status: 'applied', message: 'Swapped Kanban Board Clone and Music Player UI Clone for Support Agent with Tool Use and RAG over Robot Logs. The job asks for agent and retrieval work.', change: { kind: 'swap', from: 'p5, p6', to: 'p2, p3', ids: ['p2', 'p3'] } },
  { id: 'i2', area: 'experience', severity: 'info', source: 'lint', status: 'applied', message: 'Put the roles in newest-first order.', change: { kind: 'reorder', ids: ['e0', 'e1', 'e2'] } },
  { id: 'i3', area: 'skills', severity: 'warn', source: 'lint', status: 'applied', message: 'The Practices skills section had collapsed to nothing, so it was restored.', change: { kind: 'restore', ids: ['Practices'] } },
  { id: 'i4', area: 'certs', severity: 'warn', source: 'ai', status: 'suggested', message: 'Terraform Associate (2024) fits this job better than Docker Certified Associate (2022). It is only a suggestion because both are in range.', change: { kind: 'swap', from: 'c4', to: 'c6', ids: ['c6'] } },
  { id: 'i5', area: 'summary', severity: 'info', source: 'ai', status: 'rejected', reason: 'It added a claim that is not in your resume.', message: 'Proposed adding "led a 20-person team" to the summary.' },
  { id: 'i6', area: 'pages', severity: 'info', source: 'lint', status: 'applied', message: 'Everything fits on 2 pages.' },
]

const pick = <T,>(list: T[], key: (x: T) => string, ids?: string[]) => (ids?.length ? [...list].sort((a, b) => (ids.indexOf(key(a)) + 1 || 99) - (ids.indexOf(key(b)) + 1 || 99)) : list)

/** Pool and review as the server would return them after the owner's overrides (an empty list clears an override). */
export function reviewFor(ov: Overrides | undefined): { pool: Pool; review: Review } {
  const sel = <T extends { id: string; selected: boolean }>(l: T[], ids?: string[]) => (ids?.length ? pick(l, (x) => x.id, ids).map((x) => ({ ...x, selected: ids.includes(x.id) })) : l)
  const pool: Pool = { projects: sel(P, ov?.projects), certs: sel(C, ov?.certs), roles: sel(R, ov?.roles), skills: pick(S, (x) => x.label, ov?.skills_order) }
  const mine = ov && (ov.projects?.length || ov.certs?.length || ov.roles?.length || ov.skills_order?.length)
  const review: Review = {
    ran: true, provider: 'mock', tokens_in: 1840, tokens_out: 412, confidence: 0.82,
    issues: issues.map((i) => (ov?.certs?.length && i.id === 'i4' ? { ...i, status: 'rejected' as const, reason: 'You chose your own certifications.' } : i)),
    overrides_applied: { projects: ov?.projects ?? [], certs: ov?.certs ?? [], roles: ov?.roles ?? [], skills_order: ov?.skills_order ?? [] },
    user_overrides: mine ? ov : null,
  }
  return { pool, review }
}
