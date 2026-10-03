import { ArrowClockwise, Info, ShieldWarning, Warning, WarningOctagon } from '@phosphor-icons/react'
import { motion } from 'motion/react'
import { Link } from 'react-router-dom'
import { friendly, fromError, type Friendly } from '../lib/errors'

/** Friendly error block: what happened, what to do, the right action, and the raw code tucked behind Details. */
export function ErrorNotice({ f, error, code, message, jobId, onRetry, compact }: {
  f?: Friendly; error?: unknown; code?: string | null; message?: string; jobId?: string; onRetry?: () => void; compact?: boolean
}) {
  const x = f ?? (error !== undefined ? fromError(error, jobId) : friendly(code, { message }, jobId))
  const Icon = x.critical ? ShieldWarning : x.level === 'info' ? Info : x.level === 'warn' ? Warning : WarningOctagon
  const settings = x.actions.some((a) => a.action === 'settings'), pii = x.actions.some((a) => a.target === '/pii')
  return (
    <motion.div className={`errn ${x.level} ${x.critical ? 'critical' : ''} ${compact ? 'compact' : ''}`} role={x.level === 'info' ? 'status' : 'alert'}
      initial={{ opacity: 0, y: -6 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.3 }}>
      <Icon size={22} weight={x.critical ? 'fill' : 'regular'} aria-hidden className="errn-ic" />
      <div className="errn-b">
        <h4>{x.title}</h4>
        <p>{x.known ? x.message : message ?? x.message}</p>
        <p className="errn-hint">{x.hint}</p>
        {(x.retryable && onRetry || settings || pii) && (
          <div className="row errn-act">
            {x.retryable && onRetry && <button className="btn sm" onClick={onRetry}><ArrowClockwise size={14} aria-hidden />Retry from this step</button>}
            {settings && <Link className="btn sm" to="/settings">Open Settings</Link>}
            {pii && <Link className="btn sm" to="/pii">Open PII ledger</Link>}
          </div>
        )}
        {!x.known || x.code !== 'unknown' ? <details className="errn-d"><summary>Details</summary><code>{x.code}</code>{!x.known && message ? <span>{message}</span> : null}</details> : null}
      </div>
    </motion.div>
  )
}
