import { useEffect, useState } from 'react'
import { useApp } from '../store'

export function ConnDot() {
  const { conn } = useApp()
  const [, tick] = useState(0)
  useEffect(() => { if (conn.state !== 'reconnecting') return; const t = setInterval(() => tick((n) => n + 1), 500); return () => clearInterval(t) }, [conn])
  const secs = conn.retryAt ? Math.max(0, Math.ceil((conn.retryAt - Date.now()) / 1000)) : 0
  const label = conn.state === 'live' ? (conn.via === 'sse' ? 'Live (fallback stream)' : 'Live')
    : conn.state === 'reconnecting' ? (secs > 0 ? `Reconnecting in ${secs}s…` : 'Reconnecting…')
      : conn.state === 'offline' ? 'Offline. Will reconnect when the network returns' : 'Updating by polling every 10s'
  return (
    <span className={`conn-dot ${conn.state}`} role="status" tabIndex={0} aria-label={`Connection: ${label}`} data-tip={label}>
      <i aria-hidden />
    </span>
  )
}
