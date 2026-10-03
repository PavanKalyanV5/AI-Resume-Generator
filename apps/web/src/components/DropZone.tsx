import { animate, motion, useMotionValue, useSpring, useTransform } from 'motion/react'
import { useEffect, useRef, useState, type DragEvent, type KeyboardEvent, type ReactNode } from 'react'

/** A drop target that leans toward the pointer while a file hovers over it and thumps when the file lands. */
export function DropZone({ onFile, accept, children, label, filled }: { onFile: (f: File) => void; accept: string; children: ReactNode; label: string; filled?: boolean }) {
  const [over, setOver] = useState(false)
  const input = useRef<HTMLInputElement>(null)
  const box = useRef<HTMLDivElement>(null)
  const px = useMotionValue(0), py = useMotionValue(0), pulse = useMotionValue(1)
  const ry = useSpring(useTransform(px, [-1, 1], [-4, 4]), { stiffness: 180, damping: 16 })
  const rx = useSpring(useTransform(py, [-1, 1], [3, -3]), { stiffness: 180, damping: 16 })
  const lift = useSpring(1, { stiffness: 320, damping: 15 })
  useEffect(() => { lift.set(over ? 1.025 : 1) }, [over, lift])
  const scale = useTransform([lift, pulse], ([a, b]: number[]) => a * b)

  const track = (e: DragEvent) => {
    e.preventDefault()
    const r = box.current!.getBoundingClientRect()
    px.set(((e.clientX - r.left) / r.width) * 2 - 1); py.set(((e.clientY - r.top) / r.height) * 2 - 1)
    if (!over) setOver(true)
  }
  const leave = (e: DragEvent) => { if (!box.current?.contains(e.relatedTarget as Node)) { setOver(false); px.set(0); py.set(0) } }
  const drop = (e: DragEvent) => {
    e.preventDefault(); setOver(false); px.set(0); py.set(0)
    animate(pulse, [1, 0.955, 1.012, 1], { duration: 0.5, ease: 'easeOut' })
    const f = e.dataTransfer.files[0]; if (f) onFile(f)
  }
  const key = (e: KeyboardEvent) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); input.current?.click() } }
  return (
    <div style={{ perspective: 900 }}>
      <input ref={input} type="file" accept={accept} hidden onChange={(e) => { const f = e.target.files?.[0]; if (f) onFile(f); e.target.value = '' }} />
      <motion.div ref={box} role="button" tabIndex={0} aria-label={label} className={`drop ${over ? 'over' : ''} ${filled ? 'has' : ''}`}
        style={{ rotateX: rx, rotateY: ry, scale }} onClick={() => input.current?.click()} onKeyDown={key}
        onDragEnter={track} onDragOver={track} onDragLeave={leave} onDrop={drop} whileTap={{ scale: 0.985 }}>
        <span className="drop-ring" aria-hidden />
        <motion.div className="drop-in" animate={{ y: over ? -6 : 0 }} transition={{ type: 'spring', stiffness: 300, damping: 16 }}>{children}</motion.div>
      </motion.div>
    </div>
  )
}
