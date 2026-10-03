import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useNavigate } from 'react-router-dom'
import { api, mode } from './api'
import { friendly } from './lib/errors'
import { askedBefore, markAsked, osNotify, osPermission, osRequest, osSupported } from './lib/os'
import { errText } from './lib/util'
import { startRealtime } from './realtime'
import { isActive, isDone, isFailed, type ConnInfo, type JobEvent, type JobMeta, type LinkState, type Level, type Notif, type NotifAction, type RtEvent } from './types'

export interface Toast {
  id: number; key: string; level: Level; title: string; message?: string; hint?: string; actions?: NotifAction[]
  progress?: { done: number; total: number }; count: number; ms: number; rev: number; job_id?: string; step?: string; href?: string; code?: string
}
type ToastIn = { kind: 'ok' | 'err' | 'info'; text: string; href?: string }
interface Ctx {
  demo: boolean; jobs: JobMeta[]; loaded: boolean; refresh: () => void
  events: Record<string, JobEvent[]>; link: Record<string, LinkState>; watch: (id: string) => () => void
  toasts: Toast[]; toast: (t: ToastIn) => void; dismiss: (id: number) => void; runAction: (a: NotifAction, c: { job_id?: string; step?: string; toast?: number }) => Promise<void>
  conn: ConnInfo; notifs: Notif[]; unread: number; markRead: (upto?: number) => void; notifOpen: boolean; setNotifOpen: (v: boolean) => void
  askOs: boolean; answerOs: (yes: boolean) => void; offerOs: () => void
  palette: boolean; setPalette: (v: boolean) => void; help: boolean; setHelp: (v: boolean) => void
  tray: boolean; setTray: (v: boolean) => void
  budget: number; setBudget: (v: number) => void
  theme: string; setTheme: (t: string) => void
  proposed: number; setProposed: (n: number) => void
}
const C = createContext<Ctx>(null as never)
export const useApp = () => useContext(C)
const ls = (k: string, d: string) => { try { return localStorage.getItem(k) ?? d } catch { return d } }
const MAX_TOASTS = 3

export function Provider({ children }: { children: ReactNode }) {
  const nav = useNavigate()
  const [jobs, setJobs] = useState<JobMeta[]>([])
  const [loaded, setLoaded] = useState(false)
  const [events, setEvents] = useState<Record<string, JobEvent[]>>({})
  const [link, setLink] = useState<Record<string, LinkState>>({})
  const [toasts, setToasts] = useState<Toast[]>([])
  const [notifs, setNotifs] = useState<Notif[]>([])
  const [notifOpen, setNotifOpen] = useState(false)
  const [conn, setConn] = useState<ConnInfo>({ state: 'reconnecting', via: 'ws' })
  const [askOs, setAskOs] = useState(false)
  const [tray, setTray] = useState(false)
  const [proposed, setProposed] = useState(0)
  const [palette, setPalette] = useState(false)
  const [help, setHelp] = useState(false)
  const [budget, setBudgetS] = useState(Number(ls('budget', '5')))
  const [theme, setThemeS] = useState(ls('theme', 'system'))
  const status = useRef(new Map<string, string>())
  const watchers = useRef(new Map<string, { n: number; off: () => void }>())
  const auto = useRef(new Set<string>())
  const jobsRef = useRef<JobMeta[]>([])
  const connRef = useRef(conn)
  connRef.current = conn
  const tid = useRef(0)
  const deb = useRef(0)
  const timers = useRef(new Map<number, number>())

  useEffect(() => {
    const r = document.documentElement
    if (theme === 'system') r.removeAttribute('data-theme'); else r.setAttribute('data-theme', theme)
  }, [theme])
  const setTheme = (t: string) => {
    const go = () => { setThemeS(t); const r = document.documentElement; if (t === 'system') r.removeAttribute('data-theme'); else r.setAttribute('data-theme', t) }
    // Cross-fade the whole page when the browser supports view transitions; otherwise swap instantly.
    const vt = (document as Document & { startViewTransition?: (f: () => void) => unknown }).startViewTransition
    if (vt && !matchMedia('(prefers-reduced-motion: reduce)').matches) vt.call(document, go); else go()
    try { localStorage.setItem('theme', t) } catch { /* private mode */ }
  }
  const setBudget = (v: number) => { setBudgetS(v); try { localStorage.setItem('budget', String(v)) } catch { /* private mode */ } }

  const dismiss = useCallback((id: number) => { clearTimeout(timers.current.get(id)); timers.current.delete(id); setToasts((t) => t.filter((x) => x.id !== id)) }, [])
  /** Adds or updates a toast. Identical keys are merged (count goes up and the timer restarts); errors stay until dismissed. */
  const push = useCallback((t: Omit<Toast, 'id' | 'count' | 'rev'>) => {
    setToasts((list) => {
      const hit = list.find((x) => x.key === t.key)
      const id = hit?.id ?? ++tid.current
      clearTimeout(timers.current.get(id))
      if (t.ms > 0) timers.current.set(id, window.setTimeout(() => dismiss(id), t.ms))
      const next: Toast = { ...t, id, count: hit ? (t.progress ? 1 : hit.count + 1) : 1, rev: (hit?.rev ?? 0) + 1 }
      return hit ? list.map((x) => (x.id === id ? next : x)) : [...list, next].slice(-MAX_TOASTS)
    })
  }, [dismiss])
  const toast = useCallback((t: ToastIn) => {
    const level: Level = t.kind === 'ok' ? 'success' : t.kind === 'err' ? 'error' : 'info'
    push({ key: `${level}:${t.text}`, level, title: t.text, href: t.href, ms: level === 'error' ? 12000 : 6000 })
  }, [push])

  const refresh = useCallback(async () => {
    try {
      const list = await api.listJobs()
      for (const j of list) {
        const prev = status.current.get(j.id)
        // Only when the realtime channel is down do we announce completions from polling.
        if (prev && isActive(prev) && !isActive(j.status) && connRef.current.state !== 'live') {
          if (isDone(j.status)) toast({ kind: 'ok', text: 'Resume tailored and ready.', href: `/jobs/${j.id}` })
          else if (isFailed(j.status)) toast({ kind: 'err', text: `A job failed. ${errText(j.error) || 'Open it to retry.'}`, href: `/jobs/${j.id}` })
        }
        status.current.set(j.id, j.status)
      }
      jobsRef.current = list
      setJobs(list)
    } catch { /* keep the last list; the polling loop tries again */ } finally { setLoaded(true) }
  }, [toast])

  const watch = useCallback((id: string) => {
    let w = watchers.current.get(id)
    if (!w) {
      const off = api.subscribe(
        id,
        (e) => {
          setEvents((m) => (m[id]?.some((x) => x.seq === e.seq) ? m : { ...m, [id]: [...(m[id] ?? []), e] }))
          clearTimeout(deb.current); deb.current = window.setTimeout(refresh, 200)
        },
        (s) => setLink((m) => ({ ...m, [id]: s })),
        () => { const s = jobsRef.current.find((j) => j.id === id)?.status; return !!s && !isActive(s) },
      )
      w = { n: 0, off }
      watchers.current.set(id, w)
    }
    w.n++
    return () => {
      const x = watchers.current.get(id)
      if (x && --x.n <= 0) { x.off(); watchers.current.delete(id) }
    }
  }, [refresh])

  useEffect(() => {
    refresh()
    const anyActive = () => jobsRef.current.some((j) => isActive(j.status))
    let t = 0
    const loop = () => { refresh(); t = window.setTimeout(loop, anyActive() ? 2500 : 12000) }
    t = window.setTimeout(loop, 2500)
    return () => clearTimeout(t)
  }, [refresh])

  // Keep a stream open for every active job so the tray stays live on any page.
  useEffect(() => {
    const releases = new Map<string, () => void>()
    jobs.forEach((j) => {
      if (isActive(j.status) && !auto.current.has(j.id)) { auto.current.add(j.id); releases.set(j.id, watch(j.id)) }
    })
    releases.forEach((rel, id) => {
      const stop = window.setInterval(() => {
        const s = jobsRef.current.find((x) => x.id === id)?.status
        if (!s || !isActive(s)) { rel(); auto.current.delete(id); clearInterval(stop) }
      }, 1500)
    })
  }, [jobs, watch])

  // ---- realtime: one connection feeds toasts, the notification center and OS notifications ----
  const ingest = useCallback((ev: RtEvent) => {
    const f = ev.code ? friendly(ev.code, ev, ev.job_id, ev.step) : null
    const title = f?.known ? f.title : ev.title, message = f?.known ? f.message : ev.message, hint = f?.known ? f.hint : ev.hint
    const actions = ev.actions?.length ? ev.actions : f?.actions
    if (ev.kind === 'step') {
      const key = `p:${ev.job_id}:${ev.step}`
      if (ev.progress && ev.progress.done < ev.progress.total) push({ key, level: 'info', title, message, progress: ev.progress, ms: 0, job_id: ev.job_id, step: ev.step })
      else setToasts((l) => { const h = l.find((x) => x.key === key); if (h) { clearTimeout(timers.current.get(h.id)); return l.filter((x) => x.id !== h.id) } return l })
      return
    }
    if (ev.code === 'playbook.proposed') void api.getPlaybook().then((p) => setProposed(p.groups.proposed.length), () => {})
    const critical = f?.critical
    push({ key: `${ev.level}:${ev.code ?? title}:${ev.job_id ?? ''}`, level: ev.level, title, message, hint, actions, code: ev.code, job_id: ev.job_id, step: ev.step, ms: ev.level === 'error' || critical ? 0 : ev.level === 'warn' ? 12000 : 7000 })
    setNotifs((l) => (l.some((n) => n.seq === ev.seq) ? l : [...l, { seq: ev.seq, level: ev.level, title, message, hint, code: ev.code, job_id: ev.job_id, step: ev.step, retryable: ev.retryable, actions, ts: ev.ts, read: false }]))
    if (ev.kind === 'job_status') {
      setToasts((l) => l.filter((x) => !x.key.startsWith(`p:${ev.job_id}:`)))
      clearTimeout(deb.current); deb.current = window.setTimeout(refresh, 150)
      if (document.hidden && (ev.level === 'success' || ev.level === 'error' || ev.level === 'warn')) osNotify(ev.level === 'success' ? 'Resume ready' : title, ev.level === 'success' ? message : message, () => ev.job_id && nav(`/jobs/${ev.job_id}`))
    }
  }, [push, refresh, nav])
  useEffect(() => startRealtime({ onEvent: ingest, onState: setConn, onPoll: () => { void refresh(); void loadNotifs() } }), [ingest]) // eslint-disable-line react-hooks/exhaustive-deps
  const loadNotifs = useCallback(async () => {
    try { const n = await api.getNotifications(); setNotifs((cur) => { const m = new Map(n.map((x) => [x.seq, x])); cur.forEach((x) => { if (!m.has(x.seq)) m.set(x.seq, x) }); return [...m.values()].sort((a, b) => a.seq - b.seq) }) } catch { /* the center stays as it is */ }
  }, [])
  useEffect(() => { void loadNotifs(); api.getPlaybook().then((p) => setProposed(p.groups.proposed.length), () => {}) }, [loadNotifs])
  const unread = notifs.filter((n) => !n.read).length
  const markRead = useCallback((upto?: number) => {
    const top = upto ?? Math.max(0, ...notifs.map((n) => n.seq))
    setNotifs((l) => l.map((n) => (n.seq <= top ? { ...n, read: true } : n)))
    void api.markNotificationsRead(top).catch(() => {})
  }, [notifs])

  const runAction = useCallback(async (a: NotifAction, c: { job_id?: string; step?: string; toast?: number }) => {
    if (c.toast !== undefined && a.action !== 'retry') dismiss(c.toast)
    if (a.action === 'dismiss') return
    if (a.action === 'settings') { nav('/settings'); return }
    if (a.action === 'open') { nav(a.target?.startsWith('/') ? a.target : c.job_id ? `/jobs/${c.job_id}` : '/jobs'); return }
    if (a.action === 'retry' && c.job_id) {
      try { await api.retry(c.job_id, a.target ?? c.step); if (c.toast !== undefined) dismiss(c.toast); void refresh(); toast({ kind: 'info', text: 'Retrying from that step.' }) }
      catch (e) { toast({ kind: 'err', text: e instanceof Error ? e.message : 'Retry did not start.' }) }
    }
  }, [nav, dismiss, refresh, toast])

  // Ask for OS notification permission lazily, once, after the first job starts.
  const offerOs = useCallback(() => { if (osSupported() && osPermission() === 'default' && !askedBefore()) setAskOs(true) }, [])
  const answerOs = useCallback((yes: boolean) => { markAsked(); setAskOs(false); if (yes) void osRequest() }, [])

  const v = useMemo(() => ({
    demo: mode.demo, jobs, loaded, refresh, events, link, watch, toasts, toast, dismiss, runAction, conn, notifs, unread, markRead, notifOpen, setNotifOpen, askOs, answerOs, offerOs,
    tray, setTray, proposed, setProposed, budget, setBudget, theme, setTheme, palette, setPalette, help, setHelp,
  }), [jobs, loaded, refresh, events, link, watch, toasts, toast, dismiss, runAction, conn, notifs, unread, markRead, notifOpen, askOs, answerOs, offerOs, tray, proposed, budget, theme, palette, help]) // eslint-disable-line react-hooks/exhaustive-deps
  return <C.Provider value={v}>{children}</C.Provider>
}
