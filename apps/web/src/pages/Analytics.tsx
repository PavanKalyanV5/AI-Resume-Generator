import { ChartBar } from '@phosphor-icons/react'
import { motion } from 'motion/react'
import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import { BarsByDay, StatusMix } from '../components/Charts'
import { CountUp, EASE, Rise, Skel } from '../components/Motion'
import { cost } from '../lib/util'
import { useApp } from '../store'
import type { Stats } from '../types'

export default function Analytics() {
  const [s, setS] = useState<Stats | null>(null)
  const [err, setErr] = useState('')
  const { budget } = useApp()
  useEffect(() => { api.getStats().then(setS, (e) => setErr(e.message)) }, [])
  if (err) return <div className="err-box">{err}</div>
  if (!s) return <div className="stack" style={{ gap: 16 }}><Skel h={96} r={18} /><Skel h={260} r={18} /></div>
  const jobs = s.jobs_by_day.reduce((a, d) => a + d.count, 0)
  const spend = cost(s.tokens_in, s.tokens_out)
  const pct = budget > 0 ? Math.min(100, (spend / budget) * 100) : 0
  const tot = s.tokens_in + s.tokens_out || 1
  const cov = s.avg_coverage <= 1 ? s.avg_coverage * 100 : s.avg_coverage
  return (
    <>
      <div className="page-head"><h1>Analytics</h1><p>Activity over the last 14 days, and what it cost.</p></div>
      {jobs === 0 ? <Rise className="empty"><ChartBar size={34} aria-hidden /><h2>Nothing to chart yet</h2><p>Run a job and its numbers show up here.</p><Link className="btn primary" to="/">Start a job</Link></Rise> : (
        <div className="stack gap-xl">
          <Rise className="stats">
            <div className="stat"><b><CountUp value={jobs} /></b><span>Jobs in 14 days</span></div>
            <div className="stat"><b><CountUp value={cov} suffix="%" /></b><span>Average ATS coverage</span></div>
            <div className="stat"><b><CountUp value={s.tokens_in + s.tokens_out} /></b><span>Tokens used</span></div>
            <div className="stat"><b><CountUp value={spend} prefix="$" decimals={2} /></b><span>Estimated spend</span></div>
          </Rise>
          <Rise as="section" className="panel" aria-label="Jobs per day" i={1}><header><h2>Jobs per day</h2></header><BarsByDay data={s.jobs_by_day} /></Rise>
          <div className="cols">
            <Rise as="section" className="panel" aria-label="Token and cost spend" inView>
              <header><h2>Tokens and spend</h2><p>Cost is an estimate from list prices, not a bill.</p></header>
              <div className="stack" style={{ gap: 10 }}>
                <div className="mix" role="img" aria-label={`${s.tokens_in} in, ${s.tokens_out} out`}>
                  <motion.span style={{ flex: s.tokens_in / tot, background: 'var(--ai)', transformOrigin: 'left' }} initial={{ scaleX: 0 }} animate={{ scaleX: 1 }} transition={{ duration: 0.8, ease: EASE }} />
                  <motion.span style={{ flex: s.tokens_out / tot, background: 'var(--c1)', transformOrigin: 'left' }} initial={{ scaleX: 0 }} animate={{ scaleX: 1 }} transition={{ duration: 0.8, ease: EASE, delay: 0.1 }} />
                </div>
                <div className="legend"><span><i className="sw" style={{ background: 'var(--ai)' }} />Sent {s.tokens_in.toLocaleString('en-US')}</span><span><i className="sw" style={{ background: 'var(--c1)' }} />Received {s.tokens_out.toLocaleString('en-US')}</span></div>
              </div>
              <div className="stack" style={{ gap: 8 }}>
                <div className="row between small"><span>${spend.toFixed(2)} of ${budget.toFixed(2)} budget cap</span><span className="num muted">{Math.round(pct)}%</span></div>
                <div className="bar-line wide" role="progressbar" aria-valuenow={Math.round(pct)} aria-valuemin={0} aria-valuemax={100} aria-label="Budget used"><motion.i className={pct >= 90 ? 'bad' : ''} initial={{ scaleX: 0 }} animate={{ scaleX: pct / 100 }} transition={{ duration: 0.9, ease: EASE, delay: 0.2 }} /></div>
                <Link to="/settings" className="small">Change budget cap</Link>
              </div>
            </Rise>
            <Rise as="section" className="panel" aria-label="Status mix" inView i={1}><header><h2>Status mix</h2></header><StatusMix by={s.by_status} /></Rise>
          </div>
        </div>
      )}
    </>
  )
}
