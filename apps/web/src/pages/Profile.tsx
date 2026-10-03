import { ShieldCheck } from '@phosphor-icons/react'
import { useEffect, useMemo, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import { Rise, Skel } from '../components/Motion'
import { HBars } from '../components/Viz'
import type { SkillLevel, ProfileFacts, Ym } from '../types'

const LEVELS: SkillLevel[] = ['Beginner', 'Working', 'Strong', 'Expert']
const mi = ([y, m]: Ym) => y * 12 + m - 1
const MON = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']
const ymText = ([y, m]: Ym) => `${MON[m - 1]} ${y}`
const KIND = { FullTime: 'Full-time', Internship: 'Internship', Other: 'Other' } as const

function Gantt({ f }: { f: ProfileFacts }) {
  const rows = useMemo(() => {
    const by = new Map<string, ProfileFacts['skill_timeline']>()
    f.skill_timeline.forEach((t) => by.set(t.skill, [...(by.get(t.skill) ?? []), t]))
    return [...by.entries()].map(([skill, iv]) => ({ skill, iv, total: iv.reduce((a, t) => a + t.months, 0) })).sort((a, b) => b.total - a.total).slice(0, 14)
  }, [f.skill_timeline])
  if (!rows.length) return <p className="small muted">No dated skills yet. Add dates to your roles and projects.</p>
  const all = f.skill_timeline.flatMap((t) => [mi(t.start), mi(t.end) + 1])
  const lo = Math.floor(Math.min(...all) / 12) * 12, hi = Math.ceil(Math.max(...all) / 12) * 12, span = hi - lo || 12
  const step = span / 12 > 8 ? 2 : 1
  const years = Array.from({ length: Math.floor(span / 12 / step) + 1 }, (_, i) => lo / 12 + i * step)
  const x = (m: number) => `${((m - lo) / span) * 100}%`
  return (
    <div className="gantt" role="img" aria-label={`Skill timeline: ${rows.map((r) => `${r.skill} ${r.total} months`).join(', ')}`}>
      {rows.map((r) => (
        <div key={r.skill} className="g-row" title={`${r.skill}: ${r.total} months`}>
          <span>{r.skill}</span>
          <div className="g-track">{r.iv.map((t, i) => <i key={i} style={{ left: x(mi(t.start)), width: `calc(${x(mi(t.end) + 1)} - ${x(mi(t.start))})`, minWidth: 3 }} />)}</div>
        </div>
      ))}
      <div className="g-axis"><div /><div>{years.map((y) => <span key={y} style={{ left: x(y * 12) }}>{y}</span>)}</div></div>
    </div>
  )
}

export default function Profile() {
  const [f, setF] = useState<ProfileFacts | null>(null)
  const [err, setErr] = useState<{ msg: string; none: boolean } | null>(null)
  useEffect(() => { api.getProfileFacts().then(setF, (e) => setErr({ msg: e.message, none: e.status === 404 })) }, [])
  const lvl = useMemo(() => new Map(f?.skills.map((s) => [s.canonical, s.level]) ?? []), [f])
  const roleName = (i: number) => f?.timeline.find((t) => t.idx === i)?.org ?? ''
  return (
    <>
      <div className="page-head"><h1>Your profile model</h1><p>What the app understood from your resume, computed on this machine from your own words.</p></div>
      {err ? <Rise className="empty"><h2>{err.none ? 'No resume yet' : 'Could not build your profile'}</h2><p>{err.none ? 'Add your resume and this page fills in.' : err.msg}</p><Link className="btn primary" to="/">Go to intake</Link></Rise>
        : !f ? <div className="stack"><Skel h={56} r={12} /><Skel h={320} r={18} /></div> : (
          <div className="stack gap-xl">
            <div className="truth"><ShieldCheck size={20} aria-hidden style={{ flex: 'none', marginTop: 1 }} /><span><b>Ground truth:</b> the AI may not claim anything not shown here. Rewrites that add a skill, number or employer missing from this model are reverted.</span></div>
            <Rise as="section" className="panel" aria-label="Skill timeline"><header><h2>Skill timeline</h2><p>When each skill was in use, from the dates on your roles and projects.</p></header><Gantt f={f} /></Rise>
            <div className="cols">
              <Rise as="section" className="panel" aria-label="Proficiency" inView>
                <header><h2>Proficiency</h2><p>From months of use, recency, where it appears and how strongly your bullets describe it.</p></header>
                <ul className="hbars clean">
                  {f.skill_matrix.map((s) => {
                    const l = lvl.get(s.skill), n = l ? LEVELS.indexOf(l) + 1 : 0
                    return (<li key={s.skill} style={{ gridTemplateColumns: 'minmax(80px, 120px) minmax(0, 1fr) 118px' }} title={`${s.skill}: ${Math.round(s.proficiency * 100)}%`}><span className="hb-n">{s.skill}</span>
                      <span className="hb-t"><i style={{ width: `${s.proficiency * 100}%`, background: 'var(--c1)' }} /></span>
                      <span className="lvl">{LEVELS.map((x, k) => <i key={x} className={k < n ? 'on' : ''} />)}<span style={{ marginLeft: 4 }}>{l ?? ''}</span></span></li>)
                  })}
                </ul>
                <p className="hint">Beginner, Working, Strong, Expert.</p>
              </Rise>
              <div className="stack gap-xl">
                <Rise as="section" className="panel" aria-label="Years by role type" inView i={1}>
                  <header><h2>Years by role type</h2><p>{f.years.total_effective.toFixed(1)} effective years (an internship counts half).</p></header>
                  <HBars label="Years by role type" fmt={(v) => `${v.toFixed(1)} y`} rows={[{ name: 'Full-time', value: f.years.fulltime }, { name: 'Internship', value: f.years.internship }, { name: 'Other', value: f.years.other }]} />
                  <ul className="clean stack" style={{ gap: 6 }}>{f.timeline.map((t) => <li key={t.idx} className="small"><b>{t.role}</b>, {t.org} <span className="muted">({KIND[t.kind]}, {t.start ? ymText(t.start) : 'undated'}{t.is_current ? ' to now' : t.end ? ` to ${ymText(t.end)}` : ''})</span></li>)}</ul>
                </Rise>
                <Rise as="section" className="panel" aria-label="Domains" inView i={2}>
                  <header><h2>Domain weights</h2><p>Share of your total proficiency.</p></header>
                  <HBars label="Domain weights" max={1} fmt={(v) => `${Math.round(v * 100)}%`} rows={f.domains.map((d) => ({ name: d.name, value: d.weight }))} />
                </Rise>
              </div>
            </div>
            <Rise as="section" className="panel" aria-label="Achievements" inView>
              <header><h2>Top achievements</h2><p>Bullets with the most measurable impact. Their numbers are the only numbers the AI may use.</p></header>
              {f.achievements.length === 0 ? <p className="small muted">No bullets with numbers found. Adding measurable results makes rewrites stronger.</p> : (
                <ul className="clean stack" style={{ gap: 14 }}>{[...f.achievements].sort((a, b) => b.impact_score - a.impact_score).slice(0, 6).map((a) => (
                  <li key={`${a.role_idx}.${a.bullet_idx}`} className="stack" style={{ gap: 6 }}><span>{a.text}</span>
                    <span className="row" style={{ gap: 6 }}>{a.metrics.map((m, i) => <span key={i} className="metric">{m.raw}</span>)}<span className="tiny muted">{roleName(a.role_idx)}</span></span></li>))}</ul>
              )}
            </Rise>
            <Rise as="section" className="panel" aria-label="Career gaps" inView>
              <header><h2>Career gaps</h2><p>Stretches between full-time roles that a reader may ask about.</p></header>
              {f.gaps_in_timeline.length === 0 ? <p className="small muted">No gaps found.</p> : <ul className="clean stack" style={{ gap: 6 }}>{f.gaps_in_timeline.map(([a, b, m], i) => <li key={i} className="small">{ymText(a)} to {ymText(b)}, {m} months</li>)}</ul>}
            </Rise>
          </div>
        )}
    </>
  )
}
