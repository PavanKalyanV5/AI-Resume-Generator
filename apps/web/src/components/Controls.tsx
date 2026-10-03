import { motion } from 'motion/react'
import { useId, type ReactNode } from 'react'
import { SPRING } from './Motion'

export function Seg<T extends string>({ value, onChange, options, label }: { value: T; onChange: (v: T) => void; options: { value: T; label: ReactNode; disabled?: boolean }[]; label: string }) {
  const id = useId()
  return (
    <div className="seg-ctl" role="radiogroup" aria-label={label}>
      {options.map((o) => (
        <label key={o.value} className={o.disabled ? 'dis' : ''}>
          <input type="radio" name={id} value={o.value} checked={value === o.value} disabled={o.disabled} onChange={() => onChange(o.value)} />
          {value === o.value && <motion.span layoutId={`seg-${id}`} className="seg-pill" transition={SPRING} />}
          <span className="seg-txt">{o.label}</span>
        </label>
      ))}
    </div>
  )
}

export function Toggle({ on, onChange, label, disabled, id }: { on: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean; id?: string }) {
  return (
    <button id={id} type="button" role="switch" aria-checked={on} aria-label={label} disabled={disabled} className={`switch ${on ? 'on' : ''}`} onClick={() => onChange(!on)}>
      <motion.span className="knob" layout transition={SPRING} />
    </button>
  )
}
