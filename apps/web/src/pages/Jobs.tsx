import { ArrowRight, ListChecks } from '@phosphor-icons/react'
import { motion } from 'motion/react'
import { Link } from 'react-router-dom'
import { EASE, Rise, Skel } from '../components/Motion'
import { errText, fmtDate, jobTitle } from '../lib/util'
import { useApp } from '../store'
import { isActive, isDone, isFailed } from '../types'

export const statusClass = (s: string) => (isActive(s) ? 'run' : isDone(s) ? 'ok' : isFailed(s) ? 'err' : '')
export const StatusBadge = ({ s }: { s: string }) => <span className={`badge st ${statusClass(s)}`}>{isActive(s) && <i className="pulse" aria-hidden />}{s}</span>

export default function Jobs() {
  const { jobs, loaded } = useApp()
  return (
    <>
      <div className="page-head"><h1>Jobs</h1><p>Every tailoring run, newest first.</p></div>
      {!loaded ? <div className="stack" style={{ gap: 2 }}>{[0, 1, 2, 3].map((i) => <Skel key={i} h={68} r={14} />)}</div> : jobs.length === 0 ? (
        <Rise className="empty"><ListChecks size={34} aria-hidden /><h2>No jobs yet</h2><p>Add your resume and a job description to tailor your first one.</p><Link className="btn primary" to="/">Start a job</Link></Rise>
      ) : (
        <ul className="clean joblist">
          {jobs.map((j, i) => (
            <motion.li key={j.id} initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.5, ease: EASE, delay: Math.min(i, 8) * 0.04 }}>
              <Link className="jobrow" to={`/jobs/${j.id}`}>
                <span className="jr-main"><span className="jr-title">{jobTitle(j)}</span>
                  <span className="small muted">{j.provider} provider, {fmtDate(j.created_at)}{j.error ? `, ${errText(j.error)}` : ''}</span></span>
                <StatusBadge s={j.status} />
                <ArrowRight className="jr-go" size={16} aria-hidden />
              </Link>
            </motion.li>
          ))}
        </ul>
      )}
    </>
  )
}
