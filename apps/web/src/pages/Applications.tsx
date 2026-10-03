import { Kanban } from '@phosphor-icons/react'
import { useCallback, useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import { Rise, Skel } from '../components/Motion'
import { HBars } from '../components/Viz'
import { ago } from '../lib/util'
import { useApp } from '../store'
import { STAGES, type AppStage, type Application, type Funnel } from '../types'

function AppCard({ a, save }: { a: Application; save: (id: string, stage: AppStage, note?: string) => void }) {
  const [note, setNote] = useState(a.note ?? '')
  useEffect(() => setNote(a.note ?? ''), [a.note])
  return (
    <li className="acard">
      <Link to={`/jobs/${a.job_id}`}><b>{a.company}</b></Link>
      <span className="small muted">{a.role}</span>
      <span className="tiny muted">Applied {ago(a.applied_at)}, moved {ago(a.updated_at)}</span>
      <select className="sel-sm" aria-label={`Stage for ${a.company}`} value={a.stage} onChange={(e) => save(a.job_id, e.target.value as AppStage)}>
        {STAGES.map((s) => <option key={s} value={s}>{s}</option>)}
      </select>
      <textarea aria-label={`Note for ${a.company}`} placeholder="Add a note" value={note} onChange={(e) => setNote(e.target.value)} onBlur={() => note !== (a.note ?? '') && save(a.job_id, a.stage, note)} />
    </li>
  )
}

export default function Applications() {
  const [apps, setApps] = useState<Application[] | null>(null)
  const [fn, setFn] = useState<Funnel | null>(null)
  const [err, setErr] = useState('')
  const { toast } = useApp()
  const load = useCallback(() => { api.getApps().then(setApps, (e) => setErr(e.message)); api.getFunnel().then(setFn, () => {}) }, [])
  useEffect(load, [load])
  const save = async (id: string, stage: AppStage, note?: string) => {
    try { await api.trackApp(id, stage, note); load() } catch (e) { toast({ kind: 'err', text: e instanceof Error ? e.message : 'Could not save.' }) }
  }
  const pct = (v: number) => `${Math.round(v * 100)}%`
  return (
    <>
      <div className="page-head"><h1>Applications</h1><p>Track where each tailored resume ended up. The outcome model learns from these, and it needs about 25 to estimate anything.</p></div>
      {err ? <div className="err-box">{err}</div> : !apps ? <Skel h={320} r={18} /> : apps.length === 0 ? (
        <Rise className="empty"><Kanban size={34} aria-hidden /><h2>No applications tracked</h2><p>Open a finished job and choose Track application. It then shows up here.</p><Link className="btn primary" to="/jobs">Open jobs</Link></Rise>
      ) : (
        <div className="stack gap-xl">
          {fn && (
            <Rise as="section" className="panel" aria-label="Funnel">
              <header><h2>Funnel</h2><p>{fn.total} tracked, {fn.pending} still waiting. Rates only count applications that have an outcome{fn.total - fn.pending > 0 && <>: {pct(fn.response_rate)} responded (range {pct(fn.response_ci[0])} to {pct(fn.response_ci[1])}), {pct(fn.interview_rate)} reached an interview, {pct(fn.offer_rate)} got an offer</>}.</p></header>
              <HBars label="Applications by furthest stage" fmt={(v) => String(v)} max={Math.max(fn.total, 1)} rows={[{ name: 'Applied', value: fn.total }, { name: 'Responded', value: fn.responded }, { name: 'Interview', value: fn.interviews }, { name: 'Offer', value: fn.offers }, { name: 'Rejected', value: fn.rejected }, { name: 'Ghosted', value: fn.ghosted }]} />
            </Rise>
          )}
          <div className="board" role="list" aria-label="Applications by stage">
            {STAGES.map((s) => {
              const col = apps.filter((a) => a.stage === s)
              return (
                <section key={s} className="col" role="listitem" aria-label={s}>
                  <h3><span>{s}</span><span className="num muted">{col.length}</span></h3>
                  {col.length === 0 ? <div className="none">Nothing here</div> : <ul className="clean stack" style={{ gap: 10 }}>{col.map((a) => <AppCard key={a.job_id} a={a} save={save} />)}</ul>}
                </section>
              )
            })}
          </div>
        </div>
      )}
    </>
  )
}
