import { ArrowUpRight, CircleNotch, Copy, CloudArrowUp, Key, Monitor, Moon, Sun } from '@phosphor-icons/react'
import { AnimatePresence, motion } from 'motion/react'
import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react'
import { api, mode } from '../api'
import { ErrorNotice } from '../components/ErrorNotice'
import { Seg } from '../components/Controls'
import { Rise, Skel } from '../components/Motion'
import { useApp } from '../store'
import { human } from '../components/Viz'
import { MobileCard } from '../components/MobileCard'
import type { DriveState, Heuristics, KeyProvider, Keys } from '../types'

const NAMES: Record<KeyProvider, string> = { gemini: 'Google Gemini', anthropic: 'Anthropic Claude' }

function KeyRow({ p, state, done }: { p: KeyProvider; state: Keys[KeyProvider]; done: () => void }) {
  const [v, setV] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<unknown>(null)
  const { toast } = useApp()
  const stored = state.source === 'stored', env = state.source === 'env'
  const save = async (e: FormEvent) => {
    e.preventDefault(); setBusy(true); setErr(null)
    try { await api.setKey(p, v); setV(''); toast({ kind: 'ok', text: `${NAMES[p]} key saved.` }); done() }
    catch (x) { setErr(x) } finally { setBusy(false) }
  }
  const remove = async () => {
    setBusy(true); setErr(null)
    try { await api.deleteKey(p); toast({ kind: 'ok', text: `Stored ${NAMES[p]} key removed.` }); done() }
    catch (x) { setErr(x) } finally { setBusy(false) }
  }
  return (
    <form className="keyrow" onSubmit={save}>
      <div className="row between"><label className="label" htmlFor={`k-${p}`}>{NAMES[p]}</label>
        <AnimatePresence mode="wait" initial={false}>
          <motion.span key={state.source ?? 'none'} initial={{ opacity: 0, y: 4 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -4 }} transition={{ duration: 0.2 }}
            className={`badge ${stored ? 'ok' : env ? 'z-local' : ''}`}>{stored ? 'Stored key (overrides env)' : env ? 'Using environment variable' : state.configured ? 'Configured' : 'Not configured'}</motion.span>
        </AnimatePresence></div>
      <div className="inline-field">
        <input id={`k-${p}`} type="password" autoComplete="new-password" spellCheck={false} value={v} onChange={(e) => setV(e.target.value)} placeholder={stored ? 'Enter a new key to replace it' : env ? 'Optional: paste a key to override the environment' : 'Paste your API key'} aria-describedby={`h-${p}`} />
        <button className="btn primary" disabled={busy || !v.trim()}>{env && !stored ? 'Override' : 'Save'}</button>
        {stored && <button type="button" className="btn" disabled={busy} onClick={remove}>Remove stored key</button>}
      </div>
      <p id={`h-${p}`} className="hint">{env && !stored ? 'The server found this key in its environment. A stored key takes priority until you remove it. ' : ''}Write-only. Once saved, a key is never shown again.</p>
      {err !== null && <ErrorNotice error={err} />}
    </form>
  )
}

function DriveCard() {
  const [d, setD] = useState<DriveState | null>(null)
  const [err, setErrS] = useState<unknown>(null)
  const setErr = (e: unknown) => setErrS(e === '' ? null : e)
  const [folder, setFolder] = useState('')
  const [cid, setCid] = useState('')
  const [sec, setSec] = useState('')
  const [busy, setBusy] = useState(false)
  const [waiting, setWaiting] = useState(false)
  const { toast } = useApp()
  const waitUntil = useRef(0)
  const load = useCallback(() => api.getDrive().then((x) => { setD(x); setFolder((f) => f || x.folder_name); return x }, (e) => { setErr(e); return null }), [])
  useEffect(() => { void load() }, [load])
  useEffect(() => {
    if (!waiting) return
    const t = setInterval(async () => {
      const x = await load()
      if (x?.connected) { setWaiting(false); toast({ kind: 'ok', text: `Google Drive connected${x.account_email ? ` as ${x.account_email}` : ''}.` }) }
      else if (Date.now() > waitUntil.current) { setWaiting(false); setErr(new Error('Google did not confirm in time. Try Connect again.')) }
    }, 2000)
    return () => clearInterval(t)
  }, [waiting, load, toast])
  const run = async (fn: () => Promise<unknown>, ok?: string) => {
    setBusy(true); setErr('')
    try { await fn(); if (ok) toast({ kind: 'ok', text: ok }); await load() } catch (e) { setErr(e) } finally { setBusy(false) }
  }
  const connect = () => run(async () => {
    const { auth_url } = await api.driveConnect()
    if (!mode.demo) window.open(auth_url, '_blank', 'noopener')
    waitUntil.current = Date.now() + 180000; setWaiting(true)
  })
  const copy = (t: string) => navigator.clipboard?.writeText(t).then(() => toast({ kind: 'info', text: 'Copied.' }), () => {})
  if (!d) return <section className="panel"><Skel h={22} w="40%" /><Skel h={120} r={12} /></section>
  return (
    <section className="panel drive-card" aria-label="Google Drive">
      <header>
        <h2><CloudArrowUp size={18} aria-hidden />Google Drive</h2>
        <p>Optionally save each finished resume to a folder in your own Google account. The AI never sees these files.</p>
      </header>
      {err !== null && <ErrorNotice error={err} />}
      {!d.configured ? (
        <form className="stack" style={{ gap: 14 }} onSubmit={(e) => { e.preventDefault(); void run(() => api.putDriveOauth(cid.trim(), sec.trim()), 'OAuth client saved.').then(() => { setCid(''); setSec('') }) }}>
          <ol className="steps-list">
            <li>Open the <a href="https://console.cloud.google.com/apis/library/drive.googleapis.com" target="_blank" rel="noreferrer">Google Cloud Console<ArrowUpRight size={12} aria-hidden /></a>, create or pick a project, and enable the Google Drive API.</li>
            <li>Under APIs and services, open OAuth consent screen. Choose External and add your own Google account as a test user.</li>
            <li>Under Credentials, create an OAuth client ID with application type Desktop app.</li>
            <li>Add this redirect URI to the client:
              <span className="copyline"><code>{d.redirect_uri}</code><button type="button" className="icon-btn" aria-label="Copy redirect URI" onClick={() => copy(d.redirect_uri)}><Copy size={15} /></button></span></li>
            <li>Paste the client ID and secret below. They are stored locally and never shown again.</li>
          </ol>
          <div className="two">
            <div className="field"><label htmlFor="cid">Client ID</label><input id="cid" value={cid} onChange={(e) => setCid(e.target.value)} autoComplete="off" spellCheck={false} placeholder="123456-abc.apps.googleusercontent.com" /></div>
            <div className="field"><label htmlFor="csec">Client secret</label><input id="csec" type="password" value={sec} onChange={(e) => setSec(e.target.value)} autoComplete="new-password" spellCheck={false} placeholder="Paste the secret" /></div>
          </div>
          <div className="row"><button className="btn primary" disabled={busy || !cid.trim() || !sec.trim()}>Save OAuth client</button></div>
        </form>
      ) : (
        <div className="stack" style={{ gap: 18 }}>
          <div className={`conn-state ${d.connected ? 'on' : ''}`}>
            <span className="cs-dot" aria-hidden />
            <div className="grow"><b>{d.connected ? 'Connected' : waiting ? 'Waiting for Google' : 'Not connected'}</b>
              <span className="small muted">{d.connected ? (d.account_email ?? 'Your Google account') : waiting ? 'Finish signing in in the tab that just opened. This page updates by itself.' : 'Connect to turn on "Save to Google Drive" for new jobs.'}</span></div>
            {d.connected
              ? <button className="btn" disabled={busy} onClick={() => run(() => api.driveDisconnect(), 'Google Drive disconnected.')}>Disconnect</button>
              : waiting ? <button className="btn" onClick={() => setWaiting(false)}><CircleNotch className="spin" size={15} aria-hidden />Cancel</button>
                : <button className="btn primary" disabled={busy} onClick={connect}>Connect Google Drive</button>}
          </div>
          <form className="field" onSubmit={(e) => { e.preventDefault(); void run(() => api.putDrive(folder.trim()), 'Folder name saved.') }}>
            <label htmlFor="folder">Drive folder</label>
            <div className="inline-field"><input id="folder" value={folder} onChange={(e) => setFolder(e.target.value)} aria-describedby="fh" /><button className="btn" disabled={busy || !folder.trim() || folder.trim() === d.folder_name}>Save</button></div>
            <p id="fh" className="hint">Created in your Drive the first time a job uploads. Only files this app creates are visible to it.</p>
          </form>
        </div>
      )}
    </section>
  )
}

const HGROUPS: [string, string, (k: string) => boolean][] = [
  ['Fit floors', 'What trimming never goes below when squeezing onto the page.', (k) => k.startsWith('fit_')],
  ['Content sizing', 'How much of each section to show for a one or two page target.', (k) => /_(one|two)$|^compact_volume$/.test(k)],
  ['Page-length score', 'Weights that decide between one and two pages. A score at or above the threshold means two.', () => true],
]

function HeuristicsPanel() {
  const [h, setH] = useState<Heuristics | null>(null)
  const [draft, setDraft] = useState<Record<string, string>>({})
  const [err, setErr] = useState('')
  const [busy, setBusy] = useState(false)
  const { toast } = useApp()
  const apply = (x: Heuristics) => { setH(x); setDraft({}); setErr('') }
  useEffect(() => { api.getHeuristics().then(apply, (e) => setErr(e.message)) }, [])
  const run = async (fn: () => Promise<Heuristics>, ok: string) => {
    setBusy(true)
    try { apply(await fn()); toast({ kind: 'ok', text: ok }) } catch (e) { setErr(e instanceof Error ? e.message : 'That did not work.') } finally { setBusy(false) }
  }
  if (!h) return <section className="panel"><header><h2>Page-fit heuristics</h2></header>{err ? <ErrorNotice message={err} /> : <Skel h={160} r={12} />}</section>
  const keys = Object.keys(h.defaults).sort()
  const patch = Object.fromEntries(Object.entries(draft).filter(([k, v]) => v.trim() !== '' && Number(v) !== h.effective[k]).map(([k, v]) => [k, Number(v)]))
  const dirty = Object.keys(patch).length
  const anyChanged = keys.some((k) => h.effective[k] !== h.defaults[k])
  const seen = new Set<string>()
  return (
    <section className="panel" aria-label="Page-fit heuristics">
      <header><h2>Page-fit heuristics</h2><p>The numbers behind how long your resume is and what it includes. Defaults are sensible. Change them only if results feel off.</p></header>
      {HGROUPS.map(([title, blurb, test]) => {
        const ks = keys.filter((k) => !seen.has(k) && test(k))
        ks.forEach((k) => seen.add(k))
        return [title, blurb, ks] as const
      }).reverse().map(([title, blurb, ks]) => {
        return (
          <div key={title}><h3 className="subh" style={{ fontSize: 14, color: 'var(--ink)' }}>{title}</h3><p className="hint" style={{ marginTop: 0 }}>{blurb}</p>
            {ks.map((k) => {
              const changed = h.effective[k] !== h.defaults[k]
              return (
                <div key={k} className={`hfield ${draft[k] !== undefined && Number(draft[k]) !== h.effective[k] ? 'chg' : ''}`}>
                  <label htmlFor={`h-${k}`}>{human(k)}{changed && <span className="chg-m">changed</span>}<span className="d">default {h.defaults[k]}</span></label>
                  <input id={`h-${k}`} type="number" step="any" value={draft[k] ?? String(h.effective[k])} onChange={(e) => setDraft({ ...draft, [k]: e.target.value })} />
                  {changed ? <button className="link-btn" disabled={busy} onClick={() => run(() => api.putHeuristics({ [k]: h.defaults[k] }), `${human(k)} reset.`)}>Reset</button> : <span />}
                </div>
              )
            })}
          </div>
        )
      })}
      {err && <p className="field-err" role="alert">{err}</p>}
      <div className="row"><button className="btn primary" disabled={busy || !dirty} onClick={() => run(() => api.putHeuristics(patch), 'Heuristics saved.')}>Save changes</button>
        <button className="btn" disabled={busy || !anyChanged} onClick={() => run(() => api.resetHeuristics(), 'Back to the defaults.')}>Reset all to defaults</button></div>
    </section>
  )
}

function PriorPages() {
  const [v, setV] = useState<string | null>(null)
  const { toast } = useApp()
  useEffect(() => { api.getPriorPages().then((n) => setV(n == null ? '' : String(n)), () => setV('')) }, [])
  const save = async (x: string) => {
    setV(x)
    try { await api.setPriorPages(x ? Number(x) : null); toast({ kind: 'ok', text: 'Saved.' }) } catch (e) { toast({ kind: 'err', text: e instanceof Error ? e.message : 'Could not save.' }) }
  }
  return (
    <section className="panel" aria-label="Prior resume length">
      <header><h2>Prior resume length</h2><p>How many pages your usual resume has. It is the strongest hint for the Auto page setting. Importing a PDF fills this in.</p></header>
      <div className="field"><label htmlFor="prior">Pages</label>
        <select id="prior" value={v ?? ''} disabled={v === null} onChange={(e) => save(e.target.value)} style={{ maxWidth: 200 }}><option value="">Not set</option>{[1, 2, 3, 4, 5, 6, 7, 8, 9].map((n) => <option key={n} value={n}>{n}</option>)}</select></div>
    </section>
  )
}

export default function Settings() {
  const [keys, setKeys] = useState<Keys | null>(null)
  const { budget, setBudget, theme, setTheme } = useApp()
  const load = () => api.getKeys().then(setKeys, () => setKeys({ gemini: { configured: false, source: null }, anthropic: { configured: false, source: null } }))
  useEffect(() => { void load() }, [])
  return (
    <>
      <div className="page-head"><h1>Settings</h1><p>Keys let you use a real AI provider. The mock provider needs none.</p></div>
      <div className="settings">
        <Rise as="section" className="panel" aria-label="API keys">
          <header><h2><Key size={18} aria-hidden />API keys</h2><p>Stored by the local server and used only for the redacted AI step.</p></header>
          {keys ? <div className="stack" style={{ gap: 26 }}>{(['anthropic', 'gemini'] as const).map((p) => <KeyRow key={p} p={p} state={keys[p]} done={load} />)}</div> : <Skel h={150} r={12} />}
        </Rise>
        <Rise i={1}><DriveCard /></Rise>
        <Rise as="section" className="panel" i={2} aria-label="Budget">
          <header><h2>Budget cap</h2><p>New jobs with a paid provider are blocked once estimated spend reaches this amount.</p></header>
          <div className="field"><label htmlFor="budget">Monthly cap in US dollars</label>
            <input id="budget" type="number" inputMode="decimal" min={0} step={0.5} value={budget} onChange={(e) => setBudget(Math.max(0, Number(e.target.value)))} aria-describedby="bh" style={{ maxWidth: 200 }} />
            <p id="bh" className="hint">Saved in this browser. Spend is estimated from token counts, not billed amounts.</p></div>
        </Rise>
        <Rise i={3}><PriorPages /></Rise>
        <Rise i={4}><HeuristicsPanel /></Rise>
        <Rise i={5}><MobileCard /></Rise>
        <Rise as="section" className="panel" i={6} aria-label="Appearance">
          <header><h2>Appearance</h2><p>Follows your system by default. Press <kbd>t</kbd> anywhere to switch.</p></header>
          <Seg label="Theme" value={theme as 'system' | 'light' | 'dark'} onChange={setTheme} options={[{ value: 'system', label: <><Monitor size={15} aria-hidden /> System</> }, { value: 'light', label: <><Sun size={15} aria-hidden /> Light</> }, { value: 'dark', label: <><Moon size={15} aria-hidden /> Dark</> }]} />
        </Rise>
      </div>
    </>
  )
}
