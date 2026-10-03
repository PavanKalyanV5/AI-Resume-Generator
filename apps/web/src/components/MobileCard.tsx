import { DeviceMobile } from '@phosphor-icons/react'
import QRCode from 'qrcode'
import { useCallback, useEffect, useState } from 'react'
import { api } from '../api'
import { ago } from '../lib/util'
import { useApp } from '../store'
import type { PairCode, RemoteAudit, RemoteDevice, RemoteStatus } from '../types'
import { Toggle } from './Controls'
import { ErrorNotice } from './ErrorNotice'

const iso = (ms: number) => new Date(ms).toISOString()

function Pair({ p, again }: { p: PairCode; again: () => void }) {
  const { toast } = useApp()
  const [qr, setQr] = useState('')
  const [now, setNow] = useState(Date.now())
  useEffect(() => { void QRCode.toDataURL(JSON.stringify(p.qr_payload), { margin: 1, width: 224, errorCorrectionLevel: 'M' }).then(setQr) }, [p])
  useEffect(() => { const t = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(t) }, [])
  const left = Math.max(0, Math.round((p.expires_at - now) / 1000))
  return (
    <div className="row" style={{ gap: 20, alignItems: 'flex-start' }}>
      {qr && left > 0 ? <img src={qr} width={224} height={224} alt="QR code to pair your phone" style={{ background: '#fff', borderRadius: 12 }} /> : <div className="hint" style={{ width: 224 }}>Code expired.</div>}
      <div className="stack" style={{ gap: 8 }}>
        <p className="hint" style={{ margin: 0 }}>In the companion app, scan the QR code or type this code. It works once.</p>
        <code className="mono" style={{ fontSize: 28, letterSpacing: 4 }}>{left > 0 ? p.code : '--------'}</code>
        <span className="tiny muted" aria-live="off">{left > 0 ? `Expires in ${Math.floor(left / 60)}:${String(left % 60).padStart(2, '0')}` : 'Expired'}</span>
        <div className="row"><button className="btn sm" onClick={again}>New code</button>
          <button className="btn sm" onClick={() => { void navigator.clipboard?.writeText(JSON.stringify(p.qr_payload)); toast({ kind: 'ok', text: 'Pairing text copied. Paste it in the phone app.' }) }}>Copy pairing text</button></div>
      </div>
    </div>
  )
}

export function MobileCard() {
  const [s, setS] = useState<RemoteStatus | null>(null)
  const [devs, setDevs] = useState<RemoteDevice[]>([])
  const [log, setLog] = useState<RemoteAudit[]>([])
  const [pair, setPair] = useState<PairCode | null>(null)
  const [err, setErr] = useState<unknown>(null)
  const [busy, setBusy] = useState(false)
  const [gone, setGone] = useState(false)
  const { toast } = useApp()
  const load = useCallback(async () => {
    try { const st = await api.remoteStatus(); setS(st); setDevs(await api.remoteDevices()); setLog(st.enabled ? await api.remoteAudit() : []) } catch { setGone(true) }
  }, [])
  useEffect(() => { void load() }, [load])
  const run = async (f: () => Promise<unknown>) => { setBusy(true); setErr(null); try { await f(); await load() } catch (e) { setErr(e) } finally { setBusy(false) } }
  if (gone) return <section className="panel" aria-label="Mobile companion"><header><h2><DeviceMobile size={18} aria-hidden />Mobile companion</h2><p>Remote access is managed from the computer that runs the app, not from a phone.</p></header></section>
  if (!s) return <section className="panel" aria-label="Mobile companion"><header><h2><DeviceMobile size={18} aria-hidden />Mobile companion</h2></header></section>
  const toggle = (on: boolean) => run(async () => { setS(await api.remoteToggle(on)); if (!on) setPair(null) })
  const newCode = () => run(async () => setPair(await api.remotePairCode()))
  return (
    <section className="panel" aria-label="Mobile companion">
      <header>
        <div className="row between"><h2><DeviceMobile size={18} aria-hidden />Mobile companion</h2><Toggle on={s.enabled} onChange={toggle} disabled={busy} label="Allow phones to connect" /></div>
        <p>Check on jobs and approve results from your phone. Heavy work stays on this computer.</p>
      </header>
      {err !== null && <ErrorNotice error={err} />}
      {!s.enabled ? (
        <p className="hint">Off. Turning it on opens an encrypted (HTTPS) port on your local network, so any device on that network can reach the sign-in page. Only phones you pair with a one-time code can see anything. Turn it off any time.</p>
      ) : (
        <div className="stack" style={{ gap: 16 }}>
          <div className="stack" style={{ gap: 6 }}>
            <span className="label">Reachable at</span>
            <div className="row">{s.addresses.map((a) => <span key={a.ip} className="badge mono">{a.ip}:{s.port}{a.kind === 'tailscale' ? ' (Tailscale)' : ''}</span>)}</div>
            <span className="label">Certificate fingerprint</span>
            <code className="mono tiny" style={{ wordBreak: 'break-all' }}>{s.fingerprint}</code>
            <p className="hint" style={{ margin: 0 }}>The phone checks this fingerprint when it connects. Pairing shows it in the QR code.</p>
            {s.cert_stale && <p className="hint">This computer's network addresses changed since the certificate was made. Phones can still connect by an old address only.</p>}
          </div>
          {pair ? <Pair p={pair} again={newCode} /> : <div><button className="btn primary" disabled={busy} onClick={newCode}>Pair a phone</button></div>}
          <div className="stack" style={{ gap: 8 }}>
            <span className="label">Paired phones</span>
            {devs.length === 0 && <p className="hint" style={{ margin: 0 }}>None yet.</p>}
            {devs.map((d) => (
              <div key={d.id} className="row between"><span>{d.name} <span className="tiny muted">{d.last_seen ? `last seen ${ago(iso(d.last_seen))}` : 'never used'}</span></span>
                <button className="btn sm" disabled={busy} onClick={() => run(async () => { await api.remoteRevoke(d.id); toast({ kind: 'ok', text: `${d.name} can no longer connect.` }) })}>Revoke</button></div>
            ))}
          </div>
          {log.length > 0 && (
            <details><summary className="label">Recent phone activity</summary>
              <ul className="tiny mono" style={{ margin: '8px 0 0', padding: 0, listStyle: 'none' }}>{log.map((a, i) => <li key={i}>{ago(iso(a.ts))} · {a.device} · {a.method} {a.path} · {a.status}</li>)}</ul></details>
          )}
        </div>
      )}
      <p className="hint"><b>What a phone can do:</b> see your jobs and results, start or approve a job, read notifications and your learning playbook. <b>What it cannot do:</b> reveal redacted personal details, see your API keys, change settings, or import or replace your resume.</p>
    </section>
  )
}
