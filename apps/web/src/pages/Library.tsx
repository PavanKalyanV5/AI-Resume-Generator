import { ArrowUpRight, MagnifyingGlass, Books } from '@phosphor-icons/react'
import { AnimatePresence, motion } from 'motion/react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import { Seg } from '../components/Controls'
import { DriveLinks, FileButtons } from '../components/Files'
import { CountUp, EASE, Rise, Skel } from '../components/Motion'
import { ago } from '../lib/util'
import type { LibraryItem } from '../types'

export const CoverChip = ({ v }: { v: number | null }) => {
  if (v == null) return <span className="cov none" title="No coverage score">No score</span>
  const p = Math.round(v <= 1 ? v * 100 : v)
  return <span className={`cov ${p >= 75 ? 'hi' : p >= 55 ? 'mid' : 'lo'}`} title={`ATS coverage ${p}%`}><i style={{ ['--p' as string]: p }} aria-hidden /><b className="num">{p}%</b></span>
}

function Row({ it }: { it: LibraryItem }) {
  return (
    <motion.li layout="position" className="lib-row" initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, transition: { duration: 0.15 } }} transition={{ duration: 0.4, ease: EASE }}>
      <Link to={`/jobs/${it.id}`} className="lib-main">
        <span className="lib-role">{it.role}</span>
        <span className="small muted">{ago(it.created_at)}</span>
      </Link>
      <CoverChip v={it.coverage} />
      <div className="lib-actions">
        <FileButtons id={it.id} files={it.files.map((f) => f.name)} size="sm" />
        <DriveLinks links={it.drive} />
        <Link className="icon-link" to={`/jobs/${it.id}`} aria-label={`Open ${it.role} at ${it.company}`}><ArrowUpRight size={16} /></Link>
      </div>
    </motion.li>
  )
}

export default function Library() {
  const [items, setItems] = useState<LibraryItem[] | null>(null)
  const [err, setErr] = useState('')
  const [q, setQ] = useState('')
  const [view, setView] = useState<'company' | 'recent'>('company')
  const search = useRef<HTMLInputElement>(null)
  useEffect(() => { api.getLibrary().then(setItems, (e) => setErr(e.message)) }, [])
  useEffect(() => {
    const k = (e: KeyboardEvent) => { if (e.key === '/' && !/INPUT|TEXTAREA/.test((e.target as HTMLElement).tagName)) { e.preventDefault(); search.current?.focus() } }
    window.addEventListener('keydown', k); return () => window.removeEventListener('keydown', k)
  }, [])
  const list = useMemo(() => {
    const n = q.trim().toLowerCase()
    return (items ?? []).filter((i) => !n || `${i.company} ${i.role}`.toLowerCase().includes(n)).sort((a, b) => b.created_at.localeCompare(a.created_at))
  }, [items, q])
  const groups = useMemo(() => {
    const m = new Map<string, LibraryItem[]>()
    list.forEach((i) => m.set(i.company, [...(m.get(i.company) ?? []), i]))
    return [...m.entries()]
  }, [list])
  return (
    <>
      <div className="page-head">
        <h1>Your tailored resumes</h1>
        <p>Every finished version, filed by company, ready to open or send.</p>
      </div>
      <div className="lib-tools">
        <div className="search"><MagnifyingGlass size={16} aria-hidden /><input ref={search} type="search" value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search company or role" aria-label="Search the library" /><kbd>/</kbd></div>
        <Seg label="Group" value={view} onChange={setView} options={[{ value: 'company', label: 'By company' }, { value: 'recent', label: 'Newest first' }]} />
        {items && <span className="small muted num"><CountUp value={list.length} /> {list.length === 1 ? 'resume' : 'resumes'}</span>}
      </div>
      {err ? <div className="err-box">{err}</div> : !items ? (
        <div className="stack" style={{ gap: 14 }}>{[0, 1, 2].map((i) => <Skel key={i} h={88} r={18} />)}</div>
      ) : items.length === 0 ? (
        <Rise className="empty"><Books size={34} aria-hidden /><h2>Nothing in the library yet</h2><p>Finished resumes are filed here by company. Tailor your first one to start the shelf.</p><Link className="btn primary" to="/">Start a job</Link></Rise>
      ) : list.length === 0 ? (
        <div className="empty"><h2>No match for "{q}"</h2><p>Try a company name or part of a role title.</p><button className="btn" onClick={() => setQ('')}>Clear search</button></div>
      ) : view === 'company' ? (
        <div className="lib">
          <AnimatePresence initial={false}>
            {groups.map(([co, rows], gi) => (
              <motion.section key={co} layout="position" className="lib-group" initial={{ opacity: 0, y: 16 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} transition={{ duration: 0.5, ease: EASE, delay: Math.min(gi, 5) * 0.05 }} aria-label={co}>
                <header><h2>{co}</h2><span className="small muted">{rows.length} {rows.length === 1 ? 'version' : 'versions'}</span></header>
                <ul className="clean"><AnimatePresence initial={false}>{rows.map((r) => <Row key={r.id} it={r} />)}</AnimatePresence></ul>
              </motion.section>
            ))}
          </AnimatePresence>
        </div>
      ) : (
        <ul className="clean lib flat"><AnimatePresence initial={false}>{list.map((r) => (
          <motion.li key={r.id} layout="position" className="lib-flat" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
            <div className="lib-co">{r.company}</div>
            <ul className="clean"><Row it={r} /></ul>
          </motion.li>
        ))}</AnimatePresence></ul>
      )}
    </>
  )
}
