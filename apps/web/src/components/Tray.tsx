import { ArrowClockwise, ArrowUpRight, X } from '@phosphor-icons/react'
import { AnimatePresence, motion } from 'motion/react'
import { useEffect, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import { useApp } from '../store'
import { STEPS, isActive, isDone, isFailed, type JobMeta } from '../types'
import { errText, fmtDate, jobTitle } from '../lib/util'
import { EASE, SPRING } from './Motion'
import { StatusBadge } from '../pages/Jobs'

function progress(j: JobMeta, last?: string) {
  if (isDone(j.status)) return 100
  const i = last ? STEPS.indexOf(last as (typeof STEPS)[number]) : -1
  return Math.min(96, Math.round(((i + 0.5) / 8) * 100))
}

export function Tray() {
  const { jobs, events, setTray, refresh, link, toast } = useApp()
  const close = useRef<HTMLButtonElement>(null)
  const [busy, setBusy] = useState('')
  useEffect(() => {
    close.current?.focus()
    const k = (e: KeyboardEvent) => { if (e.key === 'Escape') setTray(false) }
    window.addEventListener('keydown', k)
    return () => window.removeEventListener('keydown', k)
  }, [setTray])
  const act = async (id: string, fn: () => Promise<void>) => {
    setBusy(id)
    try { await fn(); await refresh() } catch (e) { toast({ kind: 'err', text: e instanceof Error ? e.message : 'That did not work.' }) } finally { setBusy('') }
  }
  const active = jobs.filter((j) => isActive(j.status))
  const recent = jobs.filter((j) => !isActive(j.status)).slice(0, 5)
  const item = (j: JobMeta) => {
    const ev = events[j.id] ?? []
    const last = ev[ev.length - 1]
    const p = progress(j, last?.step)
    return (
      <motion.li key={j.id} layout className="tjob" initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, x: 24 }} transition={SPRING}>
        <div className="row between nowrap">
          <Link className="t" to={`/jobs/${j.id}`} onClick={() => setTray(false)}>{jobTitle(j)}</Link>
          <StatusBadge s={j.status} />
        </div>
        {isActive(j.status) && <div className="bar-line wide" role="progressbar" aria-valuenow={p} aria-valuemin={0} aria-valuemax={100} aria-label="Job progress"><motion.i animate={{ scaleX: p / 100 }} initial={{ scaleX: 0 }} transition={{ duration: 0.8, ease: EASE }} /></div>}
        <p className="small muted">{last && isActive(j.status) ? last.message : errText(j.error) || `${j.provider}, ${fmtDate(j.created_at)}`}</p>
        {link[j.id] === 'reconnecting' && isActive(j.status) && <p className="tiny" style={{ color: 'var(--ai)' }}>Connection lost. Reconnecting and picking up where it left off.</p>}
        <div className="row">
          {isActive(j.status) && <button className="btn sm" disabled={busy === j.id} onClick={() => act(j.id, () => api.cancel(j.id))}>Cancel</button>}
          {isFailed(j.status) && <button className="btn sm" disabled={busy === j.id} onClick={() => act(j.id, () => api.retry(j.id))}><ArrowClockwise size={14} aria-hidden />Retry</button>}
          <Link className="btn sm ghost" to={`/jobs/${j.id}`} onClick={() => setTray(false)}>Open<ArrowUpRight size={14} aria-hidden /></Link>
        </div>
      </motion.li>
    )
  }
  return (
    <>
      <motion.div className="scrim" onClick={() => setTray(false)} initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.25 }} />
      <motion.aside className="tray" role="dialog" aria-label="Activity" aria-modal="true" initial={{ x: '104%' }} animate={{ x: 0 }} exit={{ x: '104%' }} transition={{ type: 'spring', stiffness: 320, damping: 34 }}>
        <header><h2>Activity</h2><button ref={close} className="icon-btn" aria-label="Close activity" onClick={() => setTray(false)}><X size={18} /></button></header>
        <div className="list">
          {active.length === 0 && recent.length === 0 && <div className="empty"><p>No jobs yet. Start one from Intake and it will show up here.</p></div>}
          {active.length > 0 && <h3>Running</h3>}
          <ul className="clean"><AnimatePresence initial={false}>{active.map(item)}</AnimatePresence></ul>
          {recent.length > 0 && <h3>Recent</h3>}
          <ul className="clean"><AnimatePresence initial={false}>{recent.map(item)}</AnimatePresence></ul>
        </div>
      </motion.aside>
    </>
  )
}
