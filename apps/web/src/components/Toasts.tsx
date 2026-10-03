import { CheckCircle, Info, ShieldWarning, Warning, WarningCircle, X } from '@phosphor-icons/react'
import { AnimatePresence, motion } from 'motion/react'
import { Link } from 'react-router-dom'
import { useApp, type Toast } from '../store'
import { EASE, SPRING } from './Motion'

const ICON = { error: WarningCircle, warn: Warning, success: CheckCircle, info: Info } as const

function Item({ t }: { t: Toast }) {
  const { dismiss, runAction } = useApp()
  const I = t.code === 'redact.leak' ? ShieldWarning : ICON[t.level]
  const acts = (t.actions ?? []).filter((a) => a.action !== 'dismiss').slice(0, 3)
  return (
    <motion.div layout className={`toast ${t.level} ${t.code === 'redact.leak' ? 'critical' : ''}`} role={t.level === 'error' ? 'alert' : 'status'} aria-live={t.level === 'error' ? 'assertive' : 'polite'}
      initial={{ opacity: 0, y: 40, scale: 0.92 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, x: 60, scale: 0.95, transition: { duration: 0.2 } }} transition={SPRING}>
      <I size={20} weight="fill" aria-hidden className="t-ic" />
      <div className="t-tx">
        <b>{t.title}{t.count > 1 && <span className="t-count num" aria-label={`${t.count} times`}>x{t.count}</span>}</b>
        {t.message && <span>{t.message}</span>}
        {t.hint && <span className="t-hint">{t.hint}</span>}
        {t.href && <Link to={t.href} onClick={() => dismiss(t.id)}>View</Link>}
        {t.progress && (
          <span className="t-prog" role="progressbar" aria-valuemin={0} aria-valuemax={t.progress.total} aria-valuenow={t.progress.done} aria-label={t.title}>
            <motion.i initial={false} animate={{ scaleX: t.progress.done / t.progress.total }} transition={{ duration: 0.6, ease: EASE }} />
          </span>
        )}
        {acts.length > 0 && (
          <span className="t-act">{acts.map((a) => <button key={a.label} className="t-btn" onClick={() => runAction(a, { job_id: t.job_id, step: t.step, toast: t.id })}>{a.label}</button>)}</span>
        )}
      </div>
      <button className="t-x" aria-label="Dismiss" onClick={() => dismiss(t.id)}><X size={15} /></button>
      {t.ms > 0 && <motion.i key={t.rev} className="t-life" initial={{ scaleX: 1 }} animate={{ scaleX: 0 }} transition={{ duration: t.ms / 1000, ease: 'linear' }} />}
    </motion.div>
  )
}

export function Toasts() {
  const { toasts } = useApp()
  return (
    <div className="toasts" role="region" aria-label="Notifications">
      <AnimatePresence initial={false}>{toasts.map((t) => <Item key={t.id} t={t} />)}</AnimatePresence>
    </div>
  )
}
