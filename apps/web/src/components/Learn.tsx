import { PencilSimple, Seal } from '@phosphor-icons/react'
import { useState, type ReactNode } from 'react'
import { api } from '../api'
import { useApp } from '../store'
import { REASONS, type Correction } from '../types'

const msg = (e: unknown, d: string) => (e instanceof Error ? e.message : d)

/** One-click reason chips. Each click saves the whole set; the owner can toggle any of them off again. */
export function ReasonChips({ id, tags = [] }: { id: number; tags?: string[] }) {
  const [on, setOn] = useState(tags)
  const { toast } = useApp()
  const flip = async (t: string) => {
    const next = on.includes(t) ? on.filter((x) => x !== t) : [...on, t], prev = on
    setOn(next)
    try { await api.setReason(id, next) } catch (e) { setOn(prev); toast({ kind: 'err', text: msg(e, 'Could not save the reason.') }) }
  }
  return (
    <span className="chips reasons" role="group" aria-label="Why did you change this?">
      {REASONS.map(([k, l]) => <button key={k} type="button" className={`chip pick ${on.includes(k) ? 'hit' : ''}`} aria-pressed={on.includes(k)} onClick={() => flip(k)}>{l}</button>)}
    </span>
  )
}

const noun = (c: Correction) => (c.item.kind === 'cert' ? 'certification' : c.item.kind === 'project' ? 'project' : c.area === 'roles' ? 'role' : c.area)
export const describe = (c: Correction) =>
  c.action === 'edit' ? (c.item_id.startsWith('summary') ? `Edited summary line ${Number(c.item_id.slice(8)) + 1}` : `Edited a bullet (${c.item_id})`)
    : c.action === 'reorder' ? 'Reordered skills' : `${c.action === 'add' ? 'Added' : 'Removed'} ${noun(c)}${c.item.title ? `: ${c.item.title}` : ''}`

/** "Teach it why": the job's corrections, each with reason chips. Optional, never blocks anything. */
export function TeachWhy({ corrections }: { corrections?: Correction[] }) {
  const list = (corrections ?? []).filter((c) => c.action !== 'keep')
  if (!list.length) return null
  return (
    <section className="panel teach" aria-label="Teach it why">
      <header><h2>Teach it why</h2><p>You changed {list.length === 1 ? 'one thing' : `${list.length} things`}. Tell the app why, and it can suggest a rule. Nothing changes until you approve a rule.</p></header>
      <ul className="clean stack" style={{ gap: 14 }}>{list.map((c) => <li key={c.id} className="stack" style={{ gap: 8 }}><span className="small"><b>{describe(c)}</b></span><ReasonChips id={c.id} tags={c.reason_tags} /></li>)}</ul>
    </section>
  )
}

/** A tailored line the owner can click to edit. The textarea holds the raw text, so **bold** markers survive a save. */
export function EditLine({ job, path, raw, node, edited, locked, onSaved }: { job: string; path: string; raw: string; node: ReactNode; edited: boolean; locked: boolean; onSaved: () => void }) {
  const [open, setOpen] = useState(false)
  const [val, setVal] = useState(raw)
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState('')
  const [cid, setCid] = useState<number | null>(null)
  const save = async () => {
    setBusy(true); setErr('')
    try { const r = await api.editLine(job, path, val); setCid(r.correction_id); setOpen(false); onSaved() } catch (e) { setErr(msg(e, 'Could not save that edit.')) } finally { setBusy(false) }
  }
  if (open) {
    return (
      <span className="edit-box">
        <textarea autoFocus aria-label="Edit this line" value={val} onChange={(e) => setVal(e.target.value)} onKeyDown={(e) => { if (e.key === 'Escape') setOpen(false); if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) void save() }} />
        <span className="small muted">Use **double asterisks** for bold. Ctrl+Enter saves.</span>
        {err && <span className="field-err" role="alert">{err}</span>}
        <span className="row" style={{ gap: 8 }}>
          <button type="button" className="btn sm primary" disabled={busy || !val.trim() || val === raw} onClick={save}>{busy ? 'Saving...' : 'Save edit'}</button>
          <button type="button" className="btn sm ghost" onClick={() => setOpen(false)}>Cancel</button>
        </span>
      </span>
    )
  }
  return (
    <>
      <span className="ed" role="button" tabIndex={0} aria-disabled={locked} title={locked ? 'Wait for the re-render to finish' : 'Click to edit this line'} onClick={() => { if (!locked) { setVal(raw); setOpen(true) } }} onKeyDown={(e) => { if (e.key === 'Enter' && !locked) { setVal(raw); setOpen(true) } }}>{node}<PencilSimple size={12} aria-hidden className="ed-ic" /></span>
      {edited && <span className="badge ok ed-b">Edited by you</span>}
      {cid !== null && <span className="edit-why"><span className="small muted">Why? </span><ReasonChips id={cid} /></span>}
    </>
  )
}

export const approveToast = 'Approved. It is now a test case: the app checks every future rule change against the picks you approved here.'
export const ApprovedBadge = () => <span className="badge ok"><Seal size={14} aria-hidden />Approved</span>
