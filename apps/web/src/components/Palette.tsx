import { ArrowBendDownLeft, Books, ChartBar, FilePlus, Gear, ListChecks, MagnifyingGlass, Moon, Pulse, Sun, Monitor, Keyboard, Briefcase, ShieldCheck, Flask, Kanban, UserCircle, BookBookmark, TrendUp } from '@phosphor-icons/react'
import type { Icon } from '@phosphor-icons/react'
import { AnimatePresence, motion } from 'motion/react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { mode } from '../api'
import { CO_HIDDEN } from '../companion'
import { useApp } from '../store'
import { jobTitle } from '../lib/util'
import { SPRING } from './Motion'

interface Cmd { id: string; label: string; group: string; hint?: string; Icon: Icon; run: () => void; kw?: string }

export const SHORTCUTS: [string, string][] = [
  ['Ctrl/Cmd K', 'Open the command palette'], ['g then i', 'Go to Intake'], ['g then l', 'Go to Library'], ['g then j', 'Go to Jobs'],
  ['g then p', 'Go to PII ledger'], ['g then a', 'Go to Analytics'], ['g then s', 'Go to Settings'], ['g then m', 'Go to ML Lab'], ['g then b', 'Go to Playbook'], ['g then e', 'Go to Learning'], ['g then k', 'Go to Applications'], ['g then f', 'Go to Profile'], ['n', 'New job'], ['/', 'Search the library'], ['t', 'Switch theme'], ['?', 'Show this list'], ['Esc', 'Close any panel'],
]

export function Palette() {
  const { jobs, setPalette, setTray, setTheme, theme, setHelp, setNotifOpen } = useApp()
  const nav = useNavigate()
  const [q, setQ] = useState('')
  const [i, setI] = useState(0)
  const input = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLUListElement>(null)
  const go = (to: string) => () => nav(to)
  const cmds = useMemo<Cmd[]>(() => [
    { id: 'intake', label: 'New job (Intake)', group: 'Go to', hint: 'g i', Icon: FilePlus, run: go('/'), kw: 'new tailor start' },
    { id: 'library', label: 'Library', group: 'Go to', hint: 'g l', Icon: Books, run: go('/library'), kw: 'resumes files drive' },
    { id: 'jobs', label: 'Jobs', group: 'Go to', hint: 'g j', Icon: ListChecks, run: go('/jobs') },
    { id: 'playbook', label: 'Playbook (rules)', group: 'Go to', hint: 'g b', Icon: BookBookmark, run: go('/playbook'), kw: 'rules learned approve suggestions regression versions' },
    { id: 'learning', label: 'Learning curve', group: 'Go to', hint: 'g e', Icon: TrendUp, run: go('/learning'), kw: 'corrections per job approval progress' },
    { id: 'lab', label: 'ML Lab', group: 'Go to', hint: 'g m', Icon: Flask, run: go('/lab'), kw: 'models learning recommendations learn next skills retrain' },
    { id: 'applications', label: 'Applications', group: 'Go to', hint: 'g k', Icon: Kanban, run: go('/applications'), kw: 'track tracker funnel interview offer' },
    { id: 'profile', label: 'Your profile model', group: 'Go to', hint: 'g f', Icon: UserCircle, run: go('/profile'), kw: 'facts skills timeline proficiency ground truth' },
    { id: 'pii', label: 'PII ledger', group: 'Go to', hint: 'g p', Icon: ShieldCheck, run: go('/pii'), kw: 'personal data privacy mask reveal redact' },
    { id: 'analytics', label: 'Analytics', group: 'Go to', hint: 'g a', Icon: ChartBar, run: go('/analytics'), kw: 'spend tokens' },
    { id: 'settings', label: 'Settings', group: 'Go to', hint: 'g s', Icon: Gear, run: go('/settings'), kw: 'keys drive budget' },
    ...jobs.slice(0, 6).map((j): Cmd => ({ id: j.id, label: jobTitle(j), group: 'Recent jobs', Icon: Briefcase, run: go(`/jobs/${j.id}`), kw: `${j.status} ${j.provider}` })),
    { id: 'notifs', label: 'Open notifications', group: 'Actions', Icon: Pulse, run: () => setNotifOpen(true), kw: 'bell alerts messages' },
    { id: 'activity', label: 'Open activity', group: 'Actions', Icon: Pulse, run: () => setTray(true) },
    { id: 'theme', label: `Theme: ${theme === 'dark' ? 'switch to light' : 'switch to dark'}`, group: 'Actions', hint: 't', Icon: theme === 'dark' ? Sun : Moon, run: () => setTheme(theme === 'dark' ? 'light' : 'dark'), kw: 'dark light mode' },
    { id: 'system', label: 'Theme: follow system', group: 'Actions', Icon: Monitor, run: () => setTheme('system') },
    { id: 'help', label: 'Keyboard shortcuts', group: 'Actions', hint: '?', Icon: Keyboard, run: () => setHelp(true) },
  ].filter((c) => !mode.companion || !CO_HIDDEN.has(c.id)), [jobs, theme]) // eslint-disable-line react-hooks/exhaustive-deps
  const list = useMemo(() => {
    const n = q.trim().toLowerCase()
    return n ? cmds.filter((c) => `${c.label} ${c.group} ${c.kw ?? ''}`.toLowerCase().includes(n)) : cmds
  }, [q, cmds])
  useEffect(() => { input.current?.focus() }, [])
  useEffect(() => { setI(0) }, [q])
  useEffect(() => { listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' }) }, [i])
  const pick = (c?: Cmd) => { if (!c) return; setPalette(false); c.run() }
  const key = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') { e.preventDefault(); setI((x) => Math.min(list.length - 1, x + 1)) }
    else if (e.key === 'ArrowUp') { e.preventDefault(); setI((x) => Math.max(0, x - 1)) }
    else if (e.key === 'Enter') { e.preventDefault(); pick(list[i]) }
    else if (e.key === 'Escape') setPalette(false)
    else if (e.key === 'Tab') e.preventDefault()
  }
  let last = ''
  return (
    <motion.div className="modal-wrap" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.18 }} onMouseDown={(e) => { if (e.target === e.currentTarget) setPalette(false) }}>
      <motion.div className="palette" role="dialog" aria-modal="true" aria-label="Command palette" onKeyDown={key}
        initial={{ opacity: 0, y: -16, scale: 0.96 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: -8, scale: 0.98 }} transition={SPRING}>
        <div className="pal-in"><MagnifyingGlass size={18} aria-hidden /><input ref={input} value={q} onChange={(e) => setQ(e.target.value)} placeholder="Jump to a page, a job, or an action" role="combobox" aria-expanded="true" aria-controls="pal-list" aria-activedescendant={list[i] ? `pc-${list[i].id}` : undefined} aria-label="Command" /><kbd>Esc</kbd></div>
        <ul id="pal-list" ref={listRef} role="listbox" className="clean pal-list">
          {list.length === 0 && <li className="pal-empty">Nothing matches "{q}".</li>}
          {list.map((c, n) => {
            const head = c.group !== last; last = c.group
            return (
              <li key={c.id} role="presentation">
                {head && <div className="pal-g">{c.group}</div>}
                <div id={`pc-${c.id}`} role="option" aria-selected={n === i} className={`pal-i ${n === i ? 'on' : ''}`} onMouseMove={() => setI(n)} onClick={() => pick(c)}>
                  {n === i && <motion.span layoutId="palsel" className="pal-bg" transition={{ type: 'spring', stiffness: 600, damping: 44 }} />}
                  <c.Icon size={17} /><span className="grow">{c.label}</span>{c.hint ? <kbd>{c.hint}</kbd> : n === i && <ArrowBendDownLeft size={14} />}
                </div>
              </li>
            )
          })}
        </ul>
      </motion.div>
    </motion.div>
  )
}

export function Shortcuts() {
  const { setHelp } = useApp()
  useEffect(() => { const k = (e: KeyboardEvent) => { if (e.key === 'Escape') setHelp(false) }; window.addEventListener('keydown', k); return () => window.removeEventListener('keydown', k) }, [setHelp])
  return (
    <motion.div className="modal-wrap" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} onMouseDown={(e) => { if (e.target === e.currentTarget) setHelp(false) }}>
      <motion.div className="palette help" role="dialog" aria-modal="true" aria-label="Keyboard shortcuts" initial={{ opacity: 0, y: 12, scale: 0.97 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, scale: 0.98 }} transition={SPRING}>
        <div className="row between"><h2>Keyboard shortcuts</h2><button className="icon-btn" aria-label="Close" onClick={() => setHelp(false)} autoFocus>Esc</button></div>
        <dl className="sc">{SHORTCUTS.map(([k, d]) => <div key={k}><dt><kbd>{k}</kbd></dt><dd>{d}</dd></div>)}</dl>
      </motion.div>
    </motion.div>
  )
}

export const Overlays = () => {
  const { palette, help } = useApp()
  return <AnimatePresence>{palette && <Palette key="p" />}{help && <Shortcuts key="h" />}</AnimatePresence>
}
