import { animate, motion, useInView, useReducedMotion } from 'motion/react'
import { useLayoutEffect, useRef, type CSSProperties, type ReactNode } from 'react'

export const EASE = [0.22, 1, 0.36, 1] as const
export const SPRING = { type: 'spring', stiffness: 420, damping: 34, mass: 0.8 } as const
export const SOFT = { type: 'spring', stiffness: 260, damping: 28 } as const

/** Number that counts up the first time it scrolls into view and re-tweens when the value changes. */
export function CountUp({ value, decimals = 0, prefix = '', suffix = '', className }: { value: number; decimals?: number; prefix?: string; suffix?: string; className?: string }) {
  const ref = useRef<HTMLSpanElement>(null)
  const from = useRef(0)
  const reduce = useReducedMotion()
  const seen = useInView(ref, { once: true })
  const fmt = (n: number) => prefix + n.toLocaleString('en-US', { minimumFractionDigits: decimals, maximumFractionDigits: decimals }) + suffix
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    if (reduce) { el.textContent = fmt(value); from.current = value; return }
    if (!seen) { el.textContent = fmt(from.current); return }
    const c = animate(from.current, value, { duration: 1.1, ease: EASE, onUpdate: (v) => { el.textContent = fmt(v); from.current = v } })
    return () => c.stop()
  }, [value, seen, reduce]) // eslint-disable-line react-hooks/exhaustive-deps
  return <span ref={ref} className={`num ${className ?? ''}`}>{fmt(from.current)}</span>
}

export function Skel({ h = 16, w = '100%', r, className, style }: { h?: number | string; w?: number | string; r?: number | string; className?: string; style?: CSSProperties }) {
  return <div className={`skel ${className ?? ''}`} style={{ height: h, width: w, borderRadius: r, ...style }} aria-hidden />
}

/** Fade-and-lift entrance. Used sparingly, mainly to stagger the first paint of a page. */
export function Rise({ children, i = 0, className, inView, as = 'div', 'aria-label': aria }: { children: ReactNode; i?: number; className?: string; inView?: boolean; as?: 'div' | 'section' | 'li'; 'aria-label'?: string }) {
  const M = motion[as] as typeof motion.div
  const t = { duration: 0.6, delay: i * 0.06, ease: EASE }
  return inView
    ? <M className={className} aria-label={aria} initial={{ opacity: 0, y: 18 }} whileInView={{ opacity: 1, y: 0 }} viewport={{ once: true, amount: 0.15 }} transition={t}>{children}</M>
    : <M className={className} aria-label={aria} initial={{ opacity: 0, y: 14 }} animate={{ opacity: 1, y: 0 }} transition={t}>{children}</M>
}

/** A value that is replaced by a token when "on". The bar is the signature motif for what leaves this machine. */
export function Redact({ real, token, on = true, className }: { real: string; token: string; on?: boolean; className?: string }) {
  return (
    <span className={`rx ${on ? 'on' : ''} ${className ?? ''}`}>
      <span className="rx-real">{real}</span>
      <span className="rx-bar" aria-hidden><span>{token}</span></span>
    </span>
  )
}
