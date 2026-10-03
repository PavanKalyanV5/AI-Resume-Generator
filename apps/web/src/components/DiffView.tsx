import type { ReactNode } from 'react'
import { wordDiff, type Op } from '../lib/diff'
import type { Resume } from '../types'

/** Per character of the marker-free text: inside a `**bold**` pair? (Unbalanced markers mean no bold.) */
function boldMask(s: string): boolean[] {
  const parts = s.split('**')
  if (parts.length % 2 === 0) return Array.from(strip(s), () => false)
  return parts.flatMap((p, i) => Array.from(p, () => i % 2 === 1))
}
const strip = (s: string) => s.replace(/\*\*/g, '')

/** Render diff ops of `strip(text)`, with the bold spans of `text` as <strong> (never raw asterisks). */
function render(ops: Op[], text: string) {
  const mask = boldMask(text)
  let at = 0
  return ops.map((o, i) => {
    const kids: ReactNode[] = []
    for (let j = 0; j < o.t.length;) {
      let k = j
      while (k < o.t.length && mask[at + k] === mask[at + j]) k++
      const seg = o.t.slice(j, k)
      kids.push(mask[at + j] ? <strong key={j}>{seg}</strong> : seg)
      j = k
    }
    at += o.t.length
    return o.k === 'add' ? <ins key={i}>{kids}</ins> : o.k === 'del' ? <del key={i}>{kids}</del> : <span key={i}>{kids}</span>
  })
}
const same = (ops: Op[]) => ops.filter((o) => o.k === 'same').reduce((n, o) => n + o.t.split(/\s+/).filter(Boolean).length, 0)

function best(text: string, pool: string[]) {
  let b = '', n = -1
  for (const p of pool) { const s = same(wordDiff(strip(text), strip(p))); if (s > n) { n = s; b = p } }
  return b
}
const Side = ({ items, other, side, extra, wrap }: { items: string[]; other: string[]; side: 'old' | 'new'; extra?: (i: number) => ReactNode; wrap?: (i: number, raw: string, node: ReactNode) => ReactNode }) => (
  <ul>
    {items.map((t, i) => {
      const m = best(t, other)
      const ops = side === 'old' ? wordDiff(strip(t), strip(m)).filter((o) => o.k !== 'add') : wordDiff(strip(m), strip(t)).filter((o) => o.k !== 'del')
      const node = render(ops, t)
      return <li key={i}>{wrap ? wrap(i, t, node) : node}{extra?.(i)}</li>
    })}
  </ul>
)

/** `edit(rolePos, i, raw, node)` wraps a tailored line (rolePos -1 is the summary) to make it editable. `bullets(rolePos, i)` maps a shown bullet to its feedback control (the id is relative to the full resume, not this subset). */
export function DiffView({ original, tailored, bullets, edit }: { original: Resume; tailored: Resume; bullets?: (role: number, i: number) => ReactNode; edit?: (role: number, i: number, raw: string, node: ReactNode) => ReactNode }) {
  const skills = (r: Resume) => r.skills.map((g) => `${g.label}: ${g.skills.join(', ')}`)
  return (
    <div className="cmp">
      <div className="h">Original</div><div className="h">Tailored</div>
      <div className="sec">Summary</div>
      <div className="cell"><Side items={original.summary} other={tailored.summary} side="old" /></div>
      <div className="cell"><Side items={tailored.summary} other={original.summary} side="new" wrap={edit && ((i, raw, node) => edit(-1, i, raw, node))} /></div>
      {original.experience.map((e, p) => {
        const t = tailored.experience.find((x) => x.id === e.id) ?? e
        return (
          <div key={e.id} style={{ display: 'contents' }}>
            <div className="sec">{e.role}, {e.organization}</div>
            <div className="cell"><Side items={e.bullets} other={t.bullets} side="old" /></div>
            <div className="cell"><Side items={t.bullets} other={e.bullets} side="new" extra={bullets && ((i) => bullets(p, i))} wrap={edit && ((i, raw, node) => edit(p, i, raw, node))} /></div>
          </div>
        )
      })}
      <div className="sec">Skills</div>
      <div className="cell"><Side items={skills(original)} other={skills(tailored)} side="old" /></div>
      <div className="cell"><Side items={skills(tailored)} other={skills(original)} side="new" /></div>
    </div>
  )
}
