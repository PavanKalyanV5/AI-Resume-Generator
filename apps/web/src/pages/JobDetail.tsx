import { ArrowClockwise, ArrowRight, Seal, Warning, GitDiff, ListChecks, ShieldCheck, Target, XCircle } from '@phosphor-icons/react'
import { AnimatePresence, motion } from 'motion/react'
import { useMemo, useState, type ReactNode } from 'react'
import { Link, NavLink, useLocation, useOutlet, useOutletContext, useParams } from 'react-router-dom'
import { api, mode } from '../api'
import { DiffView } from '../components/DiffView'
import { ApprovedBadge, approveToast, EditLine, TeachWhy } from '../components/Learn'
import { DriveLinks, FileButtons } from '../components/Files'
import { BulletFb, Insights, TrackControl } from '../components/Insights'
import { CountUp, EASE, Skel } from '../components/Motion'
import { SegStrip, Timeline, jobProgress } from '../components/Steps'
import { useJob } from '../lib/useJob'
import { resumeHtml } from '../lib/resumeHtml'
import { fmtDate, jobTitle } from '../lib/util'
import { useApp } from '../store'
import { ErrorNotice } from '../components/ErrorNotice'
import { friendly, isCritical } from '../lib/errors'
import { isActive, isDone, isFailed, needsAttention, type JobDetail, type JobEvent } from '../types'
import { StatusBadge } from './Jobs'

interface Ctx { data: JobDetail; events: JobEvent[]; reload: () => void }
export const useJobCtx = () => useOutletContext<Ctx>()

export default function JobLayout() {
  const { id = '' } = useParams()
  const loc = useLocation()
  const { data, error, reload } = useJob(id)
  const { events, refresh, toast, link } = useApp()
  const [busy, setBusy] = useState(false)
  const ctx = data ? ({ data, events: events[id] ?? [], reload: () => { void refresh(); reload() } } satisfies Ctx) : undefined
  const outlet = useOutlet(ctx)
  if (error && !data) return <div className="empty"><h2>Job not found</h2><p>{error}</p><Link className="btn" to="/jobs">All jobs</Link></div>
  if (!data) return <div className="stack" style={{ gap: 16 }}><Skel h={44} w="55%" r={10} /><Skel h={10} r={6} /><Skel h={340} r={18} /></div>
  const j = data.job
  const act = async (fn: () => Promise<void>) => {
    setBusy(true)
    try { await fn(); await refresh(); reload() } catch (e) { toast({ kind: 'err', text: e instanceof Error ? e.message : 'That did not work.' }) } finally { setBusy(false) }
  }
  const pct = jobProgress(data.steps)
  const blocked = typeof j.error === 'object' && !!j.error && isCritical(j.error.code)
  return (
    <>
      <div className="job-head">
        <div className="row between jh-row">
          <div className="stack" style={{ gap: 10, minWidth: 0 }}>
            <h1 className="job-title">{j.role || j.company ? <><span>{j.role ?? 'Tailored resume'}</span>{j.company && <small>{j.company}</small>}</> : jobTitle(j)}</h1>
            <div className="row" style={{ gap: 10 }}>
              <StatusBadge s={j.status} />
              <span className="small muted">{j.provider} provider, started {fmtDate(j.created_at)}</span>
              {link[id] === 'reconnecting' && <span className="badge warn">Reconnecting</span>}
            </div>
          </div>
          <div className="row actions">
            {isActive(j.status) && <button className="btn" disabled={busy} onClick={() => act(() => api.cancel(id))}><XCircle size={16} aria-hidden />Cancel</button>}
            {isFailed(j.status) && !blocked && <button className="btn primary" disabled={busy} onClick={() => act(() => api.retry(id))}><ArrowClockwise size={16} aria-hidden />Retry</button>}
            <FileButtons id={j.id} files={data.files} />
            <DriveLinks links={data.drive} />
            {isDone(j.status) && (data.approved ? <ApprovedBadge /> : <button className="btn" disabled={busy} onClick={() => act(async () => { await api.approveJob(id); toast({ kind: 'ok', text: approveToast }) })}><Seal size={16} aria-hidden />Approve this resume</button>)}
            {isDone(j.status) && <TrackControl id={j.id} />}
          </div>
        </div>
        {j.error && isFailed(j.status) && (typeof j.error === 'string' ? <ErrorNotice message={j.error} /> : (
          <ErrorNotice f={friendly(j.error.code, j.error, j.id, j.error.step)} onRetry={() => act(() => api.retry(id, (j.error as { step?: string }).step))} />
        ))}
        <div className="prog">
          <SegStrip steps={data.steps} />
          <span className="small muted prog-n"><CountUp value={pct} suffix="%" /> {isActive(j.status) ? 'done' : isFailed(j.status) ? 'stopped' : 'complete'}</span>
        </div>
        <nav className="tabs" aria-label="Job sections">
          {[{ to: `/jobs/${id}`, end: true, I: GitDiff, t: 'Result' }, { to: `/jobs/${id}/match`, I: Target, t: 'Match' }, { to: `/jobs/${id}/review`, I: ListChecks, t: 'Review' }, { to: `/jobs/${id}/privacy`, I: ShieldCheck, t: 'Privacy audit' }].map((x) => (
            <NavLink key={x.to} to={x.to} end={x.end}>
              {({ isActive: on }) => <>{on && <motion.span layoutId="jobtab" className="tab-ul" transition={{ type: 'spring', stiffness: 500, damping: 38 }} />}<x.I size={16} aria-hidden />{x.t}</>}
            </NavLink>
          ))}
        </nav>
      </div>
      <AnimatePresence mode="wait" initial={false}>
        <motion.div key={loc.pathname} initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} transition={{ duration: 0.28, ease: EASE }}>{outlet}</motion.div>
      </AnimatePresence>
    </>
  )
}

export function JobOverview() {
  const { data, events, reload } = useJobCtx()
  const { refresh, toast } = useApp()
  const [tab, setTab] = useState<'diff' | 'pdf'>('diff')
  const [busy, setBusy] = useState(false)
  const html = useMemo(() => (data.tailored ? resumeHtml(data.tailored) : ''), [data.tailored])
  const retry = async (from: string) => {
    setBusy(true)
    try { await api.retry(data.job.id, from); await refresh() } catch (e) { toast({ kind: 'err', text: e instanceof Error ? e.message : 'Retry failed.' }) } finally { setBusy(false) }
  }
  // The result shows a subset of the resume; feedback ids are positions in the full one.
  const keep = data.selection?.bullets
  const fbId = (p: number, i: number) => {
    if (!keep) return `e${p}.b${i}`
    const r = keep.map((b, k) => (b.length ? k : -1)).filter((k) => k >= 0)[p]
    return r === undefined || keep[r][i] === undefined ? null : `e${r}.b${keep[r][i]}`
  }
  const active = isActive(data.job.status)
  const attn = needsAttention(data.review)
  const edited = new Set((data.corrections ?? []).filter((c) => c.action === 'edit').map((c) => c.item_id))
  const editLine = data.tailored && (isDone(data.job.status) || active) ? (p: number, i: number, raw: string, node: ReactNode) => {
    const path = p < 0 ? `summary.${i}` : fbId(p, i)
    return path ? <EditLine job={data.job.id} path={path} raw={raw} node={node} edited={edited.has(path)} locked={active} onSaved={reload} /> : node
  } : undefined
  const pdfReady = data.files.includes('resume.pdf')
  return (
    <>
      {attn.length > 0 && <Link className="attn" to={`/jobs/${data.job.id}/review`}><Warning size={18} aria-hidden /><span><b>Needs your attention.</b> {attn.length} {attn.length === 1 ? 'item' : 'items'} from the plan check {attn.length === 1 ? 'is' : 'are'} waiting for you.</span><span className="attn-go">Open Review <ArrowRight size={14} aria-hidden /></span></Link>}
      <div className="split">
      <section className="stack" style={{ minWidth: 0, gap: 16 }} aria-label="Result">
        <div className="pills" role="tablist" aria-label="Result view">
          {([['diff', 'Original and tailored'], ['pdf', 'PDF preview']] as const).map(([k, t]) => (
            <button key={k} role="tab" aria-selected={tab === k} onClick={() => setTab(k)}>{tab === k && <motion.span layoutId="rtab" className="pill-bg" transition={{ type: 'spring', stiffness: 500, damping: 38 }} />}<span>{t}</span></button>
          ))}
        </div>
        {tab === 'diff' ? (
          data.tailored ? <DiffView original={data.original} tailored={data.tailored} edit={editLine} bullets={isDone(data.job.status) ? (p, i) => { const id = fbId(p, i); return id ? <BulletFb job={data.job.id} id={id} /> : null } : undefined} /> : (
            <div className="empty"><h2>{isFailed(data.job.status) ? 'No tailored version yet' : 'Waiting on the AI step'}</h2>
              <p>{isFailed(data.job.status) ? 'Retry the failed step to produce one. Your original is untouched.' : 'The tailored resume appears here with every changed word marked.'}</p>
              {active && <div className="stack" style={{ gap: 8, width: '100%' }}><Skel h={14} w="90%" /><Skel h={14} w="75%" /><Skel h={14} w="82%" /></div>}</div>
          )
        ) : mode.demo ? (
          data.tailored ? <iframe className="pdf" title="Tailored resume preview (demo rendering)" srcDoc={html} /> : <div className="empty"><p>The preview appears once the PDF step finishes.</p></div>
        ) : pdfReady ? <iframe className="pdf" title="Tailored resume PDF" src={api.fileUrl(data.job.id, 'resume.pdf')} />
          : <div className="empty"><p>The preview appears once the PDF step finishes.</p></div>}
      </section>
      <section className="tl-panel" aria-label="Progress">
        <header>
          <h2>Progress</h2>
          <p>{data.steps.some((s) => s.name === 'upload_drive') ? 'Local steps stay on this machine. Two send redacted text to the AI, and one saves to your Google Drive.' : 'Local steps stay on this machine. Two send redacted text to the AI.'}</p>
        </header>
        <Timeline steps={data.steps} events={events} jobStatus={data.job.status} onRetry={retry} busy={busy || (typeof data.job.error === 'object' && !!data.job.error && isCritical(data.job.error.code))} />
      </section>
      </div>
      {data.tailored && <TeachWhy corrections={data.corrections} />}
      {(data.rules_applied?.length ?? 0) > 0 && (
        <section className="panel rules-used" aria-label="Rules used" style={{ marginTop: 16 }}>
          <header><h2>Rules used</h2><p>Your playbook rules that shaped this resume.</p></header>
          <ul className="clean stack" style={{ gap: 10 }}>{data.rules_applied!.map((r) => <li key={r.rule_id} className="small"><Link to={`/playbook#${r.rule_id}`}><b>{r.text || r.rule_id}</b></Link><span className="muted">{r.effect}</span></li>)}</ul>
        </section>
      )}
      {data.tailored && <Insights data={data} />}
    </>
  )
}
