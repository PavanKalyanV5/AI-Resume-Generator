import { TrendDown } from '@phosphor-icons/react'
import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import { CorrectionsChart } from '../components/Charts'
import { CountUp, Rise, Skel } from '../components/Motion'
import { useApp } from '../store'
import type { LearnMetrics } from '../types'
import { Regression } from './Playbook'

export default function Learning() {
  const [m, setM] = useState<LearnMetrics | null>(null)
  const [err, setErr] = useState('')
  const { proposed } = useApp()
  useEffect(() => { api.getLearning().then(setM, (e) => setErr(e.message)) }, [])
  if (err) return <div className="err-box">{err}</div>
  if (!m) return <div className="stack" style={{ gap: 16 }}><Skel h={96} r={18} /><Skel h={260} r={18} /></div>
  const n = m.jobs.length, approved = m.jobs.filter((j) => j.approved).length
  const half = Math.floor(n / 2), avg = (l: number[]) => (l.length ? l.reduce((a, b) => a + b, 0) / l.length : 0)
  const first = avg(m.jobs.slice(0, half).map((j) => j.corrections)), last = avg(m.jobs.slice(n - half).map((j) => j.corrections))
  const enough = approved >= 3 && n >= 4
  const pass = m.regression.cases ? Math.round((m.regression.passing / m.regression.cases) * 100) : null
  return (
    <>
      <div className="page-head"><h1>Learning</h1><p>Is the app getting closer to what you would pick yourself? Fewer corrections per job means yes.</p></div>
      <div className="stack gap-xl">
        {!enough && <Rise className="empty"><TrendDown size={34} aria-hidden /><h2>Needs a few approved jobs</h2><p>Approve at least 3 finished resumes, out of 4 or more jobs, and a trend appears here. So far: {n} {n === 1 ? 'job' : 'jobs'}, {approved} approved.</p><Link className="btn primary" to="/jobs">Open your jobs</Link></Rise>}
        <Rise className="stats s3">
          <div className="stat"><b><CountUp value={Math.round(m.approval_rate * 100)} suffix="%" /></b><span>Finished jobs you approved</span></div>
          <div className="stat"><b><CountUp value={m.active_rules} /></b><span>Active rules{m.proposed_rules > 0 && <>, <Link to="/playbook">{m.proposed_rules} suggested</Link></>}</span></div>
          <div className="stat"><b><CountUp value={m.rule_hits} /></b><span>Times a rule shaped a pick</span></div>
          <div className="stat"><b>{pass === null ? 'n/a' : <CountUp value={pass} suffix="%" />}</b><span>{pass === null ? 'No test cases yet' : `Regression pass rate (${m.regression.passing} of ${m.regression.cases})`}</span></div>
          <div className="stat"><b><CountUp value={m.tokens_saved_est} /></b><span>AI tokens saved (estimate)</span></div>
          <div className="stat"><b><CountUp value={proposed || m.proposed_rules} /></b><span>Rules waiting for you</span></div>
        </Rise>
        <Rise as="section" className="panel" aria-label="Corrections per job" i={1}>
          <header><h2>Corrections per job</h2><p>Each bar is one job, oldest first: how many picks you had to change. The dashed line is the trend.{enough && <> Early jobs averaged {first.toFixed(1)}, recent ones {last.toFixed(1)}.</>}</p></header>
          {n === 0 ? <p className="small muted">No jobs yet.</p> : <CorrectionsChart data={m.jobs.map((j, i) => ({ label: `Job ${i + 1}, ${j.company}`, n: j.corrections }))} />}
          <p className="small muted">{enough ? (last < first ? 'It is learning: you correct less than you used to.' : 'No improvement yet. Give reasons on your corrections so it can suggest rules.') : 'Too early to call a trend. This chart is honest: a few jobs prove nothing.'}</p>
        </Rise>
        <Regression compact />
      </div>
    </>
  )
}
