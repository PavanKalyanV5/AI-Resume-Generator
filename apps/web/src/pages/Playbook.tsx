import { ArrowCounterClockwise, BookBookmark, Check, Play, Plus, X } from '@phosphor-icons/react'
import { useCallback, useEffect, useState } from 'react'
import { Link, useLocation } from 'react-router-dom'
import { api } from '../api'
import { Rise, Skel } from '../components/Motion'
import { jobTitle } from '../lib/util'
import { useApp } from '../store'
import type { Impact, Matcher, Playbook as PB, PlaybookRule, RegressionRun, RuleBody, RuleStatus } from '../types'

const msg = (e: unknown, d = 'That did not work.') => (e instanceof Error ? e.message : d)
const AREAS = ['certs', 'projects', 'skills', 'roles', 'summary', 'bullets']
const FAMILIES = ['backend', 'frontend', 'fullstack', 'data_eng', 'ml_ai', 'devops_cloud', 'security']
const KIND: Record<string, string> = { certs: 'cert', projects: 'project', skills: 'skill', roles: 'role' }
const GROUPS: [RuleStatus, string, string][] = [
  ['proposed', 'Suggested rules', 'Learned from your corrections. Nothing applies until you approve it.'],
  ['active', 'Active', 'Applied to every new and re-run job where the rule fits.'],
  ['disabled', 'Disabled', 'Switched off. Enable them any time.'],
  ['rejected', 'Rejected', 'You said no. These are never suggested again.'],
]
const scopeText = (s: string) => (s === 'global' ? 'All jobs' : s.startsWith('family:') ? `${s.slice(7).replace(/_/g, ' ')} jobs` : `Company ${s.slice(8)}`)
const originText = (r: PlaybookRule) => (r.origin === 'seed' ? r.badge ?? 'From your 2026-10-03 feedback' : r.origin === 'mined' ? 'Learned' : 'Yours')
const actionText = (r: PlaybookRule) => {
  const a = r.action
  return a.forbid ? 'Avoid' : a.require ? 'Always include' : a.cap != null ? `At most ${a.cap}` : a.boost != null ? `${a.boost >= 0 ? 'Prefer' : 'Demote'} (${a.boost > 0 ? '+' : ''}${+a.boost.toFixed(2)})` : a.prefer_over ? 'Prefer over others' : r.area === 'summary' || r.area === 'bullets' ? 'Writing guidance' : ''
}
const matchText = (m: Matcher) => [m.issuer_in?.length && `issuer: ${m.issuer_in.join(', ')}`, m.title_regex && `title matches ${m.title_regex}`, m.tier && `tier ${m.tier}`, m.tech_any?.length && `tech: ${m.tech_any.join(', ')}`, m.domain_bucket && `${m.domain_bucket} topics`, m.language_specific != null && (m.language_specific ? 'language-specific basics' : 'not language-specific'), m.frontend_only != null && (m.frontend_only ? 'frontend-only' : 'not frontend-only')].filter(Boolean).join(' · ')
const csv = (s: string) => s.split(',').map((x) => x.trim()).filter(Boolean)
const tri = (v: boolean | null | undefined) => (v == null ? '' : v ? 'yes' : 'no')
const unTri = (v: string) => (v === '' ? null : v === 'yes')

/** Add or edit one rule. The server validates; its message is shown as is. */
function RuleForm({ initial, onSave, onCancel }: { initial?: PlaybookRule; onSave: (b: RuleBody) => Promise<void>; onCancel?: () => void }) {
  const m0 = initial?.matcher ?? {}, a0 = initial?.action ?? {}
  const [area, setArea] = useState(initial?.area ?? 'certs')
  const [scopeKind, setScopeKind] = useState(initial?.scope.split(':')[0] ?? 'global')
  const [scopeVal, setScopeVal] = useState(initial?.scope.split(':')[1] ?? '')
  const [act, setAct] = useState(a0.forbid ? 'forbid' : a0.require ? 'require' : a0.cap != null ? 'cap' : a0.prefer_over && a0.boost == null ? 'prefer' : 'boost')
  const [num, setNum] = useState(String(a0.cap ?? (a0.boost != null ? +a0.boost.toFixed(2) : 0.25)))
  const [issuers, setIssuers] = useState((m0.issuer_in ?? []).join(', '))
  const [regex, setRegex] = useState(m0.title_regex ?? '')
  const [tier, setTier] = useState(m0.tier ?? '')
  const [tech, setTech] = useState((m0.tech_any ?? []).join(', '))
  const [domain, setDomain] = useState(m0.domain_bucket ?? '')
  const [lang, setLang] = useState(tri(m0.language_specific))
  const [front, setFront] = useState(tri(m0.frontend_only))
  const [text, setText] = useState(initial?.text ?? '')
  const [err, setErr] = useState('')
  const [busy, setBusy] = useState(false)
  const item = area in KIND
  const acts = area === 'roles' ? ['forbid'] : area === 'skills' ? ['boost'] : ['boost', 'forbid', 'require', 'cap', ...(act === 'prefer' ? ['prefer'] : [])]
  const submit = async (e: React.FormEvent) => {
    e.preventDefault(); setBusy(true); setErr('')
    const a = item ? (act === 'prefer' ? a0 : { prefer_over: a0.prefer_over ?? null, boost: act === 'boost' ? Number(num) : null, forbid: act === 'forbid', require: act === 'require', cap: act === 'cap' ? Number(num) : null }) : {}
    const m: Matcher = item ? { ...m0, kind: KIND[area], issuer_in: csv(issuers), title_regex: regex.trim() || null, tier: tier.trim() || null, tech_any: csv(tech), domain_bucket: domain.trim() || null, language_specific: unTri(lang), frontend_only: unTri(front) } : {}
    try { await onSave({ text, area, scope: scopeKind === 'global' ? 'global' : `${scopeKind}:${scopeVal.trim()}`, matcher: m, action: a }); if (!initial) setText('') } catch (x) { setErr(msg(x)) } finally { setBusy(false) }
  }
  const id = initial?.id ?? 'new'
  return (
    <form className="rform" onSubmit={submit}>
      <div className="g2">
        <div className="field"><label htmlFor={`a-${id}`}>Applies to</label>
          <select id={`a-${id}`} value={area} onChange={(e) => { setArea(e.target.value); setAct(e.target.value === 'roles' ? 'forbid' : 'boost') }}>{AREAS.map((x) => <option key={x} value={x}>{x === 'summary' || x === 'bullets' ? `Writing: ${x}` : x}</option>)}</select></div>
        <div className="field"><label htmlFor={`w-${id}`}>When the job is</label>
          <div className="row nowrap"><select id={`w-${id}`} value={scopeKind} onChange={(e) => setScopeKind(e.target.value)}><option value="global">Any job</option><option value="family">In family</option><option value="company">At company</option></select>
            {scopeKind !== 'global' && <><input aria-label="Family or company" list="fams" value={scopeVal} onChange={(e) => setScopeVal(e.target.value)} placeholder={scopeKind === 'family' ? 'data_eng' : 'Acme Robotics'} /><datalist id="fams">{FAMILIES.map((f) => <option key={f} value={f} />)}</datalist></>}</div></div>
      </div>
      {item && (
        <>
          <div className="g2">
            <div className="field"><label htmlFor={`x-${id}`}>What to do</label>
              <div className="row nowrap"><select id={`x-${id}`} value={act} onChange={(e) => setAct(e.target.value)}>{acts.map((x) => <option key={x} value={x}>{({ boost: 'Prefer or demote', forbid: 'Avoid', require: 'Always include', cap: 'Limit how many', prefer: 'Prefer over others (kept)' } as Record<string, string>)[x]}</option>)}</select>
                {(act === 'boost' || act === 'cap') && <input type="number" aria-label={act === 'boost' ? 'Boost, -0.5 to 0.5' : 'Maximum count'} step={act === 'boost' ? 0.05 : 1} min={act === 'boost' ? -0.5 : 1} max={act === 'boost' ? 0.5 : undefined} value={num} onChange={(e) => setNum(e.target.value)} style={{ width: 90 }} />}</div>
              <span className="hint">{act === 'boost' ? 'Positive helps an item win a slot, negative pushes it down (-0.5 to 0.5).' : ''}</span></div>
            <div className="field"><label htmlFor={`i-${id}`}>Items whose issuer contains</label><input id={`i-${id}`} value={issuers} onChange={(e) => setIssuers(e.target.value)} placeholder="google, aws" /></div>
          </div>
          <div className="g2">
            <div className="field"><label htmlFor={`r-${id}`}>Title matches (regex)</label><input id={`r-${id}`} value={regex} onChange={(e) => setRegex(e.target.value)} placeholder="(?i)basics|bootcamp" /></div>
            <div className="field"><label htmlFor={`t-${id}`}>Uses any technology</label><input id={`t-${id}`} value={tech} onChange={(e) => setTech(e.target.value)} placeholder="kubernetes, go" /></div>
            <div className="field"><label htmlFor={`d-${id}`}>Topic</label><input id={`d-${id}`} value={domain} onChange={(e) => setDomain(e.target.value)} placeholder="cloud, data, ml, security" /></div>
            <div className="field"><label htmlFor={`ti-${id}`}>Project tier</label><input id={`ti-${id}`} value={tier} onChange={(e) => setTier(e.target.value)} placeholder="featured" /></div>
            <div className="field"><label htmlFor={`l-${id}`}>Language-specific basics</label><select id={`l-${id}`} value={lang} onChange={(e) => setLang(e.target.value)}><option value="">Either</option><option value="yes">Yes</option><option value="no">No</option></select></div>
            <div className="field"><label htmlFor={`f-${id}`}>Frontend-only</label><select id={`f-${id}`} value={front} onChange={(e) => setFront(e.target.value)}><option value="">Either</option><option value="yes">Yes</option><option value="no">No</option></select></div>
          </div>
        </>
      )}
      <div className="field"><label htmlFor={`tx-${id}`}>In plain words</label><textarea id={`tx-${id}`} value={text} onChange={(e) => setText(e.target.value)} maxLength={300} placeholder="For cloud jobs prefer recognised cloud certificates." /><span className="hint">Shown on the rule and given to the writer. No names, emails, links or phone numbers.</span></div>
      {err && <div className="err-box" role="alert">{err}</div>}
      <div className="row"><button className="btn primary" disabled={busy || !text.trim()}>{busy ? 'Saving...' : initial ? 'Save changes' : 'Add rule'}</button>{onCancel && <button type="button" className="btn ghost" onClick={onCancel}>Cancel</button>}</div>
    </form>
  )
}

/** Two-step button: the first click arms it. */
function Twice({ label, sure, onGo, cls = 'btn sm ghost' }: { label: string; sure: string; onGo: () => void; cls?: string }) {
  const [armed, setArmed] = useState(false)
  return armed ? <><button className="btn sm" onClick={() => { setArmed(false); onGo() }}>{sure}</button><button className="btn sm ghost" onClick={() => setArmed(false)}>Keep</button></> : <button className={cls} onClick={() => setArmed(true)}>{label}</button>
}

function RuleCard({ r, flash, act, onChanged }: { r: PlaybookRule; flash: boolean; act: (f: () => Promise<unknown>, ok?: string) => Promise<void>; onChanged: (impact?: { id: string; text: string; impact: Impact }) => void }) {
  const { jobs } = useApp()
  const [editing, setEditing] = useState(false)
  const [confirm, setConfirm] = useState(false)
  const job = (id: string) => { const j = jobs.find((x) => x.id === id); return j ? jobTitle(j) : `Job ${id.slice(0, 8)}` }
  const op = (o: 'approve' | 'reject' | 'disable' | 'enable') => act(async () => { const x = await api.ruleOp(r.id, o); if (o === 'approve' || o === 'enable') onChanged({ id: r.id, text: r.text, impact: x.impact ?? { would_change: 0, of: 0, cases: [], summary: '' } }); else onChanged() })
  const m = matchText(r.matcher)
  return (
    <article id={r.id} className={`rule ${r.status === 'proposed' ? 'prop' : ''} ${flash ? 'flash-on' : ''}`} aria-label={r.text}>
      <p className="rule-t">{r.text}</p>
      <div className="rule-m">
        <span className="badge">{scopeText(r.scope)}</span><span className="badge">{r.area}</span>
        <span className={`badge ${r.origin === 'user' ? 'z-local' : r.origin === 'mined' ? 'learn' : ''}`}>{originText(r)}</span>
        {actionText(r) && <span className="badge">{actionText(r)}</span>}
        <span className="small muted">Used {r.hits} {r.hits === 1 ? 'time' : 'times'}</span>
      </div>
      {m && <p className="small muted">Matches: {m}</p>}
      {r.support_count > 0 && <p className="small">Seen in {r.support_count} of your corrections{r.examples.length > 0 && <>: {r.examples.map((id, i) => <span key={id}>{i > 0 && ', '}<Link to={`/jobs/${id}`}>{job(id)}</Link></span>)}</>}.</p>}
      {r.impact && r.status === 'active' && <p className="small muted">When it was switched on, it {r.impact.summary || 'changed nothing'}.</p>}
      {confirm && (
        <div className="rule-box" role="group" aria-label="Confirm approval">
          <p>Approving turns this rule on for new and re-run jobs. Right after, the app replays it on your approved resumes and tells you how many would change. You can switch it off straight away.</p>
          <div className="row"><button className="btn sm primary" onClick={() => { setConfirm(false); void op('approve') }}><Check size={14} aria-hidden />Approve and check impact</button><button className="btn sm ghost" onClick={() => setConfirm(false)}>Not yet</button></div>
        </div>
      )}
      {editing ? <RuleForm initial={r} onCancel={() => setEditing(false)} onSave={async (b) => { await api.putRule(r.id, b); setEditing(false); onChanged() }} /> : (
        <div className="rule-a">
          {r.status === 'proposed' && !confirm && <><button className="btn sm primary" onClick={() => setConfirm(true)}><Check size={14} aria-hidden />Approve</button><button className="btn sm" onClick={() => op('reject')}><X size={14} aria-hidden />Reject</button></>}
          {r.status === 'active' && <button className="btn sm" onClick={() => op('disable')}>Disable</button>}
          {(r.status === 'disabled' || r.status === 'rejected') && <button className="btn sm" onClick={() => op('enable')}>Enable</button>}
          <button className="btn sm ghost" onClick={() => setEditing(true)}>Edit</button>
          {r.origin !== 'seed' && <Twice label="Delete" sure="Delete rule" onGo={() => act(async () => { await api.deleteRule(r.id); onChanged() })} />}
        </div>
      )}
    </article>
  )
}

export function Regression({ compact }: { compact?: boolean }) {
  const { jobs, toast } = useApp()
  const [run, setRun] = useState<RegressionRun | null | undefined>(undefined)
  const [busy, setBusy] = useState(false)
  useEffect(() => { api.latestRegression().then(setRun, () => setRun(null)) }, [])
  const go = async () => { setBusy(true); try { setRun(await api.runRegression()) } catch (e) { toast({ kind: 'err', text: msg(e) }) } finally { setBusy(false) } }
  const title = (id: string) => { const j = jobs.find((x) => x.id === id); return j ? jobTitle(j) : `Job ${id.slice(0, 8)}` }
  const nm: Record<string, string> = { projects: 'Projects', certs: 'Certifications', roles: 'Roles' }
  return (
    <Rise as="section" className="panel" aria-label="Regression check" inView>
      <header><h2>Regression check</h2><p>Every resume you approve becomes a test case. This replays your current rules on them, locally with no AI, and shows what would come out differently.</p></header>
      <div className="row between"><span className="small" role="status">{run === undefined ? 'Loading...' : run ? `${run.passing} of ${run.cases} approved resumes still come out the same. Last run ${new Date(run.ran_at).toLocaleString()}.` : 'Not run yet.'}</span>
        <button className="btn" onClick={go} disabled={busy}><Play size={16} aria-hidden />{busy ? 'Running...' : 'Run check'}</button></div>
      {run && run.cases === 0 && <p className="small muted">No test cases yet. Open a finished job and choose "Approve this resume".</p>}
      {run && run.results.length > 0 && (
        <ul className="clean stack" style={{ gap: 10 }}>{run.results.map((x) => (
          <li key={x.job_id} className={`reg-case ${x.pass ? '' : 'fail'}`}>
            <div className="row between"><Link to={`/jobs/${x.job_id}`}><b>{title(x.job_id)}</b></Link><span className={`badge ${x.pass ? 'ok' : 'err'}`}>{x.pass ? 'Same picks' : 'Differs'}</span></div>
            {Object.entries(x.diff).map(([k, d]) => (
              <div key={k}><div className="subh">{nm[k] ?? k}{d.order_only && ' (same items, different order)'}</div>
                {!d.order_only && <div className="reg-diff">
                  <div className="had"><span className="muted">Your approved resume had</span>{d.missing.length ? <ul>{d.missing.map((t) => <li key={t}>{t}</li>)}</ul> : <p className="muted">nothing extra</p>}</div>
                  <div className="pick"><span className="muted">Current rules would pick instead</span>{d.extra.length ? <ul>{d.extra.map((t) => <li key={t}>{t}</li>)}</ul> : <p className="muted">nothing extra</p>}</div>
                </div>}</div>
            ))}
          </li>))}</ul>
      )}
      {!compact && run && run.results.some((x) => !x.pass) && <p className="hint">A difference is not always wrong. If your newer rules are right, approve the resume again.</p>}
    </Rise>
  )
}

export default function Playbook() {
  const { toast, setProposed } = useApp()
  const { hash } = useLocation()
  const [pb, setPb] = useState<PB | null>(null)
  const [err, setErr] = useState('')
  const [adding, setAdding] = useState(false)
  const [done, setDone] = useState<{ id: string; text: string; impact: Impact } | null>(null)
  const load = useCallback(() => api.getPlaybook().then((p) => { setPb(p); setProposed(p.groups.proposed.length) }, (e) => setErr(msg(e))), [setProposed])
  useEffect(() => { void load() }, [load])
  useEffect(() => { if (pb && hash) document.getElementById(hash.slice(1))?.scrollIntoView({ block: 'center', behavior: 'smooth' }) }, [pb, hash])
  const act = async (f: () => Promise<unknown>) => { try { await f() } catch (e) { toast({ kind: 'err', text: msg(e) }) } finally { void load() } }
  const rules = pb?.rules ?? []
  return (
    <>
      <div className="page-head"><h1>Playbook</h1><p>Rules the app follows when it picks and words your resume. It suggests them from your corrections. You approve every one, and you can switch any off.</p></div>
      {err ? <div className="err-box">{err}</div> : !pb ? <div className="stack"><Skel h={120} r={18} /><Skel h={220} r={18} /></div> : (
        <div className="stack gap-xl">
          {done && (
            <div className={`rule-box ${done.impact.would_change ? '' : 'good'}`} role="status">
              <p><b>Rule is on.</b> {done.impact.of === 0 ? 'You have no approved resumes yet, so there is nothing to compare against.' : `It ${done.impact.summary}.`}</p>
              <div className="row"><button className="btn sm" onClick={() => act(async () => { await api.ruleOp(done.id, 'disable'); setDone(null); toast({ kind: 'info', text: 'Rule switched off again.' }) })}><ArrowCounterClockwise size={14} aria-hidden />Undo, switch it off</button><button className="btn sm ghost" onClick={() => setDone(null)}>Keep it</button></div>
            </div>
          )}
          {rules.length === 0 && <Rise className="empty"><BookBookmark size={34} aria-hidden /><h2>No rules yet</h2><p>Rules appear here once you correct a few jobs, or add your own below.</p></Rise>}
          {GROUPS.map(([st, title, sub]) => {
            const list = rules.filter((r) => r.status === st)
            if (!list.length && st !== 'proposed') return null
            return (
              <Rise as="section" key={st} className="panel" aria-label={title} i={0}>
                <header><h2>{title} <span className="badge">{list.length}</span></h2><p>{sub}</p></header>
                {list.length === 0 ? <p className="small muted">Nothing waiting. When you correct the same thing on 3 jobs, a suggestion shows up here.</p> : <div className="stack" style={{ gap: 12 }}>{list.map((r) => <RuleCard key={`${r.id}:${r.version}`} r={r} flash={hash === `#${r.id}`} act={act} onChanged={(imp) => { if (imp) setDone(imp); void load() }} />)}</div>}
              </Rise>
            )
          })}
          <Rise as="section" className="panel" aria-label="Add a rule" inView>
            <header><h2>Add a rule</h2><p>Write down a preference of your own. It starts working straight away.</p></header>
            {adding ? <RuleForm onCancel={() => setAdding(false)} onSave={async (b) => { await api.createRule(b); setAdding(false); toast({ kind: 'ok', text: 'Rule added.' }); await load() }} /> : <div><button className="btn" onClick={() => setAdding(true)}><Plus size={16} aria-hidden />New rule</button></div>}
          </Rise>
          <Regression />
          <Rise as="section" className="panel" aria-label="Versions" inView>
            <header><h2>Versions</h2><p>Every change saves a version. Roll back to undo several changes at once. Suggestions and rejections are kept.</p></header>
            <ul className="clean vers">{pb.versions.map((v, i) => (
              <li key={v.version}><span><b>v{v.version}</b> <span className="muted">{v.note}, {new Date(v.ts).toLocaleString()}</span></span>
                {i === 0 ? <span className="badge ok">Current</span> : <Twice label="Roll back" sure={`Roll back to v${v.version}`} onGo={() => act(async () => { setPb(await api.rollback(v.version)); toast({ kind: 'ok', text: `Rolled back to v${v.version}.` }) })} />}</li>))}</ul>
          </Rise>
        </div>
      )}
    </>
  )
}
