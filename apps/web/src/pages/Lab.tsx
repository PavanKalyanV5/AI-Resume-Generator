import { ArrowsClockwise, Flask } from '@phosphor-icons/react'
import { useCallback, useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import { Rise, Skel } from '../components/Motion'
import { HBars, Spark, human } from '../components/Viz'
import { ago } from '../lib/util'
import { useApp } from '../store'
import type { MlCards, ModelCard, ModelStatus, Recs } from '../types'

const BADGE: Record<ModelStatus, [string, string]> = { ColdStart: ['warn', 'Cold start: using built-in prior'], Learning: ['learn', 'Learning'], Ready: ['ok', 'Ready'] }
const TEACH: Record<string, string> = {
  bullet_ranker: 'Press Keep or Remove on bullets in a job result.',
  rewrite_need: 'Every run teaches it: rewrites you keep count as accepted, reverted ones do not.',
  fit_model: 'Each rendered PDF teaches it how many lines a layout really takes.',
  jd_family: 'Correct the job type on a job page when it guesses wrong.',
  jd_seniority: 'Correct the seniority on a job page when it guesses wrong.',
  outcome: 'Track applications and what happened to them.',
  pii_candidate: 'Accept or dismiss suggestions on the PII ledger.',
}
const lowerBetter = (c: ModelCard) => /lower|MAE|Brier/i.test(c.metric_name)
const f2 = (v: number) => (v !== 0 && Math.abs(v) < 0.1 ? v.toFixed(3) : v.toFixed(2))
const title = (n: string) => human(n).replace(/\b(jd|pii)\b/g, (x) => x.toUpperCase())
const evidenceText = (e: unknown) => {
  if (typeof e === 'string') return e
  if (!Array.isArray(e)) return ''
  const n: Record<string, number> = {}
  e.forEach((x) => { const k = String((x as { kind?: string }).kind ?? '').toLowerCase(); if (k) n[k] = (n[k] ?? 0) + 1 })
  return Object.entries(n).map(([k, c]) => `${c} ${k === 'cert' ? 'certification' : k}${c > 1 ? 's' : ''}`).join(', ')
}

function Card({ c }: { c: ModelCard }) {
  const [cls, text] = BADGE[c.status]
  const better = c.metric_cv != null && c.baseline != null && (lowerBetter(c) ? c.metric_cv < c.baseline : c.metric_cv > c.baseline)
  const feats = c.top_features.slice(0, 5).map(([name, value]) => ({ name: human(name), value }))
  return (
    <Rise as="section" className="panel mcard" aria-label={title(c.name)} inView>
      <div className="m-top"><h3>{title(c.name)}</h3><span className={`badge ${cls}`}>{text}</span></div>
      <div className="m-nums">
        <div><b>{c.n_samples}</b><span>samples</span></div>
        <div><b>{c.metric_cv != null ? f2(c.metric_cv) : 'n/a'}</b><span>{c.metric_name}</span></div>
        <div><b>{c.baseline != null ? f2(c.baseline) : 'n/a'}</b><span>baseline</span></div>
      </div>
      <p className="small">{c.metric_cv == null ? 'No validated score yet, so the built-in prior is doing the work.' : better ? 'Beats the baseline, so it is allowed to influence results.' : 'Does not beat the baseline yet, so its influence stays small.'}</p>
      {c.learning_curve.length > 1 && <div><div className="subh">Score as it learned</div><Spark points={c.learning_curve.map((p) => p[1])} label={`${human(c.name)} score from ${f2(c.learning_curve[0][1])} to ${f2(c.learning_curve[c.learning_curve.length - 1][1])} over ${c.learning_curve[c.learning_curve.length - 1][0]} samples`} /></div>}
      {feats.length > 0 && <div><div className="subh">What it weighs most</div><HBars rows={feats} label={`Top features of ${human(c.name)}`} /></div>}
      <p className="small muted">{c.notes}</p>
      <p className="hint" style={{ marginTop: 0 }}><b>What teaches it:</b> {TEACH[c.name] ?? 'Normal use.'}{c.updated_at && <> Updated {ago(c.updated_at)}.</>}</p>
    </Rise>
  )
}

export default function Lab() {
  const [m, setM] = useState<MlCards | null>(null)
  const [r, setR] = useState<Recs | null>(null)
  const [err, setErr] = useState('')
  const [busy, setBusy] = useState(false)
  const { toast } = useApp()
  const load = useCallback(() => { api.getMlCards().then(setM, (e) => setErr(e.message)); api.getRecs().then(setR, () => setR({ learn_next: [], trends: [], resume_additions: [] })) }, [])
  useEffect(load, [load])
  const retrain = async () => {
    setBusy(true)
    try { await api.retrain(); toast({ kind: 'info', text: 'Retraining in the background. You will get a notification when it finishes.' }); setTimeout(load, 2500) }
    catch (e) { toast({ kind: 'err', text: e instanceof Error ? e.message : 'Could not start retraining.' }) } finally { setBusy(false) }
  }
  const sig = m ? Object.entries(m.signals) : []
  return (
    <>
      <div className="page-head"><h1>ML Lab</h1><p>Small models that run on this machine and learn from how you use the app. Each one says honestly how much it has learned.</p><p className="small"><Link to="/learning">See your learning curve</Link> or <Link to="/playbook">review the rules it suggests</Link>.</p></div>
      {err ? <div className="err-box">{err}</div> : !m ? <div className="grid-auto"><Skel h={320} r={18} /><Skel h={320} r={18} /></div> : (
        <div className="stack gap-xl">
          <div className="row between">
            <div className="signals" aria-label="Learning signals">{sig.map(([k, v]) => <span key={k}><b>{v}</b> {human(k)}</span>)}</div>
            <button className="btn" onClick={retrain} disabled={busy}><ArrowsClockwise size={16} aria-hidden />Retrain now</button>
          </div>
          <div className="grid-auto">{m.cards.map((c) => <Card key={c.name} c={c} />)}</div>
          <Rise as="section" className="panel" aria-label="Skill recommendations" inView>
            <header><h2><Flask size={18} aria-hidden />Learn next</h2><p>Skills your saved job descriptions keep asking for, ranked by demand and how close they sit to what you already know.</p></header>
            {!r ? <Skel h={160} r={12} /> : r.learn_next.length === 0 && r.trends.length === 0 ? (
              <div className="empty" style={{ border: 0, padding: 0 }}><h3>Nothing to recommend yet</h3><p>This learns from the job descriptions you process. Tailor a few resumes and the skills worth learning appear here.</p><Link className="btn primary" to="/">Start a job</Link></div>
            ) : (
              <div className="stack gap-xl">
                <ol className="rank clean">
                  {r.learn_next.map((x, i) => (
                    <li key={x.skill}><span className="r-n">{i + 1}</span>
                      <div><h3>{x.skill}</h3><p className="small muted">{x.why}</p>
                        {x.adjacent_to.length > 0 && <div className="chips" style={{ marginTop: 8 }} aria-label={`Adjacent to ${x.adjacent_to.join(', ')}`}>{x.adjacent_to.map((a) => <span key={a} className="chip">{a}</span>)}</div>}</div>
                      <span className="num small muted" title="Share of your saved JDs that mention it">{Math.round(x.demand_share * 100)}% of JDs</span></li>
                  ))}
                </ol>
                {r.trends.length > 0 && <div><div className="subh">Mentions per month in your JDs</div>
                  <ul className="clean" style={{ display: 'grid', gap: 10 }}>{r.trends.map(([skill, pts]) => (
                    <li key={skill} className="row between" style={{ flexWrap: 'nowrap' }}><span className="small">{skill}</span>
                      <span className="row" style={{ gap: 12, flexWrap: 'nowrap' }}><Spark w={120} h={28} points={pts.map((p) => p[1])} label={`${skill}: ${pts.map((p) => `${p[0]} ${p[1]}`).join(', ')}`} /><span className="num small muted" style={{ minWidth: 20, textAlign: 'right' }}>{pts[pts.length - 1]?.[1] ?? 0}</span></span></li>))}</ul></div>}
              </div>
            )}
          </Rise>
          <Rise as="section" className="panel" aria-label="Safe to add" inView>
            <header><h2>Safe to add to your resume</h2><p>Jobs ask for these and your experience already backs them up, but your skills section does not list them. Nothing here is invented.</p></header>
            {!r || r.resume_additions.length === 0 ? <p className="small muted">{r ? 'No gaps between your evidence and your skills section.' : ''}</p> : (
              <ul className="clean" style={{ display: 'grid', gap: 10 }}>{r.resume_additions.map((a) => (
                <li key={a.skill} className="row between"><span><b>{a.skill}</b> <span className="small muted">backed by {evidenceText(a.evidence) || 'your experience'}</span></span><span className="num small muted">{Math.round(a.demand_share * 100)}% of JDs</span></li>))}</ul>
            )}
          </Rise>
        </div>
      )}
    </>
  )
}
