export const fmtNum = (n: number) => n.toLocaleString('en-US')
export const fmtDate = (iso: string) => new Date(iso).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
export const dur = (a: string | null, b: string | null) => (a && b ? `${Math.max(0, (new Date(b).getTime() - new Date(a).getTime()) / 1000).toFixed(1)}s` : '')
export const gapText = (g: unknown): string => {
  if (typeof g === 'string') return g
  if (g && typeof g === 'object') {
    const o = g as Record<string, unknown>
    const main = (o.requirement ?? o.label ?? o.text ?? o.name ?? JSON.stringify(o)) as string
    return String(main)
  }
  return String(g)
}
export const gapHint = (g: unknown): string => {
  if (g && typeof g === 'object') {
    const o = g as Record<string, unknown>
    return String(o.suggestion ?? o.hint ?? o.reason ?? '')
  }
  return ''
}
// Rough list prices per 1M tokens, used for an estimate only.
export const RATE_IN = 3, RATE_OUT = 15
export const cost = (tin: number, tout: number) => (tin * RATE_IN + tout * RATE_OUT) / 1e6
export const shortId = (id: string) => id.replace('demo-', '').slice(0, 8)
export const jobTitle = (j: { id: string; company?: string | null; role?: string | null }) => (j.company || j.role ? [j.role, j.company].filter(Boolean).join(' at ') : `Job ${shortId(j.id)}`)
export function ago(iso: string) {
  const s = (Date.now() - new Date(iso).getTime()) / 1000
  if (s < 60) return 'just now'
  if (s < 3600) return `${Math.floor(s / 60)} min ago`
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`
  if (s < 86400 * 7) return `${Math.floor(s / 86400)} d ago`
  return new Date(iso).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
}
export const errText = (e: string | { message: string; title?: string } | null | undefined) => (!e ? '' : typeof e === 'string' ? e : e.title ?? e.message)
