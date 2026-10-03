import { mode } from './api'
import { demoApi } from './demo'
import type { ConnInfo, RtEvent } from './types'

/** Minimal shape shared by WebSocket and the demo's fake socket. */
export interface Sock { onopen: (() => void) | null; onmessage: ((e: { data: string }) => void) | null; onclose: (() => void) | null; onerror: (() => void) | null; send(d: string): void; close(): void }
interface Src { onopen: (() => void) | null; onmessage: ((e: { data: string }) => void) | null; onerror: (() => void) | null; close(): void }

const KEY = 'veil.rt.seq'
const HEARTBEAT_MS = 35000
const stored = () => { try { return Number(sessionStorage.getItem(KEY) || 0) } catch { return 0 } }
const persist = (n: number) => { try { sessionStorage.setItem(KEY, String(n)) } catch { /* private mode */ } }

/**
 * One connection for every live event. WebSocket first; after 3 failed attempts it falls back to SSE, and if that fails
 * too it reports 'polling' so the app can poll over REST. Events are de-duplicated and delivered in seq order, and the
 * last delivered seq is sent as ?since= on every reconnect so the server can replay what was missed.
 */
export function startRealtime(h: { onEvent: (e: RtEvent) => void; onState: (s: ConnInfo) => void; onPoll: () => void }) {
  let last = mode.demo ? 0 : stored()
  let ws: Sock | null = null, es: Src | null = null
  // The phone's local proxy does not forward WebSockets, so companion mode starts (and stays) on SSE.
  const first: ConnInfo['via'] = mode.companion ? 'sse' : 'ws'
  let via: ConnInfo['via'] = first, fails = 0, closed = false
  let retry = 0, hb = 0, flush = 0, ackT = 0, poll = 0, wsAgain = 0
  let pending: RtEvent[] = []

  const state = (s: ConnInfo['state'], retryAt?: number) => h.onState({ state: s, via, retryAt })
  const offline = () => typeof navigator !== 'undefined' && navigator.onLine === false

  const ack = () => { clearTimeout(ackT); ackT = window.setTimeout(() => { try { ws?.send(JSON.stringify({ t: 'ack', seq: last })) } catch { /* socket closing */ } }, 400) }
  const doFlush = () => {
    flush = 0
    pending.sort((a, b) => a.seq - b.seq)
    const q = pending; pending = []
    for (const e of q) if (e.seq > last) { last = e.seq; h.onEvent(e) }
    if (!mode.demo) persist(last)
    ack()
  }
  const push = (e: RtEvent) => {
    // The Rust server sends ts as epoch milliseconds; the UI treats it as an ISO string.
    if (typeof e.ts === 'number') e = { ...e, ts: new Date(e.ts).toISOString() }
    if (e.seq <= last || pending.some((p) => p.seq === e.seq)) return
    pending.push(e)
    if (!flush) flush = window.setTimeout(doFlush, 25)
  }
  const beat = () => { clearTimeout(hb); hb = window.setTimeout(() => { drop() }, HEARTBEAT_MS) }
  const teardown = () => {
    clearTimeout(hb); clearTimeout(retry)
    if (ws) { ws.onopen = ws.onmessage = ws.onclose = ws.onerror = null; try { ws.close() } catch { /* already closed */ } ws = null }
    if (es) { es.onopen = es.onmessage = es.onerror = null; es.close(); es = null }
  }
  const schedule = () => {
    if (closed) return
    const base = Math.min(30000, 1000 * 2 ** Math.max(0, fails - 1))
    const delay = Math.round(base * (0.6 + Math.random() * 0.4))
    state('reconnecting', Date.now() + delay)
    retry = window.setTimeout(connect, delay)
  }
  // Called when the active transport dies.
  function drop(): void {
    teardown()
    if (closed) return
    if (offline()) return state('offline')
    fails++
    if (fails >= 3) {
      if (via === 'ws') { via = 'sse'; fails = 0; return connect() }
      if (via === 'sse') { via = 'poll'; fails = 0; return startPolling() }
    }
    schedule()
  }
  function startPolling() {
    state('polling')
    h.onPoll()
    poll = window.setInterval(h.onPoll, 10000)
    wsAgain = window.setTimeout(() => { clearInterval(poll); via = first; fails = 0; connect() }, 45000)
  }
  function connect(): void {
    if (closed) return
    teardown(); clearInterval(poll); clearTimeout(wsAgain)
    if (offline()) return state('offline')
    if (via === 'poll') return startPolling()
    if (via === 'ws') {
      const url = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/ws?since=${last}`
      try { ws = mode.demo ? demoApi.openSocket(last) : (new WebSocket(url) as unknown as Sock) } catch { return drop() }
      ws.onopen = () => { beat() }
      ws.onmessage = (m) => {
        beat()
        let f: { t: string } & Partial<RtEvent>
        try { f = JSON.parse(m.data) } catch { return }
        if (f.t === 'hello') { fails = 0; state('live') }
        else if (f.t === 'ping') ws?.send(JSON.stringify({ t: 'pong' }))
        else if (f.t === 'event') push(f as unknown as RtEvent)
      }
      ws.onclose = ws.onerror = () => drop()
    } else {
      try { es = mode.demo ? demoApi.openEvents(last) : (new EventSource(`/api/events?since=${last}`) as unknown as Src) } catch { return drop() }
      es.onopen = () => { fails = 0; beat(); state('live') }
      es.onmessage = (m) => { beat(); try { push(JSON.parse(m.data) as RtEvent) } catch { /* ignore */ } }
      es.onerror = () => drop()
    }
  }

  const vis = () => { if (!document.hidden && !closed && !ws?.onopen && !es && via !== 'poll') { /* connected or connecting */ } if (!document.hidden && !closed) { clearTimeout(retry); if (!ws && !es && via !== 'poll') { fails = 0; connect() } } }
  const on = () => { fails = 0; via = first; connect() }
  const off = () => { teardown(); clearInterval(poll); clearTimeout(wsAgain); state('offline') }
  document.addEventListener('visibilitychange', vis)
  window.addEventListener('online', on)
  window.addEventListener('offline', off)
  connect()
  return () => {
    closed = true; teardown(); clearInterval(poll); clearTimeout(wsAgain); clearTimeout(flush); clearTimeout(ackT)
    document.removeEventListener('visibilitychange', vis); window.removeEventListener('online', on); window.removeEventListener('offline', off)
  }
}
