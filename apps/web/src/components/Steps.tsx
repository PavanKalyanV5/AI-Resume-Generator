import { ArrowClockwise, CircleNotch, Check, CloudArrowUp, ShieldCheck, UploadSimple, X } from '@phosphor-icons/react'
import { AnimatePresence, motion } from 'motion/react'
import { BASE_STEPS, STEP_INFO, isActive, isDone, type JobEvent, type Step, type Zone } from '../types'
import { dur } from '../lib/util'
import { EASE, SPRING } from './Motion'

const ZONE_ICON = { local: ShieldCheck, ai: UploadSimple, drive: CloudArrowUp } as const
const ZONE_LABEL: Record<Zone, string> = { local: 'Local', ai: 'Sent to AI (redacted)', drive: 'Sent to Google Drive (your account)' }

export const Where = ({ zone }: { zone: Zone }) => {
  const I = ZONE_ICON[zone]
  return <span className={`badge z-${zone}`}><I size={13} aria-hidden />{ZONE_LABEL[zone]}</span>
}

/** The three places data can be. Shown as a legend so the colours mean the same thing everywhere. */
export function ZoneLegend({ drive = true }: { drive?: boolean }) {
  return (
    <div className="zlegend">
      <Where zone="local" /><Where zone="ai" />{drive && <Where zone="drive" />}
    </div>
  )
}

/** Overview rail: one dot per step, one amber, and a blue one when Drive is switched on. */
export function StepRail({ drive }: { drive: boolean }) {
  const list = drive ? [...BASE_STEPS, 'upload_drive'] : [...BASE_STEPS]
  return (
    <div className="rail" role="img" aria-label={`${list.length} steps. ${drive ? 'Two steps send redacted text to the AI and the last step saves to your Google Drive.' : 'Two steps send redacted text to the AI.'} The rest run on this machine.`}>
      <AnimatePresence initial={false} mode="popLayout">
        {list.map((s, i) => (
          <motion.span key={s} layout className={`rail-seg z-${STEP_INFO[s].zone}`} initial={{ opacity: 0, scaleX: 0.4 }} animate={{ opacity: 1, scaleX: 1 }} exit={{ opacity: 0, scaleX: 0.4 }}
            transition={{ ...SPRING, delay: i * 0.025 }} title={STEP_INFO[s].title} />
        ))}
      </AnimatePresence>
    </div>
  )
}

const frac = (s: Step) => (isDone(s.status) ? 1 : 0)

/** Live progress across all steps: segments fill as steps finish. Transform-only. */
export function SegStrip({ steps }: { steps: Step[] }) {
  return (
    <div className="seg-strip" role="progressbar" aria-label="Job progress" aria-valuemin={0} aria-valuemax={steps.length} aria-valuenow={steps.filter((s) => isDone(s.status)).length}>
      {steps.map((s) => {
        const z = STEP_INFO[s.name]?.zone ?? 'local'
        return (
          <span key={s.name} className={`seg z-${z} ${s.status}`} title={STEP_INFO[s.name]?.title}>
            <motion.i initial={false} animate={{ scaleX: frac(s) }} transition={{ duration: 0.7, ease: EASE }} />
            {s.status === 'running' && <b className="sweep" />}
          </span>
        )
      })}
    </div>
  )
}

export const jobProgress = (steps: Step[]) => {
  if (!steps.length) return 0
  const d = steps.filter((s) => isDone(s.status)).length + steps.filter((s) => s.status === 'running').length * 0.5
  return Math.round((d / steps.length) * 100)
}

export function Timeline({ steps, events, jobStatus, onRetry, busy }: { steps: Step[]; events: JobEvent[]; jobStatus: string; onRetry: (from: string) => void; busy: boolean }) {
  return (
    <ol className="tl clean">
      {steps.map((s, idx) => {
        const info = STEP_INFO[s.name] ?? { title: s.name, blurb: '', zone: 'local' as Zone, tag: 'Local' }
        const evs = events.filter((e) => e.step === s.name)
        const last = evs[evs.length - 1]
        return (
          <li key={s.name} className={`z-${info.zone} ${s.status} ${info.zone !== 'local' ? 'cross' : ''}`}>
            {idx < steps.length - 1 && <span className="conn" aria-hidden><motion.i initial={false} animate={{ scaleY: isDone(s.status) ? 1 : 0 }} transition={{ duration: 0.6, ease: EASE }} /></span>}
            <span className="node" aria-hidden>
              <AnimatePresence mode="popLayout" initial={false}>
                <motion.span key={s.status} initial={{ scale: 0.3, opacity: 0 }} animate={{ scale: 1, opacity: 1 }} exit={{ scale: 0.3, opacity: 0 }} transition={SPRING} className="node-i">
                  {isDone(s.status) ? <Check size={14} weight="bold" /> : s.status === 'running' ? <CircleNotch className="spin" size={14} weight="bold" /> : s.status === 'failed' ? <X size={14} weight="bold" /> : <i className="pip" />}
                </motion.span>
              </AnimatePresence>
            </span>
            <div className="step-body">
              <div className="row tl-head">
                <h3>{info.title}</h3>
                <span className="tiny faint num">{dur(s.started_at, s.finished_at)}</span>
                {s.attempts > 1 && <span className="badge">Attempt {s.attempts}</span>}
                <span className="sr">{s.status}</span>
              </div>
              {info.zone !== 'local' && <Where zone={info.zone} />}
              {s.status !== 'failed' && <p className="msg" aria-live={s.status === 'running' ? 'polite' : undefined}>{last ? last.message : info.blurb}</p>}
              {s.status === 'failed' && (
                <div className="err-box stack" style={{ gap: 8 }}>
                  <span>{s.error ?? 'This step failed.'}</span>
                  <span><button className="btn sm" disabled={busy || isActive(jobStatus)} onClick={() => onRetry(s.name)}><ArrowClockwise size={14} aria-hidden />Retry from this step</button></span>
                </div>
              )}
              {s.status !== 'failed' && isDone(s.status) && !isActive(jobStatus) && s.name !== 'parse_jd' && (
                <button className="link-btn tiny rerun" disabled={busy} onClick={() => onRetry(s.name)}>Re-run from here</button>
              )}
              {evs.length > 1 && (
                <details className="tiny muted"><summary>{evs.length} log lines</summary>
                  <ul className="clean mono logs">{evs.map((e) => <li key={e.seq}>{new Date(e.ts).toLocaleTimeString()} {e.message}</li>)}</ul>
                </details>
              )}
            </div>
          </li>
        )
      })}
    </ol>
  )
}
