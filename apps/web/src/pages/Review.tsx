import { ArrowDown, ArrowUp, ArrowsClockwise, CheckCircle, Info, Warning, WarningOctagon } from '@phosphor-icons/react'
import { useEffect, useState } from 'react'
import { api } from '../api'
import { TeachWhy } from '../components/Learn'
import { Rise } from '../components/Motion'
import { useApp } from '../store'
import { isActive, type Pool, type ReviewArea, type ReviewIssue } from '../types'
import { useJobCtx } from './JobDetail'

const AREA: Record<ReviewArea, string> = { projects: 'Projects', certs: 'Certifications', skills: 'Skills', experience: 'Experience', summary: 'Summary', pages: 'Length', order: 'Order' }
const SEV = { info: Info, warn: Warning, error: WarningOctagon } as const
const SEV_TXT = { info: 'Note', warn: 'Warning', error: 'Problem' } as const
// ponytail: caps fixed to the 2-page defaults the plan uses; read them from decisions when the server sends them.
let CAP: { projects: number; certs: number } = { projects: 3, certs: 5 }

function Issue({ i }: { i: ReviewIssue }) {
  const I = SEV[i.severity]
  return (
    <li className={`rv-issue sev-${i.severity}`}>
      <I size={18} aria-hidden /><span className="sr-only">{SEV_TXT[i.severity]}. </span>
      <div className="rv-msg">
        <p>{i.message}</p>
        <div className="row" style={{ gap: 6 }}>
          <span className={`badge ${i.status === 'applied' ? 'ok' : i.status === 'suggested' ? 'warn' : ''}`}>{i.status === 'applied' ? 'Applied' : i.status === 'suggested' ? 'Suggested, your call' : 'Rejected'}</span>
          <span className="badge">{i.source === 'ai' ? 'AI' : 'Lint'}</span>
          {i.status === 'rejected' && i.reason && <span className="small muted">{i.reason}</span>}
        </div>
      </div>
    </li>
  )
}

type Row = { id: string; title: string; sub: string; score?: number; selected: boolean; locked?: boolean }
type Sec = 'projects' | 'certs' | 'roles' | 'skills_order'
const rowsOf = (p: Pool): Record<Sec, Row[]> => ({
  projects: p.projects.map((x) => ({ id: x.id, title: x.name, sub: `${x.tech.join(', ')} · tier ${x.tier}`, score: x.score, selected: x.selected })),
  certs: p.certs.map((x) => ({ id: x.id, title: x.title, sub: `${x.issuer}, ${x.date}${x.featured ? ' · featured' : ''}`, score: x.score, selected: x.selected })),
  roles: p.roles.map((x) => ({ id: x.id, title: x.role, sub: `${x.dates} · ${x.kind === 'FullTime' ? 'Full time' : x.kind}`, selected: x.selected, locked: x.locked })),
  skills_order: p.skills.map((x) => ({ id: x.label, title: x.label, sub: x.skills.join(', '), score: x.score, selected: true })),
})
const TITLE: Record<Sec, string> = { projects: 'Projects', certs: 'Certifications', roles: 'Experience roles', skills_order: 'Skills categories' }
const HELP: Record<Sec, string> = { projects: `Pick up to ${CAP.projects}. Listed by relevance; shown newest first on the resume.`, certs: `Pick up to ${CAP.certs}. Listed by relevance; shown newest first on the resume.`, roles: 'Locked roles stay unless you force them off.', skills_order: 'Use the arrows to reorder categories.' }
const sameList = (a: string[], b: string[]) => a.length === b.length && a.every((x, k) => x === b[k])
// What gets sent for a section: the selected ids in list order (every label for skills).
const ids = (r: Row[]) => r.filter((x) => x.selected).map((x) => x.id)

export default function Review() {
  const { data, reload } = useJobCtx()
  const { toast } = useApp()
  const { pool, review } = data
  if (pool?.caps) CAP = pool.caps // server decides the caps (decisions.max_projects / max_certs)
  const [rows, setRows] = useState<Record<Sec, Row[]> | null>(null)
  const [force, setForce] = useState(false)
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState('')
  const sig = JSON.stringify(pool)
  useEffect(() => { if (pool) setRows(rowsOf(pool)) }, [sig]) // eslint-disable-line react-hooks/exhaustive-deps
  const running = isActive(data.job.status)
  useEffect(() => { if (running) setMsg('The job is re-rendering with your plan.'); else if (msg) setMsg('Done. The result is up to date.') }, [running]) // eslint-disable-line react-hooks/exhaustive-deps

  if (!pool || !review || !rows) {
    const waiting = data.steps.some((s) => s.name === 'ai_review') && running
    return <div className="empty" role="status" aria-live="polite"><h2>{waiting ? 'The plan is being checked' : 'No review for this job'}</h2>
      <p>{waiting ? 'The review appears once the pick step and the AI check have finished.' : 'This job ran before plans were reviewed, so there is nothing to confirm or correct. New jobs get a review automatically.'}</p></div>
  }
  const base = rowsOf(pool)
  const changed = (['projects', 'certs', 'roles', 'skills_order'] as Sec[]).filter((s) => !sameList(ids(rows[s]), ids(base[s])))
  const suggested = new Set(review.issues.filter((i) => i.status === 'suggested').flatMap((i) => i.change?.ids ?? []))
  const mine = (s: Sec) => new Set(review.overrides_applied[s])
  const groups = (Object.keys(AREA) as ReviewArea[]).map((a) => [a, review.issues.filter((i) => i.area === a)] as const).filter(([, l]) => l.length)
  const who = [...new Set(review.issues.map((i) => i.source))].map((s) => (s === 'ai' ? 'the AI' : 'the lint rules')).join(' and ') || 'the AI'

  const edit = (s: Sec, f: (r: Row[]) => Row[]) => setRows((r) => r && { ...r, [s]: f(r[s]) })
  const move = (s: Sec, k: number, d: number) => edit(s, (r) => { const n = [...r]; [n[k], n[k + d]] = [n[k + d], n[k]]; return n })
  const toggle = (s: Sec, id: string) => edit(s, (r) => r.map((x) => (x.id === id ? { ...x, selected: !x.selected } : x)))
  const send = async (o: Parameters<typeof api.setOverrides>[1]) => {
    setBusy(true)
    try { await api.setOverrides(data.job.id, o); setMsg('Re-rendering with your plan. Watch the progress bar above.'); reload() }
    catch (e) { toast({ kind: 'err', text: e instanceof Error ? e.message : 'That did not work.' }) } finally { setBusy(false) }
  }
  const apply = () => send({ ...Object.fromEntries(changed.map((s) => [s, ids(rows[s])])), ...(force ? { force: true } : {}) })
  const reset = () => (review.user_overrides ? send({ reset: true }) : (setRows(rowsOf(pool)), setForce(false)))
  const badge = (s: Sec, r: Row) => {
    const was = base[s].find((x) => x.id === r.id)?.selected
    if (r.selected !== was || (r.selected && mine(s).has(r.id))) return <span className="badge z-local">You picked</span>
    if (suggested.has(r.id) && !r.selected) return <span className="badge warn">Suggested</span>
    return r.selected && s !== 'skills_order' ? <span className="badge">AI picked</span> : null
  }
  const full = (s: Sec, r: Row) => (s === 'projects' || s === 'certs') && !r.selected && ids(rows[s]).length >= CAP[s]
  const stuck = (s: Sec, r: Row) => s === 'roles' && !!r.locked && r.selected && !force

  return (
    <div className="stack gap-xl">
      <Rise as="section" className="panel" i={0}>
        <header><h2>The plan was checked</h2><p>Checked by {who}. Each fix is listed below. Confirm it, or change it and re-render.</p></header>
        <div className="row rv-stats">
          <span className="badge z-ai">{review.provider} provider</span>
          <span className="badge">Confidence {Math.round(review.confidence * 100)}%</span>
          <span className="badge">{review.tokens_in.toLocaleString()} tokens in, {review.tokens_out.toLocaleString()} out</span>
          {!review.ran && <span className="badge warn">AI check skipped, lint only</span>}
          {review.user_overrides && <span className="badge z-local">Your overrides are applied</span>}
        </div>
        {groups.length ? groups.map(([a, l]) => (
          <section key={a} className="rv-group" aria-label={AREA[a]}>
            <h3>{AREA[a]}</h3>
            <ul className="rv-list">{l.map((i) => <Issue key={i.id} i={i} />)}</ul>
          </section>
        )) : <p className="small muted"><CheckCircle size={16} aria-hidden /> Nothing needed fixing.</p>}
      </Rise>

      <Rise as="section" className="panel" i={1}>
        <header><h2>Your plan, ranked</h2><p>Bars show how well each item matches this job. Change anything, then apply.</p></header>
        <div className="rv-secs">
          {(Object.keys(TITLE) as Sec[]).map((s) => {
            const n = ids(rows[s]).length
            return (
              <section key={s} className="rv-sec" aria-label={TITLE[s]}>
                <h3>{TITLE[s]} {s !== 'skills_order' && <span className="small muted">{n}{s in CAP ? ` of ${CAP[s as 'projects' | 'certs']}` : ''} selected</span>}</h3>
                <p className="small muted">{HELP[s]}</p>
                <ul className="rv-pick">
                  {rows[s].map((r, k) => (
                    <li key={r.id} className={r.selected ? 'on' : ''}>
                      {s !== 'skills_order' && <input id={`rv-${s}-${r.id}`} type="checkbox" checked={r.selected} disabled={busy || running || full(s, r) || stuck(s, r)} onChange={() => toggle(s, r.id)} />}
                      <label className="rv-main" htmlFor={`rv-${s}-${r.id}`}>
                        <b>{r.title}</b><span className="small muted">{r.sub}</span>
                        {r.score !== undefined && <span className="bar-line" role="img" aria-label={`Match ${Math.round(r.score * 100)} of 100`}><i style={{ transform: `scaleX(${r.score})` }} /></span>}
                      </label>
                      <span className="rv-b">{badge(s, r)}{r.locked && <span className="badge">Locked</span>}</span>
                      {s !== 'roles' && (
                        <span className="rv-mv">
                          <button className="icon-btn" disabled={busy || running || k === 0} onClick={() => move(s, k, -1)} aria-label={`Move ${r.title} up`}><ArrowUp size={14} /></button>
                          <button className="icon-btn" disabled={busy || running || k === rows[s].length - 1} onClick={() => move(s, k, 1)} aria-label={`Move ${r.title} down`}><ArrowDown size={14} /></button>
                        </span>
                      )}
                    </li>
                  ))}
                </ul>
              </section>
            )
          })}
        </div>
        <div className="row rv-act">
          <label className="row small" style={{ gap: 6 }}><input type="checkbox" checked={force} onChange={(e) => setForce(e.target.checked)} />Force: allow dropping locked roles</label>
          <span style={{ flex: 1 }} />
          <button className="btn ghost" disabled={busy || running || (!review.user_overrides && !changed.length)} onClick={reset}>Reset to AI plan</button>
          <button className="btn primary" disabled={busy || running || !changed.length} onClick={apply}><ArrowsClockwise size={16} aria-hidden />{running ? 'Re-rendering...' : 'Apply & re-render'}</button>
        </div>
        <p className="small muted" role="status" aria-live="polite">{msg || (changed.length ? `Changed: ${changed.map((s) => TITLE[s].toLowerCase()).join(', ')}.` : '')}</p>
      </Rise>
      <TeachWhy corrections={data.corrections} />
    </div>
  )
}
