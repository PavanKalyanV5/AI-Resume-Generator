import { ArrowRight, Check, Eye, EyeSlash, Lock, Plus, ShieldCheck, Trash, X } from '@phosphor-icons/react'
import { AnimatePresence, motion } from 'motion/react'
import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react'
import { api } from '../api'
import { EASE, Redact, Rise, SPRING, Skel } from '../components/Motion'
import { useApp } from '../store'
import type { PiiData, PiiEntry, PiiKind } from '../types'

const REVEAL_MS = 10000
export const KIND: Record<PiiKind, { label: string; plural: string; cls: string }> = {
  person: { label: 'Name', plural: 'Names', cls: 'k1' }, email: { label: 'Email', plural: 'Emails', cls: 'k2' }, phone: { label: 'Phone', plural: 'Phones', cls: 'k3' },
  org: { label: 'Employer', plural: 'Employers', cls: 'k4' }, client: { label: 'Client', plural: 'Clients', cls: 'k5' }, custom: { label: 'Custom', plural: 'Custom', cls: 'k6' },
}
const AREA: Record<string, string> = { resume: 'Resume', experience: 'Experience', projects: 'Projects', education: 'Education', skills: 'Skills', certifications: 'Certifications', summary: 'Summary', jd: 'Job description' }
const SOURCE: Record<PiiEntry['source'], string> = { profile: 'From your profile', experience: 'Found in experience', always: 'Always redact list', job: 'From a job' }
const CKIND: Record<string, string> = { email: 'Email address', phone: 'Phone number', name: 'Possible name', url_handle: 'Profile handle' }
const kindOf = (k: string): PiiKind => (k in KIND ? (k as PiiKind) : 'custom')

const Mask = ({ text, delay = 0 }: { text: string; delay?: number }) => (
  <span className="pmask" aria-label={`Masked value, ${text.length} characters shown as ${text}`}>
    <motion.i aria-hidden initial={{ scaleX: 0 }} animate={{ scaleX: 1 }} transition={{ duration: 0.7, ease: EASE, delay }} />
    <span aria-hidden>{text}</span>
  </span>
)

function Ring({ ms }: { ms: number }) {
  return (
    <svg className="ring" width="20" height="20" viewBox="0 0 20 20" aria-hidden>
      <circle cx="10" cy="10" r="8" className="ring-bg" />
      <circle cx="10" cy="10" r="8" className="ring-fg" style={{ animationDuration: `${ms}ms` }} />
    </svg>
  )
}

function Where({ e }: { e: PiiEntry }) {
  const max = Math.max(1, ...e.occurrences.map((o) => o.count))
  return (
    <ul className="clean where" aria-label={`Used ${e.total} times`}>
      {e.occurrences.map((o) => (
        <li key={o.area} title={`${AREA[o.area] ?? o.area}: ${o.count}`}>
          <span className="w-l">{AREA[o.area] ?? o.area}</span>
          <span className="w-b"><motion.i initial={{ scaleX: 0 }} animate={{ scaleX: o.count / max }} transition={{ duration: 0.7, ease: EASE, delay: 0.2 }} /></span>
          <span className="w-n num">{o.count}</span>
        </li>
      ))}
    </ul>
  )
}

function CandRow({ c, busy, run, say }: { c: PiiData['candidates'][number]; busy: boolean; run: (id: string, f: () => Promise<unknown>, ok: string) => void; say: (s: string) => void }) {
  const [val, setVal] = useState<string | null>(null)
  const [rb, setRb] = useState(false)
  const [err, setErr] = useState('')
  const timer = useRef(0)
  const hide = useCallback((announce = true) => { clearTimeout(timer.current); setVal(null); if (announce) say('Candidate value is masked again.') }, [say])
  useEffect(() => {
    const v = () => { if (document.hidden) hide(false) }
    document.addEventListener('visibilitychange', v)
    return () => { document.removeEventListener('visibilitychange', v); clearTimeout(timer.current) }
  }, [hide])
  const reveal = async () => {
    setRb(true); setErr('')
    try {
      const r = await api.revealPii(c.id)
      setVal(r.value); say('Candidate value is shown. It will be hidden in 10 seconds.')
      clearTimeout(timer.current); timer.current = window.setTimeout(() => hide(), REVEAL_MS)
    } catch (x) { setErr(x instanceof Error ? x.message : 'Could not reveal this value.') } finally { setRb(false) }
  }
  return (
    <motion.li layout exit={{ opacity: 0, x: -40, scale: 0.97 }} transition={SPRING} className="cand">
      <div className="grow">
        {val !== null ? <span className="p-real">{val || 'Empty value'}</span> : <Mask text={c.masked} />}
        <span className="small faint">{CKIND[c.kind] ?? c.kind}, {c.count}x in {(AREA[c.where] ?? String(c.where)).toLowerCase()}</span>
        {err && <span className="small bad" role="alert"> {err}</span>}
      </div>
      <div className="row" style={{ gap: 8 }}>
        {val !== null
          ? <button className="btn sm" onClick={() => hide()} aria-label="Hide value now"><Ring ms={REVEAL_MS} /><EyeSlash size={14} aria-hidden />Hide</button>
          : <button className="btn sm" onClick={reveal} disabled={rb} aria-label="Reveal value"><Eye size={14} aria-hidden />{rb ? 'Revealing' : 'Reveal'}</button>}
        <button className="btn sm primary" disabled={busy} onClick={() => run(c.id, () => api.acceptCandidate(c.id), 'Added to the ledger.')}><Check size={14} aria-hidden />Accept</button>
        <button className="btn sm ghost" disabled={busy} onClick={() => run(c.id, () => api.dismissCandidate(c.id), 'Dismissed.')}><X size={14} aria-hidden />Dismiss</button>
      </div>
    </motion.li>
  )
}

function Row({ e, i, say }: { e: PiiEntry; i: number; say: (s: string) => void }) {
  const [val, setVal] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState('')
  const timer = useRef(0)
  const hide = useCallback((announce = true) => { clearTimeout(timer.current); setVal(null); if (announce) say(`${e.token} is masked again.`) }, [e.token, say])
  useEffect(() => {
    // Re-mask when the tab is hidden or the row goes away, so a revealed value never lingers.
    const v = () => { if (document.hidden) hide(false) }
    document.addEventListener('visibilitychange', v)
    return () => { document.removeEventListener('visibilitychange', v); clearTimeout(timer.current) }
  }, [hide])
  const reveal = async () => {
    setBusy(true); setErr('')
    try {
      const r = await api.revealPii(e.id)
      setVal(r.value); say(`Value for ${e.token} is shown. It will be hidden in 10 seconds.`)
      clearTimeout(timer.current); timer.current = window.setTimeout(() => hide(), REVEAL_MS)
    } catch (x) { setErr(x instanceof Error ? x.message : 'Could not reveal this value.') } finally { setBusy(false) }
  }
  return (
    <motion.li layout="position" className={`prow ${KIND[e.kind].cls}`} initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, x: 20 }} transition={{ duration: 0.45, ease: EASE, delay: Math.min(i, 8) * 0.04 }}>
      <div className="p-val">
        <AnimatePresence mode="wait" initial={false}>
          {val !== null ? (
            <motion.span key="v" className="p-real" initial={{ opacity: 0, filter: 'blur(4px)' }} animate={{ opacity: 1, filter: 'blur(0px)' }} exit={{ opacity: 0 }} transition={{ duration: 0.25 }}>{val || 'Empty value'}</motion.span>
          ) : <motion.span key="m" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.2 }}><Mask text={e.masked} delay={0.1 + Math.min(i, 8) * 0.04} /></motion.span>}
        </AnimatePresence>
        <span className="small faint">{SOURCE[e.source]}, {e.length} characters</span>
      </div>
      <div className="p-tok"><span className="small faint">AI sees</span><code className="tok-chip">{e.token}</code></div>
      <Where e={e} />
      <div className="p-act">
        {val !== null
          ? <button className="btn sm" onClick={() => hide()} aria-label={`Hide value for ${e.token} now`}><Ring ms={REVEAL_MS} /><EyeSlash size={15} aria-hidden />Hide</button>
          : <button className="btn sm" onClick={reveal} disabled={busy} aria-label={`Reveal value for ${e.token}`}><Eye size={15} aria-hidden />{busy ? 'Revealing' : 'Reveal'}</button>}
        {err && <span className="small bad" role="alert">{err}</span>}
      </div>
    </motion.li>
  )
}

export default function Pii() {
  const [d, setD] = useState<PiiData | null>(null)
  const [err, setErr] = useState('')
  const [live, setLive] = useState('')
  const [val, setVal] = useState('')
  const [kind, setKind] = useState<PiiKind>('custom')
  const [busy, setBusy] = useState('')
  const [on, setOn] = useState(false)
  const { toast } = useApp()
  const load = useCallback(() => api.getPii().then((x) => { setD(x); setErr('') }, (e) => setErr(e instanceof Error ? e.message : 'Could not load the ledger.')), [])
  useEffect(() => { void load(); const t = setTimeout(() => setOn(true), 700); return () => clearTimeout(t) }, [load])
  const say = useCallback((s: string) => setLive(s), [])
  const run = async (key: string, fn: () => Promise<unknown>, ok: string) => {
    setBusy(key)
    try { await fn(); await load(); toast({ kind: 'ok', text: ok }) } catch (e) { toast({ kind: 'err', text: e instanceof Error ? e.message : 'That did not work.' }) } finally { setBusy('') }
  }
  const add = (e: FormEvent) => { e.preventDefault(); const v = val.trim(); if (!v) return; void run('add', () => api.addAlways(v, kind), 'Added to the always redact list.').then(() => setVal('')) }

  const groups = (Object.keys(KIND) as PiiKind[]).map((k) => ({ k, rows: d?.entries.filter((x) => x.kind === k) ?? [] })).filter((g) => g.rows.length)
  const total = d?.entries.reduce((a, x) => a + x.total, 0) ?? 0
  return (
    <>
      <div className="page-head">
        <h1>What the app knows<br />about you.</h1>
        <p>Every personal value it detected, kept masked. Reveal one at a time, only when you need to check it.</p>
      </div>
      <div className="sr" role="status" aria-live="polite">{live}</div>
      {err && !d ? <div className="err-box" role="alert">{err} <button className="link-btn" onClick={load}>Try again</button></div> : !d ? (
        <div className="stack" style={{ gap: 16 }}><Skel h={92} r={20} /><Skel h={320} r={20} /></div>
      ) : (
        <div className="stack gap-xl">
          <Rise className="pii-sum" as="section" aria-label="Summary">
            <div className="ps-chips">
              {(Object.keys(KIND) as PiiKind[]).map((k) => {
                const n = d.entries.filter((x) => x.kind === k).length
                return <span key={k} className={`kchip ${KIND[k].cls} ${n ? '' : 'zero'}`}><b className="num">{n}</b>{n === 1 ? KIND[k].label : KIND[k].plural}</span>
              })}
            </div>
            <div className="ps-tot"><b className="num">{d.entries.length}</b><span>values tracked, {total} uses found</span></div>
            <p className="ps-note"><Lock size={16} aria-hidden />All values stay on this machine. The AI only ever sees the tokens.</p>
          </Rise>

          {d.entries.length === 0 ? (
            <Rise className="empty"><ShieldCheck size={34} aria-hidden /><h2>Nothing detected yet</h2><p>Add a resume on Intake and the names, contact details and employers it finds are listed here.</p></Rise>
          ) : (
            <Rise as="section" className="panel ledger" aria-label="Detected values" i={1}>
              <header><h2>Detected values</h2><p>Revealing shows one value for 10 seconds, then masks it again. Nothing is saved in the page.</p></header>
              {groups.map((g) => (
                <section key={g.k} className={`pgroup ${KIND[g.k].cls}`} aria-label={KIND[g.k].plural}>
                  <h3><span className="kdot" aria-hidden />{KIND[g.k].plural}<span className="faint num">{g.rows.length}</span></h3>
                  <ul className="clean"><AnimatePresence initial={false}>{g.rows.map((e, i) => <Row key={e.id} e={e} i={i} say={say} />)}</AnimatePresence></ul>
                </section>
              ))}
            </Rise>
          )}

          <div className="cols">
            <Rise as="section" className="panel" aria-label="Possible PII not in your list" inView>
              <header><h2>Possible PII not in your list</h2><p>Found in your resume text but not yet redacted. Accept to add it to Always redact.</p></header>
              {d.candidates.length === 0 ? <p className="small muted cand-empty"><Check size={16} aria-hidden />Nothing else looked personal.</p> : (
                <ul className="clean cands"><AnimatePresence initial={false}>
                  {d.candidates.map((c) => (
                    <CandRow key={c.id} c={c} busy={busy === c.id} run={run} say={say} />
                  ))}
                </AnimatePresence></ul>
              )}
            </Rise>

            <Rise as="section" className="panel" aria-label="Always redact" inView i={1}>
              <header><h2>Always redact</h2><p>Values you want tokenised in every job, such as a codename or a school.</p></header>
              <form className="inline-field" onSubmit={add}>
                <input value={val} onChange={(e) => setVal(e.target.value)} placeholder="Value to always hide" aria-label="Value to always redact" autoComplete="off" spellCheck={false} />
                <select value={kind} onChange={(e) => setKind(e.target.value as PiiKind)} aria-label="Kind of value">
                  {(Object.keys(KIND) as PiiKind[]).map((k) => <option key={k} value={k}>{KIND[k].label}</option>)}
                </select>
                <button className="btn primary" disabled={!val.trim() || busy === 'add'}><Plus size={15} aria-hidden />Add</button>
              </form>
              {d.always.length === 0 ? <p className="small muted">Nothing here yet.</p> : (
                <ul className="clean always"><AnimatePresence initial={false}>
                  {d.always.map((a) => (
                    <motion.li key={a.id} layout initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, x: 20 }} transition={SPRING}>
                      <span className={`kchip sm ${KIND[kindOf(a.kind)].cls}`}>{KIND[kindOf(a.kind)].label}</span><Mask text={a.masked} />
                      <button className="icon-btn sm" aria-label={`Remove ${a.masked} from always redact`} disabled={busy === a.id} onClick={() => run(a.id, () => api.removeAlways(a.id), 'Removed.')}><Trash size={15} /></button>
                    </motion.li>
                  ))}
                </AnimatePresence></ul>
              )}
            </Rise>
          </div>

          <Rise as="section" className="panel why" aria-label="Why is this redacted?" inView>
            <header><h2>Why is this redacted?</h2><p>The AI needs your achievements, not who you are. It writes with tokens, and the real values are put back here, on your machine.</p></header>
            <div className="why-grid">
              <div><span className="small faint">What you wrote</span><p className="paper-s">Jane Doe led navigation at Acme Robotics for Globex Logistics.</p></div>
              <ArrowRight size={18} aria-hidden className="why-ar" />
              <div><span className="small faint">What the AI reads</span>
                <p className="paper-s"><Redact real="Jane Doe" token="PERSON_1" on={on} /> led navigation at <Redact real="Acme Robotics" token="ORG_1" on={on} /> for <Redact real="Globex Logistics" token="CLIENT_1" on={on} />.</p></div>
            </div>
          </Rise>
        </div>
      )}
    </>
  )
}
