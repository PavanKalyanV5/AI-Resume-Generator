import { Check, Minus, Target } from '@phosphor-icons/react'
import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import { useApp } from '../store'
import { STAGES, type AppStage, type JobDetail } from '../types'
import { HBars, human } from './Viz'

/** Keep / Remove on one tailored bullet. `id` is e<role>.b<n> against the full resume. */
export function BulletFb({ job, id }: { job: string; id: string }) {
  const [done, setDone] = useState<'keep' | 'remove' | null>(null)
  const { toast } = useApp()
  const send = async (a: 'keep' | 'remove') => {
    try { await api.feedback(job, id, a); setDone(a) } catch (e) { toast({ kind: 'err', text: e instanceof Error ? e.message : 'Could not save that.' }) }
  }
  return (
    <span className="fb">
      <button type="button" aria-pressed={done === 'keep'} aria-label="Keep this bullet" onClick={() => send('keep')}><Check size={12} aria-hidden />Keep</button>
      <button type="button" aria-pressed={done === 'remove'} aria-label="Remove this bullet" onClick={() => send('remove')}><Minus size={12} aria-hidden />Remove</button>
      <span className="ack" role="status">{done ? 'Learning from this' : ''}</span>
    </span>
  )
}

export function TrackControl({ id }: { id: string }) {
  const [stage, setStage] = useState<AppStage | null | undefined>(undefined)
  const { toast } = useApp()
  useEffect(() => { api.getApps().then((a) => setStage(a.find((x) => x.job_id === id)?.stage ?? null), () => setStage(null)) }, [id])
  const set = async (s: AppStage) => {
    try { await api.trackApp(id, s); setStage(s); toast({ kind: 'ok', text: stage ? `Moved to ${s}.` : 'Tracking this application.' }) } catch (e) { toast({ kind: 'err', text: e instanceof Error ? e.message : 'Could not track it.' }) }
  }
  if (stage === undefined) return null
  return stage ? (
    <select className="sel-sm" aria-label="Application stage" value={stage} onChange={(e) => set(e.target.value as AppStage)} style={{ height: 40 }}>{STAGES.map((s) => <option key={s} value={s}>Application: {s}</option>)}</select>
  ) : <button className="btn" onClick={() => set('applied')}><Target size={16} aria-hidden />Track application</button>
}

function JdClassPanel({ id, c }: { id: string; c: NonNullable<JobDetail['jd_class']> }) {
  const [fam, setFam] = useState(c.confirmed?.family ?? c.family[0]?.[0] ?? '')
  const [sen, setSen] = useState(c.confirmed?.seniority ?? c.seniority[0]?.[0] ?? '')
  const [saved, setSaved] = useState(!!c.confirmed)
  const { toast } = useApp()
  const save = async () => {
    try { await api.setJdClass(id, fam, sen); setSaved(true); toast({ kind: 'ok', text: 'Learning from this correction.' }) } catch (e) { toast({ kind: 'err', text: e instanceof Error ? e.message : 'Could not save.' }) }
  }
  const pct = (p: number) => `${Math.round(p * 100)}%`
  return (
    <section className="panel" aria-label="Job type">
      <header><h2>What kind of job this is</h2><p>The app's guess from the job description{c.evidence.length > 0 && <>, based on words like {c.evidence.slice(0, 4).join(', ')}</>}. Correct it and the classifier learns.</p></header>
      <div className="chips">
        {c.family.filter(([, p], i) => i === 0 || p >= 0.05).slice(0, 3).map(([n, p]) => <span key={n} className="chip hit">{human(n)} {pct(p)}</span>)}
        {c.seniority.filter(([, p], i) => i === 0 || p >= 0.05).slice(0, 2).map(([n, p]) => <span key={n} className="chip">{n} {pct(p)}</span>)}
        {saved && <span className="badge ok">Confirmed by you</span>}
      </div>
      <div className="row">
        <select className="sel-sm" aria-label="Job family" value={fam} onChange={(e) => { setFam(e.target.value); setSaved(false) }}>{c.family.map(([n]) => <option key={n} value={n}>{human(n)}</option>)}</select>
        <select className="sel-sm" aria-label="Seniority" value={sen} onChange={(e) => { setSen(e.target.value); setSaved(false) }}>{c.seniority.map(([n]) => <option key={n} value={n}>{n}</option>)}</select>
        <button className="btn sm" onClick={save} disabled={saved}>Save correction</button>
      </div>
    </section>
  )
}

const GROUND: Record<string, string> = { new_number: 'introduced an unsupported number', new_technology: 'introduced a skill you have not listed', years_claim: 'claimed years of experience you cannot back up', new_org_or_client: 'named an employer or client that is not yours', superlative: 'used an unsupported superlative' }

export function Insights({ data }: { data: JobDetail }) {
  const why = data.decisions?.why ?? []
  const g = data.grounding ?? []
  const sim = data.similar_jobs ?? []
  const rs = data.redaction_summary
  const fs = data.facts_summary
  const bulletText = (id: string) => { const m = /^e(\d+)\.b(\d+)$/.exec(id); return m ? data.original.experience[Number(m[1])]?.bullets[Number(m[2])] : undefined }
  return (
    <div className="insights">
      {why.length > 0 && (
        <section className="panel" aria-label="Why these choices">
          <header><h2>Why these choices</h2><p>How many pages, certifications and projects were decided. Each bar is one factor pushing the score up or down.</p></header>
          {why.map((w, i) => (
            <details key={w.decision} open={i === 0}>
              <summary><b>{human(w.decision)}</b> <span className="small muted">{w.outcome}</span></summary>
              <div style={{ marginTop: 12 }}><HBars label={`Factors for ${human(w.decision)}`} rows={w.contributions.map(([name, value]) => ({ name: human(name), value }))} fmt={(v) => (v > 0 ? '+' : '') + v.toFixed(2)} /></div>
            </details>
          ))}
          {data.fit && <p className="small muted">Final layout: {data.fit.pages} {data.fit.pages === 1 ? 'page' : 'pages'}, level {data.fit.layout_level}{data.fit.dropped.length > 0 && `, ${data.fit.dropped.length} items dropped to fit`}.</p>}
        </section>
      )}
      <div className="stack gap-xl">
        {data.jd_class && <JdClassPanel key={data.job.id + (data.jd_class.confirmed?.family ?? '')} id={data.job.id} c={data.jd_class} />}
        <section className="panel" aria-label="Checks on the AI's rewrites">
          <header><h2>Grounding</h2><p>Every rewrite is checked against your own facts.</p></header>
          {g.length === 0 ? <p className="small muted">{data.tailored ? 'No rewrite needed fixing.' : 'Checked once the AI step finishes.'}</p> : (
            <ul className="clean stack" style={{ gap: 12 }}>{g.map((x) => (
              <li key={x.bullet_id} className="stack" style={{ gap: 4 }}>
                <span className="small"><b>{x.repaired ? 'Repaired by the AI:' : 'Reverted:'}</b> {x.violations.map((v) => GROUND[v.kind] ?? human(v.kind)).join(', ')}</span>
                {bulletText(x.bullet_id) && <span className="small muted">{bulletText(x.bullet_id)}</span>}
                {x.violations.map((v, i) => <span key={i} className="tiny muted">{v.detail}</span>)}
              </li>))}</ul>
          )}
        </section>
        <section className="panel" aria-label="Estimates and context">
          <header><h2>Context</h2></header>
          <p className="small"><b>Response odds:</b> {data.outcome ? `${Math.round(data.outcome.p * 100)}% (likely ${Math.round(data.outcome.lo * 100)}% to ${Math.round(data.outcome.hi * 100)}%)` : <span className="muted">Needs 25 tracked applications to estimate. <Link to="/applications">Applications</Link></span>}</p>
          {rs && rs.total > 0 && <p className="small"><b>Redacted before sending:</b> {rs.total} values ({Object.entries(rs.counts).map(([k, v]) => `${v} ${k}`).join(', ')})</p>}
          {fs && <div><div className="subh">Strongest skills from your profile, {fs.years.toFixed(1)} years</div><div className="chips">{fs.top_skills.slice(0, 6).map((s) => <span key={s.canonical} className="chip">{s.canonical}, {s.level.toLowerCase()}</span>)}</div></div>}
          {sim.length > 0 && <div><div className="subh">Similar jobs you have processed</div><ul className="clean stack" style={{ gap: 6 }}>{sim.map((s) => <li key={s.id} className="small row between"><Link to={`/jobs/${s.id}`}>{s.role} at {s.company}</Link><span className="num muted">{Math.round(s.similarity * 100)}%</span></li>)}</ul></div>}
          {(data.near_duplicates?.length ?? 0) > 0 && <p className="small muted">{data.near_duplicates!.length} pairs of bullets read almost the same, for example <span className="mono">{data.near_duplicates![0].a}</span> and <span className="mono">{data.near_duplicates![0].b}</span> ({Math.round(data.near_duplicates![0].similarity * 100)}% alike).</p>}
        </section>
      </div>
    </div>
  )
}
