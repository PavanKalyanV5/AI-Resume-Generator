import { DeviceMobile, LinkBreak, QrCode } from '@phosphor-icons/react'
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { api, mode, type CompanionStatus } from './api'
import { ErrorNotice } from './components/ErrorNotice'

/** Nav/palette entries the desktop denies to a paired phone (they would answer 403). */
export const CO_HIDDEN = new Set(['lab', 'profile', 'pii', 'analytics', 'settings'])

/** Normal builds render children; the phone app shows the Pair screen until paired (then mounts the app). */
export function Gate({ children }: { children: ReactNode }) {
  const [st, setSt] = useState<CompanionStatus | null>(mode.companion)
  return st && !st.paired ? <PairScreen onPaired={setSt} /> : <>{children}</>
}

function PairScreen({ onPaired }: { onPaired: (s: CompanionStatus) => void }) {
  const [text, setText] = useState('')
  const [f, setF] = useState({ host: '', port: '8788', fp: '', code: '' })
  const [name, setName] = useState(() => localStorage.getItem('veil.devname') ?? 'My phone')
  const [err, setErr] = useState<unknown>(null)
  const [busy, setBusy] = useState(false)
  const [scan, setScan] = useState(false)
  const vid = useRef<HTMLVideoElement>(null)
  const set = (k: keyof typeof f) => (e: { target: { value: string } }) => setF({ ...f, [k]: e.target.value })
  const submit = useCallback(async (payload?: string) => {
    setBusy(true); setErr(null)
    try {
      localStorage.setItem('veil.devname', name)
      const p = (payload ?? text).trim()
      onPaired(await api.companionPair(p ? { payload: p, device_name: name } : { ...f, device_name: name }))
    } catch (e) { setErr(e); setBusy(false) }
  }, [f, name, text, onPaired])

  const canScan = 'BarcodeDetector' in window && !!navigator.mediaDevices?.getUserMedia
  useEffect(() => {
    if (!scan) return
    let stop = false, stream: MediaStream | undefined, t = 0
    ;(async () => {
      try {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const det = new (window as any).BarcodeDetector({ formats: ['qr_code'] })
        stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' } })
        if (stop || !vid.current) return
        vid.current.srcObject = stream; await vid.current.play()
        t = window.setInterval(async () => {
          const hit = (await det.detect(vid.current!).catch(() => []))[0]
          if (hit?.rawValue) { clearInterval(t); setScan(false); setText(hit.rawValue); void submit(hit.rawValue) }
        }, 400)
      } catch { setScan(false); setErr(new Error('The camera is not available here. Paste the pairing text instead.')) }
    })()
    return () => { stop = true; clearInterval(t); stream?.getTracks().forEach((x) => x.stop()) }
  }, [scan]) // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <main className="pair">
      <DeviceMobile size={34} aria-hidden />
      <h1>Pair with your desktop</h1>
      <p className="muted">On the desktop open Settings, Mobile companion, turn it on and choose Pair a phone. Then use one of the options below. Both devices need to be on the same network (or Tailscale).</p>
      <form className="stack" style={{ gap: 18 }} onSubmit={(e) => { e.preventDefault(); void submit() }}>
        {canScan && (scan
          ? <div className="scan-box"><video ref={vid} playsInline muted aria-label="Camera preview" /><button type="button" className="btn sm" onClick={() => setScan(false)}>Cancel</button></div>
          : <button type="button" className="btn" onClick={() => setScan(true)}><QrCode size={16} aria-hidden />Scan the QR code</button>)}
        <div className="field"><label htmlFor="ptext">Pairing text</label>
          <textarea id="ptext" rows={3} value={text} onChange={(e) => setText(e.target.value)} placeholder='Paste the text copied from the desktop ({"v":1,"hosts":[...]})' spellCheck={false} autoCapitalize="off" /></div>
        <details className="adv" open={!text && !!(f.host || f.fp || f.code)}><summary>Or enter the details by hand</summary>
          <div className="stack" style={{ gap: 12, marginTop: 10 }}>
            <div className="two"><div className="field"><label htmlFor="ph">Desktop address</label><input id="ph" value={f.host} onChange={set('host')} placeholder="192.168.1.20" inputMode="url" autoCapitalize="off" autoComplete="off" /></div>
              <div className="field"><label htmlFor="pp">Port</label><input id="pp" type="number" value={f.port} onChange={set('port')} /></div></div>
            <div className="field"><label htmlFor="pf">Certificate fingerprint</label><input id="pf" className="mono" value={f.fp} onChange={set('fp')} placeholder="64 hex characters" spellCheck={false} autoCapitalize="off" autoComplete="off" /></div>
            <div className="field"><label htmlFor="pc">One-time code</label><input id="pc" className="mono" value={f.code} onChange={set('code')} placeholder="K7QM4XD9" autoCapitalize="characters" autoComplete="off" /></div>
          </div></details>
        <div className="field"><label htmlFor="pn">Name this phone</label><input id="pn" value={name} onChange={(e) => setName(e.target.value)} maxLength={40} autoComplete="off" /></div>
        {err !== null && <ErrorNotice error={err} compact />}
        <button className="btn primary lg" disabled={busy || !(text.trim() || (f.host && f.fp && f.code))}>{busy ? 'Connecting' : 'Pair'}</button>
      </form>
    </main>
  )
}

/** Status chip, unpair, and the "desktop unreachable" banner. Probes the desktop every 20s and when the app returns to the foreground. */
export function CompanionBar() {
  const st = mode.companion!
  const [code, setCode] = useState<string | null>(null)
  const [confirm, setConfirm] = useState(false)
  const [busy, setBusy] = useState(false)
  const probe = useCallback(async () => { setBusy(true); setCode(await api.companionHealth()); setBusy(false) }, [])
  useEffect(() => {
    void probe()
    const t = setInterval(() => { if (!document.hidden) void probe() }, 20000)
    const v = () => { if (!document.hidden) void probe() }
    document.addEventListener('visibilitychange', v)
    return () => { clearInterval(t); document.removeEventListener('visibilitychange', v) }
  }, [probe])
  const unpair = async () => {
    await api.companionUnpair()
    const u = new URL(location.href); if (mode.demo) u.searchParams.set('paired', '0')
    location.href = u.toString()
  }
  return (
    <div className="cobar" role="region" aria-label="Desktop connection">
      <span className={`badge co-chip ${code ? 'down' : 'up'}`}><i aria-hidden />Paired with {st.host}<span className="faint">{st.device_name}</span></span>
      {confirm
        ? <span className="row" style={{ gap: 6 }}><button className="btn sm" onClick={unpair}>Confirm unpair</button><button className="btn sm ghost" onClick={() => setConfirm(false)}>Keep</button></span>
        : <button className="btn sm ghost" onClick={() => setConfirm(true)}>Unpair</button>}
      {code && (
        <div className="co-banner">
          <ErrorNotice code={code} compact />
          <div className="row" style={{ gap: 8 }}>
            {code !== 'companion.revoked' && code !== 'net.pinning_failed' && <button className="btn sm" disabled={busy} onClick={probe}><LinkBreak size={14} aria-hidden />{busy ? 'Trying' : 'Retry'}</button>}
            {(code === 'companion.revoked' || code === 'net.pinning_failed') && <button className="btn sm" onClick={unpair}>Unpair</button>}
          </div>
        </div>
      )}
    </div>
  )
}

/** Calls `use` with text shared from the Android share sheet (once), now and whenever the app returns to the foreground. */
export function useSharedText(use: (t: string) => void) {
  const ref = useRef(use); ref.current = use
  useEffect(() => {
    if (!mode.companion?.paired) return
    const check = () => { api.companionShare().then((t) => { if (t) ref.current(t) }, () => {}) }
    check()
    const v = () => { if (!document.hidden) check() }
    document.addEventListener('visibilitychange', v)
    return () => document.removeEventListener('visibilitychange', v)
  }, [])
}
