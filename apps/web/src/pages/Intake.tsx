import { ArrowRight, CloudArrowUp, FileDoc, FileText, Globe, Play, Warning, X } from '@phosphor-icons/react'
import { AnimatePresence, motion } from 'motion/react'
import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { parse } from 'yaml'
import { api, mode } from '../api'
import { useSharedText } from '../companion'
import { ErrorNotice } from '../components/ErrorNotice'
import { friendly, fromError, type Friendly } from '../lib/errors'
import { Seg, Toggle } from '../components/Controls'
import { DropZone } from '../components/DropZone'
import { EASE, Redact, Rise, SPRING } from '../components/Motion'
import { StepRail, ZoneLegend } from '../components/Steps'
import { DEMO_JD } from '../fixtures'
import { cost } from '../lib/util'
import { useApp } from '../store'
import type { DriveState, ImportResult, Keys, Provider, Resume } from '../types'

function normalize(x: unknown): Resume {
  const r = x as Partial<Resume> | null
  if (!r || typeof r !== 'object' || !r.profile || typeof r.profile.name !== 'string' || !Array.isArray(r.experience)) throw new Error('This does not look like a resume file. It needs a profile with a name and an experience list.')
  return { socials: [], summary: [], education: [], skills: [], projects: [], certifications: [], ...r } as Resume
}
const countBullets = (r: Resume) => r.experience.reduce((a, e) => a + (e.bullets?.length ?? 0), 0)
const LOW = 0.7

function FlowStep({ title, hint, done, children, i }: { title: string; hint?: string; done: boolean; children: ReactNode; i: number }) {
  return (
    <Rise i={i} as="section" className={`fstep ${done ? 'done' : ''}`}>
      <div className="fstep-rail" aria-hidden>
        <span className="fnode">
          <AnimatePresence initial={false}>{done && <motion.svg key="c" width="12" height="12" viewBox="0 0 12 12" initial={{ scale: 0.2, opacity: 0 }} animate={{ scale: 1, opacity: 1 }} transition={SPRING}><path d="M2.5 6.4 5 8.8l4.5-5.2" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" /></motion.svg>}</AnimatePresence>
        </span>
      </div>
      <div className="fstep-body">
        <div className="fstep-head"><h2>{title}</h2>{hint && <p>{hint}</p>}</div>
        {children}
      </div>
    </Rise>
  )
}

function LeavesPreview({ resume, tags, drive }: { resume: Resume | null; tags: string[]; drive: boolean }) {
  const [on, setOn] = useState(false)
  const [piiN, setPiiN] = useState<number | null>(null)
  useEffect(() => { api.getPii().then((x) => setPiiN(x.entries.length), () => {}) }, [])
  useEffect(() => { const t = setTimeout(() => setOn(true), 650); return () => clearTimeout(t) }, [])
  const p = resume?.profile ?? { name: 'Jane Doe', email: 'jane.doe@example.test', phone: '+1 555 0100', role: 'Senior Robotics Software Engineer', location: '' }
  const exp = resume?.experience ?? []
  const orgs = [...new Set(exp.map((e) => e.organization))]
  const clients = [...new Set(exp.map((e) => e.client).filter(Boolean) as string[])]
  const rows: [string, string][] = [[p.name, 'PERSON_1'], [p.email, 'EMAIL_1'], ...(p.phone ? [[p.phone, 'PHONE_1'] as [string, string]] : []),
    ...(resume ? orgs.map((o, i) => [o, `ORG_${i + 1}`] as [string, string]) : [['Acme Robotics', 'ORG_1'] as [string, string]]),
    ...clients.map((o, i) => [o, `CLIENT_${i + 1}`] as [string, string]), ...tags.map((t, i) => [t, `CUSTOM_${i + 1}`] as [string, string])]
  const shown = rows.slice(0, 9)
  const org = orgs[0] ?? 'Acme Robotics', client = clients[0]
  return (
    <aside className="leaves" aria-label="What leaves your machine">
      <div className="leaves-head">
        <h2>What leaves this machine</h2>
        <p>Only the tokens on the right. Your real values and the map between them never leave.</p>
      </div>
      <div className="paper" aria-label="Example line before and after redaction">
        <p>
          <Redact real={p.name} token="PERSON_1" on={on} /> led navigation at <Redact real={org} token="ORG_1" on={on} />
          {client ? <> for <Redact real={client} token="CLIENT_1" on={on} /></> : null}, cutting replan latency from 180 ms to 60 ms.
        </p>
      </div>
      <StepRail drive={drive} />
      <ZoneLegend drive={drive} />
      <div className="map" role="table" aria-label="Values and the tokens that replace them">
        <div className="map-h" role="row"><span>Stays here</span><span /><span>Leaves as</span></div>
        <AnimatePresence initial={false}>
          {shown.map(([real, tok]) => (
            <motion.div key={tok} layout role="row" className="map-r" initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 8 }} transition={{ duration: 0.35, ease: EASE }}>
              <span className="real" role="cell">{real}</span>
              <span className="wire" aria-hidden />
              <code className="tok" role="cell">{tok}</code>
            </motion.div>
          ))}
        </AnimatePresence>
        {rows.length > shown.length && <p className="tiny faint">and {rows.length - shown.length} more</p>}
        {piiN !== null && <Link className="pii-link" to="/pii">{piiN} values tracked. Open the PII ledger<ArrowRight size={13} aria-hidden /></Link>}
        <AnimatePresence initial={false}>
          {drive && (
            <motion.div key="drive" layout role="row" className="map-r drive" initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} transition={{ duration: 0.35, ease: EASE }}>
              <span className="real" role="cell">Finished PDF and DOCX</span><span className="wire" aria-hidden /><span className="tok" role="cell">Your Drive folder</span>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </aside>
  )
}

function ReviewPanel({ file, r, onUse, onDiscard }: { file: string; r: ImportResult; onUse: () => void; onDiscard: () => void }) {
  const low = r.confidence < LOW
  const [ack, setAck] = useState(false)
  const pct = Math.round(r.confidence * 100)
  return (
    <motion.div className={`review ${low ? 'low' : ''}`} initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -8 }} transition={{ duration: 0.45, ease: EASE }} role="group" aria-label="Review imported resume">
      <div className="review-top">
        <div>
          <h3>Check what we read from {file}</h3>
          <p className="small muted">{r.resume.profile.name || 'No name found'}, {r.resume.experience.length} roles, {countBullets(r.resume)} bullets</p>
        </div>
        <div className="conf" aria-label={`Parsing confidence ${pct} percent`}>
          <div className="conf-bar"><motion.i initial={{ scaleX: 0 }} animate={{ scaleX: r.confidence }} transition={{ duration: 0.9, ease: EASE, delay: 0.15 }} /></div>
          <span className="small num">{low ? <b>Low confidence</b> : 'Good confidence'}, {pct}%</span>
        </div>
      </div>
      {low && <p className="low-note"><Warning size={16} aria-hidden />This file was hard to read. Some details may be missing or in the wrong place. Review the list, then fix anything in the source file if it matters.</p>}
      {r.warnings.length > 0 ? (
        <ul className="warns clean">
          {r.warnings.map((w, i) => <motion.li key={w} initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0 }} transition={{ delay: 0.12 + i * 0.07, duration: 0.4, ease: EASE }}><Warning size={14} aria-hidden />{w}</motion.li>)}
        </ul>
      ) : <p className="small muted">Nothing looked off.</p>}
      <details className="peek"><summary>Preview what was read</summary>
        <ul className="clean">{r.resume.experience.map((e) => <li key={e.id}><b>{e.role}</b>, {e.organization} <span className="faint">({e.bullets.length} bullets{e.dateLabel ? `, ${e.dateLabel}` : ', no dates'})</span></li>)}</ul>
      </details>
      {low && <label className="check"><input type="checkbox" checked={ack} onChange={(e) => setAck(e.target.checked)} />I reviewed the warnings and want to use this resume</label>}
      <div className="row">
        <button className="btn primary" disabled={low && !ack} onClick={onUse}>Use this resume<span className="ic"><ArrowRight size={14} /></span></button>
        <button className="btn ghost" onClick={onDiscard}>Discard</button>
      </div>
    </motion.div>
  )
}

export default function Intake() {
  const nav = useNavigate()
  const { refresh, toast, budget, demo, offerOs } = useApp()
  const [resume, setResume] = useState<Resume | null>(null)
  const [fileName, setFileName] = useState('')
  const [importing, setImporting] = useState('')
  const [pending, setPending] = useState<{ file: string; r: ImportResult } | null>(null)
  const [err, setErrS] = useState<Friendly | null>(null)
  const setErr = (x: string | unknown, code?: string) => setErrS(!x ? null : typeof x === 'string' ? friendly(code, { message: x, title: 'Could not continue' }) : fromError(x))
  const [jd, setJd] = useState('')
  const [url, setUrl] = useState('')
  const [fetching, setFetching] = useState(false)
  const [urlErr, setUrlErr] = useState<Friendly | null>(null)
  const [company, setCompany] = useState('')
  const [role, setRole] = useState('')
  const [flash, setFlash] = useState(0)
  const [provider, setProvider] = useState<Provider>('mock')
  const [tags, setTags] = useState<string[]>([])
  const [tagIn, setTagIn] = useState('')
  const [maxTok, setMaxTok] = useState('')
  const [pages, setPages] = useState<'auto' | '1' | '2'>('auto')
  const [prior, setPrior] = useState<number | null>(null)
  useEffect(() => { api.getPriorPages().then(setPrior, () => {}) }, [])
  const [keys, setKeys] = useState<Keys | null>(null)
  const [driveSt, setDriveSt] = useState<DriveState | null>(null)
  const [toDrive, setToDrive] = useState(false)
  const [overBudget, setOverBudget] = useState(false)
  const [busy, setBusy] = useState(false)
  const jdRef = useRef<HTMLTextAreaElement>(null)

  useEffect(() => {
    api.getResume().then(setResume, () => {})
    api.getKeys().then(setKeys, () => {})
    api.getDrive().then(setDriveSt, () => {})
    api.getStats().then((s) => setOverBudget(budget > 0 && cost(s.tokens_in, s.tokens_out) >= budget), () => {})
  }, [budget])
  const co = !!mode.companion
  const driveReady = !!driveSt?.connected
  useEffect(() => { if (!driveReady) setToDrive(false) }, [driveReady])

  const accept = async (f: File) => {
    setErr(null)
    if (/\.(pdf|docx)$/i.test(f.name)) {
      setImporting(f.name)
      try { setPending({ file: f.name, r: await api.importResume(f) }) }
      catch (e) { setErr(e) }
      finally { setImporting('') }
      return
    }
    if (!/\.(json|ya?ml)$/i.test(f.name)) return setErr('Use a .json, .yaml, .pdf or .docx file.')
    try {
      const r = normalize(parse(await f.text()))
      await api.putResume(r)
      setResume(r); setFileName(f.name); setPending(null)
    } catch (e) { setErr(e) }
  }
  const useImport = async () => {
    if (!pending) return
    try {
      const r = normalize(pending.r.resume)
      await api.putResume(r)
      setResume(r); setFileName(pending.file); setPending(null)
      toast({ kind: 'ok', text: 'Resume saved.' })
    } catch (e) { setErr(e) }
  }
  const fetchJd = async () => {
    if (!url.trim()) return
    setFetching(true); setUrlErr(null)
    try {
      const r = await api.fetchJd(url.trim())
      setJd(r.text); if (r.company) setCompany(r.company); if (r.title) setRole(r.title)
      setFlash((n) => n + 1)
      toast({ kind: 'ok', text: `Fetched the posting from ${r.source}.` })
    } catch (e) {
      const f = fromError(e)
      setUrlErr(f.code === 'unknown' && (e as { status?: number }).status === 422 ? friendly('fetch.login_wall') : f)
      if (f.code === 'fetch.login_wall' || (e as { status?: number }).status === 422) jdRef.current?.focus()
    } finally { setFetching(false) }
  }
  // Text shared from the Android share sheet: a link is fetched straight away, anything else becomes the description.
  const fetchRef = useRef(fetchJd); fetchRef.current = fetchJd
  const [auto, setAuto] = useState('')
  useSharedText((t) => {
    const u = t.match(/https?:\/\/\S+/)?.[0]
    if (u && t.replace(u, '').trim().length < 40) { setUrl(u); setAuto(u) } else setJd(t)
  })
  useEffect(() => { if (auto && url === auto) { setAuto(''); void fetchRef.current() } }, [auto, url])
  const addTag = () => { const v = tagIn.trim().replace(/,$/, ''); if (v && !tags.includes(v)) setTags([...tags, v]); setTagIn('') }
  const tagKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' || e.key === ',') { e.preventDefault(); addTag() }
    else if (e.key === 'Backspace' && !tagIn && tags.length) setTags(tags.slice(0, -1))
  }
  const paid = provider !== 'mock'
  const noKey = !co && paid && keys && !keys[provider].configured
  const blocked = !co && paid && overBudget
  const can = (co || !!resume) && jd.trim().length > 20 && !noKey && !blocked && !busy
  const why = !co && !resume ? 'Add your resume first.' : jd.trim().length <= 20 ? 'Paste the job description.' : noKey ? 'Add a key for this provider in Settings.' : blocked ? 'Budget cap reached.' : ''

  const start = async () => {
    setBusy(true)
    try {
      const mt = parseInt(maxTok, 10)
      const { id } = await api.createJob({ jd_text: jd, provider, extra_redact: tags, ...(mt > 0 ? { max_tokens: mt } : {}), ...(company.trim() ? { company: company.trim() } : {}), ...(role.trim() ? { role: role.trim() } : {}), ...(pages !== 'auto' ? { pages: Number(pages) as 1 | 2 } : {}), ...(toDrive ? { upload_drive: true } : {}) })
      await refresh()
      toast({ kind: 'info', text: 'Job started. Follow it here or from Activity.' })
      offerOs()
      nav(`/jobs/${id}`)
    } catch (e) { setErr(e); setBusy(false) }
  }
  const flashCls = useMemo(() => (flash ? `flash f${flash % 2}` : ''), [flash])

  return (
    <>
      <div className="page-head">
        <h1>Tailor one resume<br />to one job.</h1>
        <p>Your details are swapped for tokens before anything is sent. You see exactly what leaves, and you review every change.</p>
      </div>
      <div className={`intake ${co ? 'co' : ''}`}>
        <div className="flow">
          {co ? <FlowStep i={1} title="Your resume" done><p className="hint" style={{ margin: 0 }}>Your resume stays on the desktop. Jobs started here are tailored from it there.</p></FlowStep> : <FlowStep i={1} title="Your resume" done={!!resume && !pending}>
            <AnimatePresence mode="wait" initial={false}>
              {pending ? <ReviewPanel key="rev" file={pending.file} r={pending.r} onUse={useImport} onDiscard={() => setPending(null)} /> : (
                <motion.div key="dz" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.2 }}>
                  <DropZone onFile={accept} accept=".json,.yaml,.yml,.pdf,.docx,application/json" label="Drop a resume file or press Enter to choose one" filled={!!resume}>
                    {importing ? (
                      <div className="dz-busy" role="status"><span className="scan" aria-hidden /><b>Reading {importing}</b><span className="small muted">Everything is parsed on this machine.</span></div>
                    ) : resume ? (
                      <><span className="big"><FileText size={22} aria-hidden />{resume.profile.name}</span>
                        <span className="small muted">{fileName || 'Resume on file'}: {resume.experience.length} roles, {countBullets(resume)} bullets. Drop another file to replace it.</span></>
                    ) : (
                      <><span className="big"><FileDoc size={22} aria-hidden />Drop your resume here</span>
                        <span className="small muted">PDF or DOCX to import, or resume.json and YAML. Or click to choose.</span></>
                    )}
                  </DropZone>
                </motion.div>
              )}
            </AnimatePresence>
            {err && <ErrorNotice f={err} compact />}
          </FlowStep>}

          <FlowStep i={2} title="The job" hint="Paste a link or the text itself." done={jd.trim().length > 20}>
            <div className="field">
              <label htmlFor="url">Job URL</label>
              <div className="inline-field">
                <div className="input-ic"><Globe size={16} aria-hidden /><input id="url" type="url" inputMode="url" value={url} onChange={(e) => setUrl(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); fetchJd() } }} placeholder="https://jobs.example.test/robotics-engineer" aria-describedby="urlh" /></div>
                <button type="button" className="btn" disabled={fetching || !url.trim()} onClick={fetchJd}>{fetching ? 'Fetching' : 'Fetch'}</button>
              </div>
              {urlErr && <div id="urlh"><ErrorNotice f={urlErr} compact /></div>}
            </div>
            <div className="two">
              <div className="field"><label htmlFor="company">Company</label><input id="company" key={`c${flash}`} className={flashCls} value={company} onChange={(e) => setCompany(e.target.value)} placeholder="Globex Logistics" autoComplete="off" /></div>
              <div className="field"><label htmlFor="role">Role</label><input id="role" key={`r${flash}`} className={flashCls} value={role} onChange={(e) => setRole(e.target.value)} placeholder="Senior Robotics Software Engineer" autoComplete="off" /></div>
            </div>
            <div className="field">
              <div className="row between"><label htmlFor="jd">Job description</label>
                <button type="button" className="link-btn" onClick={() => { setJd(DEMO_JD); setCompany((c) => c || 'Globex Logistics'); setRole((r) => r || 'Senior Robotics Software Engineer') }}>Use a sample</button></div>
              <div className={`ta ${fetching ? 'loading' : ''}`}>
                <textarea ref={jdRef} id="jd" key={`j${flash}`} className={flashCls} rows={10} value={jd} onChange={(e) => setJd(e.target.value)} placeholder="Paste the full posting, including requirements." />
                {fetching && <span className="ta-skel" aria-hidden />}
              </div>
              {demo && <p className="hint">Demo: put one of these words in the description to trigger an error: ratelimit, auth, budget, timeout, badreply, leak, fitfail, texfail, quota, nodrive. A URL containing login or blocked tests the fetch errors.</p>}
            </div>
          </FlowStep>

          <FlowStep i={3} title="Send" done={false}>
            <fieldset className="field">
              <legend>AI provider</legend>
              <Seg label="AI provider" value={provider} onChange={setProvider} options={[{ value: 'mock', label: 'Mock (offline)' }, { value: 'gemini', label: 'Gemini' }, { value: 'anthropic', label: 'Claude' }]} />
              {noKey && <p className="small bad">No key saved for this provider. <Link to="/settings">Add one in Settings</Link>.</p>}
              {blocked && <p className="small bad">Estimated spend has reached your budget cap. <Link to="/settings">Raise the cap</Link> or use the mock provider.</p>}
              {provider === 'mock' && <p className="hint">The mock provider answers locally with canned text. Nothing is sent anywhere.</p>}
            </fieldset>
            <fieldset className="field">
              <legend>Pages</legend>
              <Seg label="Pages" value={pages} onChange={setPages} options={[{ value: 'auto', label: 'Auto' }, { value: '1', label: '1 page' }, { value: '2', label: '2 pages' }]} />
              <p className="hint">Auto decides from your experience and the JD{prior ? ` (your prior resume: ${prior} ${prior === 1 ? 'page' : 'pages'})` : ''}.</p>
            </fieldset>
            <div className="field">
              <label htmlFor="redact">Also redact</label>
              <div className="tags" onClick={() => document.getElementById('redact')?.focus()}>
                <AnimatePresence initial={false}>
                  {tags.map((t) => <motion.span key={t} layout initial={{ scale: 0.6, opacity: 0 }} animate={{ scale: 1, opacity: 1 }} exit={{ scale: 0.6, opacity: 0 }} transition={SPRING} className="tag">{t}<button type="button" aria-label={`Remove ${t}`} onClick={() => setTags(tags.filter((x) => x !== t))}><X size={12} /></button></motion.span>)}
                </AnimatePresence>
                <input id="redact" type="text" value={tagIn} onChange={(e) => setTagIn(e.target.value)} onKeyDown={tagKey} onBlur={addTag} placeholder={tags.length ? '' : 'A project codename, a university'} aria-describedby="rh" />
              </div>
              <p id="rh" className="hint">Anything else you want replaced with a token. Press Enter after each one.</p>
            </div>
            {!co && <div className={`opt ${driveReady ? '' : 'off'}`}>
              <div className="opt-t"><CloudArrowUp size={20} aria-hidden /><div><label htmlFor="drive">Save to Google Drive</label>
                <p className="hint">{driveReady ? `Uploads the finished PDF and DOCX to ${driveSt?.folder_name ? `"${driveSt.folder_name}"` : 'your Drive folder'} in your own account.` : <>Google Drive is not connected. <Link to="/settings">Connect it in Settings</Link> to turn this on.</>}</p></div></div>
              <Toggle id="drive" on={toDrive} onChange={setToDrive} label="Save to Google Drive" disabled={!driveReady} />
            </div>}
            <details className="adv"><summary>Advanced</summary>
              <div className="field" style={{ maxWidth: 260 }}><label htmlFor="mt">Max output tokens</label>
                <input id="mt" type="number" min={256} step={256} value={maxTok} onChange={(e) => setMaxTok(e.target.value)} placeholder="Provider default" /></div>
            </details>
            <div className="row go">
              <button className="btn primary lg" disabled={!can} onClick={start}><Play size={16} weight="fill" aria-hidden />{busy ? 'Starting' : 'Start tailoring'}<span className="ic"><ArrowRight size={14} /></span></button>
              {!can && !busy && <span className="small muted">{why}</span>}
            </div>
          </FlowStep>
        </div>
        {!co && <LeavesPreview resume={resume} tags={tags} drive={toDrive} />}
      </div>
    </>
  )
}
