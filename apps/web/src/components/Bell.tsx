import { Bell as BellIc, CheckCircle, Info, Warning, WarningCircle } from '@phosphor-icons/react'
import { AnimatePresence, motion } from 'motion/react'
import { useEffect, useMemo, useRef } from 'react'
import { useNavigate } from 'react-router-dom'
import { ago } from '../lib/util'
import { useApp } from '../store'
import type { Notif } from '../types'
import { SPRING } from './Motion'

const ICON = { error: WarningCircle, warn: Warning, success: CheckCircle, info: Info } as const
const dayLabel = (iso: string) => {
  const d = new Date(iso), t = new Date(), y = new Date(Date.now() - 864e5)
  const same = (a: Date, b: Date) => a.toDateString() === b.toDateString()
  return same(d, t) ? 'Today' : same(d, y) ? 'Yesterday' : d.toLocaleDateString(undefined, { weekday: 'long', month: 'short', day: 'numeric' })
}

export function BellButton() {
  const { unread, notifOpen, setNotifOpen } = useApp()
  return (
    <button className="icon-btn bell" onClick={() => setNotifOpen(!notifOpen)} aria-label={`Notifications, ${unread} unread`} aria-haspopup="dialog" aria-expanded={notifOpen}>
      <BellIc size={18} />
      <AnimatePresence>{unread > 0 && <motion.span key="n" className="dot-count" initial={{ scale: 0 }} animate={{ scale: 1 }} exit={{ scale: 0 }} transition={{ type: 'spring', stiffness: 600, damping: 22 }}>{unread > 9 ? '9+' : unread}</motion.span>}</AnimatePresence>
    </button>
  )
}

export function NotifCenter() {
  const { notifs, markRead, setNotifOpen, runAction } = useApp()
  const nav = useNavigate()
  const first = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    first.current?.focus()
    const k = (e: KeyboardEvent) => { if (e.key === 'Escape') setNotifOpen(false) }
    window.addEventListener('keydown', k); return () => window.removeEventListener('keydown', k)
  }, [setNotifOpen])
  const groups = useMemo(() => {
    const m = new Map<string, Notif[]>()
    ;[...notifs].sort((a, b) => b.ts.localeCompare(a.ts)).forEach((n) => m.set(dayLabel(n.ts), [...(m.get(dayLabel(n.ts)) ?? []), n]))
    return [...m.entries()]
  }, [notifs])
  const open = (n: Notif) => { markRead(n.seq); if (n.job_id) { setNotifOpen(false); nav(`/jobs/${n.job_id}`) } }
  const unread = notifs.filter((n) => !n.read).length
  return (
    <>
      <motion.div className="scrim" onClick={() => setNotifOpen(false)} initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.2 }} />
      <motion.section className="ncenter" role="dialog" aria-modal="true" aria-label="Notifications" initial={{ opacity: 0, y: -12, scale: 0.97 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: -8, scale: 0.98 }} transition={SPRING}>
        <header>
          <h2>Notifications</h2>
          <button ref={first} className="link-btn" disabled={unread === 0} onClick={() => markRead()}>Mark all read</button>
        </header>
        <div className="nlist">
          {groups.length === 0 && <div className="empty"><p>Nothing yet. Job results and problems show up here.</p></div>}
          {groups.map(([day, list]) => (
            <section key={day} aria-label={day}>
              <h3>{day}</h3>
              <ul className="clean">
                {list.map((n) => {
                  const I = ICON[n.level]
                  return (
                    <li key={n.seq} className={`nitem ${n.level} ${n.read ? '' : 'unread'}`}>
                      <button className="nmain" onClick={() => open(n)}>
                        <I size={20} weight="fill" aria-hidden className="n-ic" />
                        <span className="n-t"><b>{n.title}{!n.read && <span className="sr"> (unread)</span>}</b><span>{n.message}</span>{n.hint && <span className="faint">{n.hint}</span>}</span>
                        <time className="tiny faint" dateTime={n.ts}>{ago(n.ts)}</time>
                      </button>
                      {n.actions?.filter((a) => a.action !== 'dismiss').length ? (
                        <div className="n-act">{n.actions.filter((a) => a.action !== 'dismiss').map((a) => <button key={a.label} className="btn sm" onClick={() => { markRead(n.seq); setNotifOpen(false); void runAction(a, { job_id: n.job_id, step: n.step }) }}>{a.label}</button>)}</div>
                      ) : null}
                    </li>
                  )
                })}
              </ul>
            </section>
          ))}
        </div>
      </motion.section>
    </>
  )
}
