import { Books, BookBookmark, ChartBar, Flask, GraduationCap, Kanban, UserCircle, ShieldCheck, FilePlus, Gear, ListChecks, MagnifyingGlass, Monitor, Moon, Pulse, Sun } from '@phosphor-icons/react'
import { AnimatePresence, motion } from 'motion/react'
import { lazy, Suspense, useEffect, useRef } from 'react'
import { Link, NavLink, Route, Routes, useLocation, useNavigate } from 'react-router-dom'
import { mode } from './api'
import { CompanionBar } from './companion'
import { BellButton, NotifCenter } from './components/Bell'
import { ConnDot } from './components/ConnDot'
import { NotifPrompt } from './components/NotifPrompt'
import { Overlays } from './components/Palette'
import { EASE, Skel } from './components/Motion'
import { ZoneLegend } from './components/Steps'
import { Toasts } from './components/Toasts'
import { Tray } from './components/Tray'
import { useApp } from './store'
import { isActive } from './types'

const Intake = lazy(() => import('./pages/Intake'))
const Library = lazy(() => import('./pages/Library'))
const Jobs = lazy(() => import('./pages/Jobs'))
const JobLayout = lazy(() => import('./pages/JobDetail'))
const JobOverview = lazy(() => import('./pages/JobDetail').then((m) => ({ default: m.JobOverview })))
const Review = lazy(() => import('./pages/Review'))
const Match = lazy(() => import('./pages/Match'))
const Privacy = lazy(() => import('./pages/Privacy'))
const Analytics = lazy(() => import('./pages/Analytics'))
const Lab = lazy(() => import('./pages/Lab'))
const Playbook = lazy(() => import('./pages/Playbook'))
const Learning = lazy(() => import('./pages/Learning'))
const Applications = lazy(() => import('./pages/Applications'))
const Profile = lazy(() => import('./pages/Profile'))
const Pii = lazy(() => import('./pages/Pii'))
const Settings = lazy(() => import('./pages/Settings'))

const NAV = [
  { to: '/', label: 'Intake', icon: FilePlus, end: true },
  { to: '/library', label: 'Library', icon: Books },
  { to: '/jobs', label: 'Jobs', icon: ListChecks },
  { to: '/playbook', label: 'Playbook', icon: BookBookmark },
  { to: '/lab', label: 'Lab', icon: Flask },
  { to: '/applications', label: 'Applications', short: 'Apps', icon: Kanban },
  { to: '/profile', label: 'Profile', icon: UserCircle },
  { to: '/pii', label: 'PII ledger', short: 'PII', icon: ShieldCheck },
  { to: '/analytics', label: 'Analytics', icon: ChartBar },
  { to: '/settings', label: 'Settings', icon: Gear },
  { to: '/learning', label: 'Learning', icon: GraduationCap },
]
// Paired phone: only what the desktop allows remotely (the rest would answer 403).
const ORDER = ['/', '/jobs', '/library', '/applications', '/playbook', '/learning']
const navFor = (co: boolean) => (co ? ORDER.map((to) => ({ ...NAV.find((n) => n.to === to)!, ...(to === '/' && { label: 'New' }) })) : NAV.filter((n) => n.to !== '/learning'))

const Mark = () => (
  <svg className="mark" width="26" height="26" viewBox="0 0 26 26" aria-hidden>
    <rect x="1" y="1" width="24" height="24" rx="7" className="m-bg" />
    <rect x="6" y="7" width="9" height="2.6" rx="1.3" className="m-a" /><rect x="6" y="12" width="14" height="4" rx="1.4" className="m-b" /><rect x="6" y="17.4" width="11" height="2.6" rx="1.3" className="m-a" />
  </svg>
)

function PageSkeleton() {
  return <div className="stack" style={{ gap: 18, paddingTop: 8 }}><Skel h={56} w="52%" r={12} /><Skel h={18} w="38%" /><Skel h={300} r={18} /></div>
}

function useShortcuts() {
  const nav = useNavigate()
  const { setPalette, setHelp, theme, setTheme, tray, setTray } = useApp()
  const chord = useRef(0)
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement
      const typing = /INPUT|TEXTAREA|SELECT/.test(el.tagName) || el.isContentEditable
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); setPalette(true); return }
      if (typing || e.metaKey || e.ctrlKey || e.altKey) return
      const now = Date.now()
      if (chord.current && now - chord.current < 1200) {
        const to = ({ i: '/', l: '/library', j: '/jobs', p: '/pii', a: '/analytics', s: '/settings', m: '/lab', b: '/playbook', e: '/learning', k: '/applications', f: '/profile' } as Record<string, string>)[e.key]
        chord.current = 0
        if (to) { e.preventDefault(); nav(to); return }
      }
      if (e.key === 'g') chord.current = now
      else if (e.key === 'n') nav('/')
      else if (e.key === 't') setTheme(theme === 'dark' ? 'light' : 'dark')
      else if (e.key === '?') setHelp(true)
      else if (e.key === 'Escape' && tray) setTray(false)
    }
    window.addEventListener('keydown', k)
    return () => window.removeEventListener('keydown', k)
  }, [nav, setPalette, setHelp, setTheme, theme, tray, setTray])
}

export default function App() {
  const { demo, jobs, proposed, setTray, tray, theme, setTheme, setPalette, notifOpen } = useApp()
  const loc = useLocation()
  const co = !!mode.companion, NAVV = navFor(co)
  useShortcuts()
  const top = loc.pathname.split('/')[1] || 'intake'
  useEffect(() => { window.scrollTo(0, 0) }, [top])
  const running = jobs.filter((j) => isActive(j.status)).length
  const next = theme === 'system' ? 'light' : theme === 'light' ? 'dark' : 'system'
  const ThemeIcon = theme === 'system' ? Monitor : theme === 'light' ? Sun : Moon
  const mac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform)
  return (
    <div className="app">
      <a href="#main" className="sr skip">Skip to content</a>
      <header className="top">
        {demo && (
          <div className="demo-bar" role="status">
            <b>Demo mode</b><span>Sample data for Jane Doe. No backend is connected and nothing is sent anywhere.</span>
            <button onClick={() => { const u = new URL(location.href); u.searchParams.delete('demo'); location.href = u.toString() }}>Try the backend</button>
          </div>
        )}
        <div className="top-in">
          <Link to="/" className="brand" aria-label="Veil, home"><Mark /><span>Veil</span></Link>
          <nav className="nav" aria-label="Main">
            {NAVV.map((n) => (
              <NavLink key={n.to} to={n.to} end={n.end}>
                {({ isActive: on }) => <>{on && <motion.span layoutId="navpill" className="nav-pill" transition={{ type: 'spring', stiffness: 520, damping: 40 }} />}<span className="nav-l">{n.label}</span>{n.to === '/playbook' && proposed > 0 && <span className="nav-badge" aria-label={`${proposed} suggested`}>{proposed}</span>}</>}
              </NavLink>
            ))}
          </nav>
          <div className="top-r">
            <button className="kbar" onClick={() => setPalette(true)} aria-label="Open command palette"><MagnifyingGlass size={16} aria-hidden /><span>Search or jump to</span><kbd>{mac ? '⌘' : 'Ctrl'} K</kbd></button>
            <button className="icon-btn kmini" onClick={() => setPalette(true)} aria-label="Open command palette"><MagnifyingGlass size={18} /></button>
            <ConnDot />
            <BellButton />
            <button className="icon-btn" onClick={() => setTheme(next)} aria-label={`Theme: ${theme}. Switch to ${next}.`} title={`Theme: ${theme}`}><ThemeIcon size={18} /></button>
            <button className="icon-btn act" onClick={() => setTray(!tray)} aria-label={`Activity, ${running} running`} aria-haspopup="dialog" aria-expanded={tray}>
              <Pulse size={18} />
              <AnimatePresence>{running > 0 && <motion.span key="n" className="dot-count" initial={{ scale: 0 }} animate={{ scale: 1 }} exit={{ scale: 0 }} transition={{ type: 'spring', stiffness: 600, damping: 22 }}>{running}</motion.span>}</AnimatePresence>
            </button>
          </div>
        </div>
      </header>
      {co && <CompanionBar />}
      <AnimatePresence mode="wait" initial={false}>
        <motion.main key={top} id="main" className="page" tabIndex={-1} initial={{ opacity: 0, y: 14 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -8, transition: { duration: 0.14 } }} transition={{ duration: 0.38, ease: EASE }}>
          <Suspense fallback={<PageSkeleton />}>
            <Routes location={loc}>
              <Route path="/" element={<Intake />} />
              <Route path="/library" element={<Library />} />
              <Route path="/jobs" element={<Jobs />} />
              <Route path="/jobs/:id" element={<JobLayout />}>
                <Route index element={<JobOverview />} />
                <Route path="match" element={<Match />} />
                <Route path="review" element={<Review />} />
                <Route path="privacy" element={<Privacy />} />
              </Route>
              <Route path="/playbook" element={<Playbook />} />
              <Route path="/learning" element={<Learning />} />
              {!co && <>
              <Route path="/lab" element={<Lab />} />
              <Route path="/applications" element={<Applications />} />
              <Route path="/profile" element={<Profile />} />
              <Route path="/pii" element={<Pii />} />
              <Route path="/analytics" element={<Analytics />} />
              <Route path="/settings" element={<Settings />} />
              </>}
              <Route path="*" element={<div className="empty"><h2>Page not found</h2><p>That address does not lead anywhere.</p><Link className="btn" to="/">Back to intake</Link></div>} />
            </Routes>
          </Suspense>
        </motion.main>
      </AnimatePresence>
      <footer className="foot">
        <p>Your resume stays on this machine. These are the only places anything else can go.</p>
        <ZoneLegend />
      </footer>
      <nav className={`tabbar ${co ? 'co' : ''}`} aria-label="Main (mobile)">
        {NAVV.map((n) => (
          <NavLink key={n.to} to={n.to} end={n.end}>
            {({ isActive: on }) => <>{on && <motion.span layoutId="tabpill" className="tab-pill" transition={{ type: 'spring', stiffness: 520, damping: 40 }} />}<n.icon size={21} /><span>{'short' in n ? n.short : n.label}</span>{n.to === '/playbook' && proposed > 0 && <span className="nav-badge">{proposed}</span>}</>}
          </NavLink>
        ))}
      </nav>
      <AnimatePresence>{tray && <Tray key="tray" />}</AnimatePresence>
      <AnimatePresence>{notifOpen && <NotifCenter key="nc" />}</AnimatePresence>
      <Toasts />
      <NotifPrompt />
      <Overlays />
    </div>
  )
}
