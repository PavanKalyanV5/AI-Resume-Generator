import { ShieldCheck, CloudArrowUp, EyeSlash, Lock } from '@phosphor-icons/react'
import { motion } from 'motion/react'
import { Fragment, useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import { EASE, Rise } from '../components/Motion'
import { StepRail, Where, ZoneLegend } from '../components/Steps'
import { useJobCtx } from './JobDetail'
import { isDone } from '../types'

const TOKEN = /((?:\[|<|⟦)?\b[A-Z][A-Z_]{2,}_\d+\b(?:\]|>|⟧)?)/g
const Tokens = ({ s }: { s: string }) => <>{s.split(TOKEN).map((p, i) => (i % 2 ? <mark key={i} className="tok">{p}</mark> : <Fragment key={i}>{p}</Fragment>))}</>

function Code({ text }: { text: string }) {
  let pretty = text
  try { pretty = JSON.stringify(JSON.parse(text), null, 2) } catch { /* plain text */ }
  const re = /("(?:\\.|[^"\\])*")(\s*:)?|(-?\b\d+(?:\.\d+)?\b)|([{}[\],])/g
  const out: React.ReactNode[] = []
  let last = 0, m: RegExpExecArray | null, i = 0
  while ((m = re.exec(pretty))) {
    if (m.index > last) out.push(<Tokens key={i++} s={pretty.slice(last, m.index)} />)
    if (m[1]) out.push(<span key={i++} className={m[2] ? 'k' : 's'}><Tokens s={m[1]} /></span>, m[2] ? <span key={i++} className="p">{m[2]}</span> : null)
    else if (m[3]) out.push(<span key={i++} className="n">{m[3]}</span>)
    else out.push(<span key={i++} className="p">{m[4]}</span>)
    last = re.lastIndex
  }
  out.push(<Tokens key={i++} s={pretty.slice(last)} />)
  return <pre className="code" tabIndex={0}>{out}</pre>
}

export default function Privacy() {
  const { data } = useJobCtx()
  const { audit, steps } = data
  const ready = !!audit.redacted_payload.user
  const counts = Object.entries(audit.token_counts)
  const total = counts.reduce((a, [, n]) => a + n, 0)
  const max = Math.max(1, ...counts.map(([, n]) => n))
  const [piiN, setPiiN] = useState<number | null>(null)
  useEffect(() => { api.getPii().then((x) => setPiiN(x.entries.length), () => {}) }, [])
  const restore = steps.find((s) => s.name === 'restore')
  const drive = steps.find((s) => s.name === 'upload_drive')
  if (!ready) return <div className="empty"><h2>Nothing to audit yet</h2><p>The payload is built in the third step. It appears here before it is sent, so you can check it.</p></div>
  return (
    <div className="stack gap-xl">
      <Rise as="section" className="panel sent-panel" aria-label="What was sent">
        <header>
          <div className="row"><h2>Exactly what the provider received</h2><Where zone="ai" />{piiN !== null && <Link className="chip-link z-local" to="/pii"><ShieldCheck size={14} aria-hidden />{piiN} tracked values, PII ledger</Link>}</div>
          <p>This is the full redacted request for the AI step. Highlighted tokens stand in for your real values.</p>
        </header>
        <StepRail drive={!!drive} /><ZoneLegend drive={!!drive} />
        <div className="stack" style={{ gap: 6 }}><h3>System prompt</h3><Code text={audit.redacted_payload.system} /></div>
        <div className="stack" style={{ gap: 6 }}><h3>User message</h3><Code text={audit.redacted_payload.user} /></div>
      </Rise>
      <div className="cols">
        <Rise as="section" className="panel" aria-label="Tokens by class" inView>
          <header><h2>Tokens by class</h2><p>{total} values were replaced before sending.</p></header>
          <ul className="clean stack" style={{ gap: 10 }}>
            {counts.map(([k, n], i) => (
              <li key={k} className="tokrow">
                <span className="mono small">{k}</span>
                <div className="bar-line sent" role="img" aria-label={`${k}: ${n}`}>{n > 0 && <motion.i initial={{ scaleX: 0 }} animate={{ scaleX: n / max }} transition={{ duration: 0.8, ease: EASE, delay: 0.1 + i * 0.07 }} />}</div>
                <span className="num small" style={{ textAlign: 'right' }}>{n}</span>
              </li>
            ))}
          </ul>
        </Rise>
        <Rise as="section" className="panel" aria-label="Restore result" inView i={1}>
          <header><h2>Restore result</h2><p>After the provider answers, the tokens are swapped back on this machine.</p></header>
          <div className="row"><Where zone="local" />
            <span className={`badge ${restore && isDone(restore.status) ? 'ok' : restore?.status === 'failed' ? 'err' : ''}`}>{restore?.status ?? 'pending'}</span></div>
          <p className="small muted">{restore && isDone(restore.status) ? `All ${total} tokens were restored. The original names and employers are back in your tailored resume.` : 'Waiting for the AI step to finish.'}</p>
          <div className="row small muted" style={{ gap: 8, alignItems: 'flex-start', flexWrap: 'nowrap' }}><EyeSlash size={16} aria-hidden style={{ flex: 'none', marginTop: 3 }} />
            <span>Values hidden. This page shows tokens and counts only. The mapping from token to real value never leaves your machine and is not shown here.</span></div>
          <div className="row small muted" style={{ gap: 8, alignItems: 'flex-start', flexWrap: 'nowrap' }}><Lock size={16} aria-hidden style={{ flex: 'none', marginTop: 3 }} />
            <span>Only names, emails, phone numbers, employers and clients are tokenised, plus anything you added to Also redact.</span></div>
        </Rise>
      </div>
      {drive && (
        <Rise as="section" className="panel drive-panel" inView aria-label="Google Drive">
          <header><div className="row"><h2>Google Drive</h2><Where zone="drive" /></div>
            <p>This step is separate from the AI step. It copies your finished PDF and DOCX to a folder in your own Google account. No AI provider sees them.</p></header>
          <div className="row small muted" style={{ gap: 8, flexWrap: 'nowrap' }}><CloudArrowUp size={16} aria-hidden style={{ flex: 'none' }} />
            <span>{drive.status === 'succeeded' ? 'Uploaded. The links are on the job page.' : drive.status === 'failed' ? 'The upload failed. Your files are still on this machine; retry from the job page.' : 'Waiting for the files to be ready.'}</span></div>
        </Rise>
      )}
    </div>
  )
}
