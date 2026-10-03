import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'

export function useWidth<T extends HTMLElement>(): [React.RefObject<T>, number] {
  const ref = useRef<T>(null)
  const [w, setW] = useState(0)
  useEffect(() => {
    const el = ref.current
    if (!el) return
    const ro = new ResizeObserver(([e]) => setW(Math.round(e.contentRect.width)))
    ro.observe(el)
    setW(Math.round(el.getBoundingClientRect().width))
    return () => ro.disconnect()
  }, [])
  return [ref, w]
}

export function useTip(): { show: (e: { clientX: number; clientY: number }, text: ReactNode) => void; hide: () => void; node: ReactNode } {
  const [t, setT] = useState<{ x: number; y: number; text: ReactNode } | null>(null)
  const show = useCallback((e: { clientX: number; clientY: number }, text: ReactNode) => setT({ x: e.clientX, y: e.clientY, text }), [])
  const hide = useCallback(() => setT(null), [])
  const node = t ? <div className="tip" role="tooltip" style={{ left: Math.min(t.x + 12, window.innerWidth - 270), top: t.y + 14 }}>{t.text}</div> : null
  return { show, hide, node }
}

export const prefersReduced = () => window.matchMedia('(prefers-reduced-motion: reduce)').matches
