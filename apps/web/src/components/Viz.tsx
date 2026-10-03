import { motion, useReducedMotion } from 'motion/react'
import { useMemo } from 'react'
import { EASE } from './Motion'

/** Tiny line chart: one series, 2px line, end marker with a surface ring, no axes. The caption carries the numbers. */
export function Spark({ points, w = 140, h = 36, label, color = 'var(--c1)' }: { points: number[]; w?: number; h?: number; label: string; color?: string }) {
  const reduce = useReducedMotion()
  const d = useMemo(() => {
    const lo = Math.min(...points), hi = Math.max(...points), span = hi - lo || 1
    return points.map((v, i) => [4 + (i * (w - 8)) / Math.max(1, points.length - 1), h - 5 - ((v - lo) / span) * (h - 10)] as const)
  }, [points, w, h])
  if (points.length < 2) return <span className="small muted">Too early for a trend</span>
  const [lx, ly] = d[d.length - 1]
  return (
    <svg className="spark" width={w} height={h} viewBox={`0 0 ${w} ${h}`} role="img" aria-label={label}>
      <title>{label}</title>
      <motion.polyline points={d.map((p) => p.join(',')).join(' ')} fill="none" stroke={color} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" initial={reduce ? false : { pathLength: 0 }} animate={{ pathLength: 1 }} transition={{ duration: 0.9, ease: EASE }} />
      <circle cx={lx} cy={ly} r={4} fill={color} stroke="var(--panel)" strokeWidth={2} />
    </svg>
  )
}

/** Horizontal bars, thin and rounded at the data end, direct value labels, recessive baseline. Negative values extend left in the warning hue. */
export function HBars({ rows, max, fmt = (v: number) => v.toFixed(2), label }: { rows: { name: string; value: number; sub?: string }[]; max?: number; fmt?: (v: number) => string; label: string }) {
  const m = max ?? Math.max(...rows.map((r) => Math.abs(r.value)), 1e-9)
  return (
    <ul className="hbars clean" aria-label={label}>
      {rows.map((r, i) => (
        <li key={r.name} title={`${r.name}: ${fmt(r.value)}`}>
          <span className="hb-n">{r.name}</span>
          <span className="hb-t"><motion.i style={{ width: `${Math.min(100, (Math.abs(r.value) / m) * 100)}%`, background: r.value < 0 ? 'var(--ai)' : 'var(--c1)' }} initial={{ scaleX: 0 }} animate={{ scaleX: 1 }} transition={{ duration: 0.7, ease: EASE, delay: i * 0.04 }} /></span>
          <span className="hb-v num">{r.sub ?? fmt(r.value)}</span>
        </li>
      ))}
    </ul>
  )
}

export const human = (s: string) => s.replace(/_/g, ' ')
