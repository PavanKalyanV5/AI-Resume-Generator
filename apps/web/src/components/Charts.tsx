import { forceCenter, forceCollide, forceLink, forceManyBody, forceSimulation, forceX, forceY, type Simulation, type SimulationLinkDatum, type SimulationNodeDatum } from 'd3-force'
import { scaleBand, scaleLinear } from 'd3-scale'
import { motion } from 'motion/react'
import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import { prefersReduced, useTip, useWidth } from '../lib/hooks'
import type { Clusters, GraphEdge, GraphNode, Resume } from '../types'
import { EASE } from './Motion'

const KIND: Record<string, { name: string; color: string; shape: 'circle' | 'dot' | 'diamond' | 'square' | 'triangle' | 'hex' | 'ring' }> = {
  requirement: { name: 'Requirement', color: 'var(--ink)', shape: 'circle' },
  bullet: { name: 'Bullet', color: 'var(--k-bullet)', shape: 'dot' },
  skill: { name: 'Skill', color: 'var(--c1)', shape: 'diamond' },
  org: { name: 'Employer', color: 'var(--c2)', shape: 'square' },
  role: { name: 'Role', color: 'var(--c4)', shape: 'triangle' },
  project: { name: 'Project', color: 'var(--c3)', shape: 'hex' },
  cert: { name: 'Certification', color: 'var(--c5)', shape: 'ring' },
}
const kinfo = (k: string) => KIND[k] ?? { name: k, color: 'var(--ink-3)', shape: 'dot' as const }
const EDGE: Record<string, { name: string; dash?: string; w: number; color: string }> = {
  covers: { name: 'Covers', w: 1.6, color: 'var(--local)' },
  mentions: { name: 'Mentions', w: 1, color: 'var(--line-2)' },
  contains: { name: 'Contains', dash: '1 3', w: 1.2, color: 'var(--ink-3)' },
  requires: { name: 'Requires', dash: '6 3', w: 1.2, color: 'var(--ink-2)' },
  at: { name: 'At', w: 0.8, color: 'var(--line-2)' },
  similarto: { name: 'Semantically close', dash: '2 3', w: 1.6, color: 'var(--c5)' },
}
const einfo = (k: string) => EDGE[k] ?? { name: k, w: 1, color: 'var(--line-2)' }

export const scoreOf = (s: number) => Math.round(s <= 1 ? s * 100 : s)
export const verdict = (p: number) => (p >= 75 ? 'Strong match' : p >= 55 ? 'Solid match with gaps' : 'Weak match')

export function Gauge({ score }: { score: number }) {
  const p = scoreOf(score), R = 84, C = 110
  const arc = `M ${C - R} 104 A ${R} ${R} 0 0 1 ${C + R} 104`
  return (
    <figure className="gauge" aria-label={`ATS coverage ${p} out of 100`}>
      <svg viewBox="0 0 220 128" width="100%" role="img" aria-label={`Coverage score ${p} of 100`}>
        <path d={arc} fill="none" stroke="var(--line)" strokeWidth="12" strokeLinecap="round" />
        <motion.path d={arc} fill="none" stroke="var(--local)" strokeWidth="12" strokeLinecap="round" initial={{ pathLength: 0 }} animate={{ pathLength: p / 100 }} transition={{ duration: 1.3, ease: EASE, delay: 0.2 }} />
        <text x={C} y="98" textAnchor="middle" className="g-num">{p}</text>
        <text x={C - R} y="124" textAnchor="middle" className="g-end">0</text>
        <text x={C + R} y="124" textAnchor="middle" className="g-end">100</text>
      </svg>
      <figcaption>{verdict(p)}</figcaption>
    </figure>
  )
}

export type Strength = { id: string; label: string; required: boolean; kind: 'keyword' | 'semantic' | 'missing'; strength: number }
/** Strength per requirement = strongest link from a bullet or skill, so the chart and heatmap agree. */
export function strengths(nodes: GraphNode[], edges: GraphEdge[], covered: string[], semantic: string[]): Strength[] {
  return nodes.filter((n) => n.kind === 'requirement').map((n) => {
    const w = edges.filter((e) => e.kind !== 'similarto' && e.kind !== 'requires' && (e.to === n.id || e.from === n.id)).map((e) => e.weight)
    const sem = semantic.includes(n.label)
    const isCovered = sem || covered.includes(n.label)
    return { id: n.id, label: n.label, required: !!n.required, kind: sem ? 'semantic' : isCovered ? 'keyword' : 'missing', strength: w.length ? Math.max(...w) : isCovered ? 0.5 : 0 }
  })
}

export function MatchLegend({ nKw, nSem, nMiss }: { nKw: number; nSem: number; nMiss: number }) {
  return (
    <div className="legend">
      <span><i className="sw kw" />{nKw} keyword match</span>
      {nSem > 0 && <span><i className="sw sem" />{nSem} semantic match</span>}
      <span><i className="sw miss" />{nMiss} missing</span>
    </div>
  )
}

export function CoverageBars({ rows }: { rows: Strength[] }) {
  const order = { keyword: 0, semantic: 1, missing: 2 }
  const sorted = [...rows].sort((a, b) => order[a.kind] - order[b.kind] || b.strength - a.strength)
  return (
    <ul className="clean cbars">
      {sorted.map((r, i) => (
        <li key={r.id} className={r.kind}>
          <div className="row between nowrap">
            <span className="small">{r.label}{r.required && <span className="faint"> (required)</span>}</span>
            <span className="small num val">{r.kind === 'missing' ? 'Missing' : r.kind === 'semantic' ? `Semantic ${Math.round(r.strength * 100)}%` : `${Math.round(r.strength * 100)}%`}</span>
          </div>
          <div className="bar-line" role="img" aria-label={`${r.label}: ${r.kind === 'missing' ? 'missing' : `${r.kind} match at ${Math.round(r.strength * 100)} percent`}`}>
            <motion.i initial={{ scaleX: 0 }} animate={{ scaleX: r.kind === 'missing' ? 0.2 : Math.max(0.05, r.strength) }} transition={{ duration: 0.8, ease: EASE, delay: 0.15 + i * 0.045 }} />
          </div>
        </li>
      ))}
    </ul>
  )
}

export function Heatmap({ nodes, edges, rows }: { nodes: GraphNode[]; edges: GraphEdge[]; rows: Strength[] }) {
  const reqs = nodes.filter((n) => n.kind === 'requirement')
  const bullets = nodes.filter((n) => n.kind === 'bullet')
  const kindOf = (id: string) => rows.find((r) => r.id === id)?.kind ?? 'missing'
  const w = (a: string, b: string) => Math.max(0, ...edges.filter((e) => e.kind === 'covers' && ((e.from === a && e.to === b) || (e.from === b && e.to === a))).map((e) => e.weight))
  const { show, hide, node } = useTip()
  return (
    <div>
      <div className="heat-scroll">
        <div className="heat" style={{ gridTemplateColumns: `minmax(160px,200px) repeat(${bullets.length}, minmax(26px, 1fr))` }} role="table" aria-label="Requirement to bullet coverage">
          <div role="row" style={{ display: 'contents' }}>
            <div />
            {bullets.map((b, i) => <div key={b.id} className="cl" role="columnheader" title={b.label}>{i + 1}</div>)}
          </div>
          {reqs.map((r, ri) => {
            const k = kindOf(r.id)
            return (
              <div role="row" key={r.id} style={{ display: 'contents' }}>
                <div className={`rl ${k}`} role="rowheader">{r.label}{k === 'semantic' && <abbr title="Semantic match: found by meaning, not by keyword"> ≈</abbr>}</div>
                {bullets.map((b, i) => {
                  const v = w(r.id, b.id)
                  const txt = v ? `Bullet ${i + 1} covers "${r.label}" at ${Math.round(v * 100)}%: ${b.label}` : `Bullet ${i + 1} does not address "${r.label}"`
                  return v ? (
                    <button key={b.id} type="button" className="c on" role="cell" aria-label={txt}
                      style={{ ['--v' as string]: Math.round(18 + v * 82), ['--d' as string]: `${(ri + i) * 28}ms` } as CSSProperties}
                      onMouseMove={(e) => show(e, txt)} onMouseLeave={hide} onFocus={(e) => { const r2 = e.currentTarget.getBoundingClientRect(); show({ clientX: r2.left, clientY: r2.bottom }, txt) }} onBlur={hide} />
                  ) : <div key={b.id} className="c" role="cell" aria-label={txt} />
                })}
              </div>
            )
          })}
        </div>
      </div>
      <p className="hint" style={{ marginTop: 10 }}>Columns are your bullets in resume order. Darker means a stronger match. A tilde marks requirements matched by meaning rather than keyword.</p>
      <details className="peek"><summary>Which bullet is which</summary>
        <ol className="small muted" style={{ paddingLeft: 20 }}>{bullets.map((b) => <li key={b.id}>{b.label}</li>)}</ol>
      </details>
      {node}
    </div>
  )
}

function Shape({ shape, r, fill, stroke, sw = 2 }: { shape: string; r: number; fill: string; stroke: string; sw?: number }) {
  const common = { fill, stroke, strokeWidth: sw, strokeLinejoin: 'round' as const }
  switch (shape) {
    case 'diamond': { const a = r * 1.3; return <path {...common} d={`M0 ${-a}L${a} 0L0 ${a}L${-a} 0Z`} /> }
    case 'square': return <rect {...common} x={-r} y={-r} width={r * 2} height={r * 2} rx={2.5} />
    case 'triangle': return <path {...common} d={`M0 ${-r * 1.2}L${r * 1.1} ${r * 0.8}L${-r * 1.1} ${r * 0.8}Z`} />
    case 'hex': return <path {...common} d={[0, 1, 2, 3, 4, 5].map((i) => `${i ? 'L' : 'M'}${(r * 1.15 * Math.cos(Math.PI / 3 * i)).toFixed(1)} ${(r * 1.15 * Math.sin(Math.PI / 3 * i)).toFixed(1)}`).join('') + 'Z'} />
    case 'ring': return <><circle r={r + 1} fill="var(--panel)" stroke={fill} strokeWidth={3} /><circle r={r - 3} fill={fill} /></>
    default: return <circle r={r} {...common} />
  }
}

type SN = SimulationNodeDatum & GraphNode & { r: number; sx: number; sy: number }
type SL = SimulationLinkDatum<SN> & { kind: string; weight: number }
const rOf = (n: GraphNode) => (n.kind === 'requirement' ? (n.required ? 11 : 9) : n.kind === 'org' || n.kind === 'role' ? 8 : n.kind === 'skill' || n.kind === 'project' || n.kind === 'cert' ? 7 : 5)
const short = (s: string, n = 26) => (s.length > n ? s.slice(0, n - 1) + '…' : s)

export function KnowledgeGraph({ graph, missing }: { graph: { nodes: GraphNode[]; edges: GraphEdge[] }; missing: string[] }) {
  const [wrap, width] = useWidth<HTMLDivElement>()
  const w = Math.max(280, width), h = w < 560 ? 420 : 520
  const [sel, setSel] = useState<string | null>(null)
  const [hov, setHov] = useState<string | null>(null)
  const sim = useRef<Simulation<SN, SL> | null>(null)
  const svg = useRef<SVGSVGElement>(null)
  const nodeEls = useRef<(SVGGElement | null)[]>([])
  const lineEls = useRef<(SVGLineElement | null)[]>([])
  const size = useRef({ w, h })
  size.current = { w, h }
  const { show, hide, node: tipNode } = useTip()

  const data = useMemo(() => {
    const n = graph.nodes.map((g, i): SN => { const a = i * 2.399, d = 20 + Math.sqrt(i) * 26; return { ...g, r: rOf(g), x: 0, y: 0, sx: Math.cos(a) * d, sy: Math.sin(a) * d } })
    const l = graph.edges.map((e): SL => ({ source: e.from, target: e.to, kind: e.kind, weight: e.weight }))
    return { n, l }
  }, [graph])
  const gapIds = useMemo(() => {
    const touched = new Set(graph.edges.flatMap((e) => [e.from, e.to]))
    return new Set(graph.nodes.filter((n) => n.kind === 'requirement' && (missing.includes(n.label) || !touched.has(n.id))).map((n) => n.id))
  }, [graph, missing])

  const paint = () => {
    const { w: W, h: H } = size.current
    data.n.forEach((n, i) => {
      n.x = Math.max(n.r + 50, Math.min(W - n.r - 50, n.x ?? 0)); n.y = Math.max(n.r + 16, Math.min(H - n.r - 24, n.y ?? 0))
      nodeEls.current[i]?.setAttribute('transform', `translate(${n.x.toFixed(1)},${n.y.toFixed(1)})`)
    })
    data.l.forEach((l, i) => {
      const a = l.source as SN, b = l.target as SN, el = lineEls.current[i]
      if (el && a.x != null && b.x != null) { el.setAttribute('x1', String(a.x)); el.setAttribute('y1', String(a.y)); el.setAttribute('x2', String(b.x)); el.setAttribute('y2', String(b.y)) }
    })
  }
  // The simulation starts from a tight spiral and opens up, so the graph "unfolds" and cools to rest.
  useEffect(() => {
    if (!width) return
    data.n.forEach((n) => { n.x = w / 2 + n.sx; n.y = h / 2 + n.sy; n.vx = 0; n.vy = 0 })
    const s = forceSimulation(data.n)
      .force('link', forceLink<SN, SL>(data.l).id((d) => d.id).distance((l) => (l.kind === 'at' || l.kind === 'contains' ? 70 : 110)).strength((l) => 0.12 + l.weight * 0.3))
      .force('charge', forceManyBody().strength(-420))
      .force('collide', forceCollide<SN>().radius((d) => d.r + 16))
      .force('x', forceX(w / 2).strength(0.025)).force('y', forceY(h / 2).strength(0.045)).force('center', forceCenter(w / 2, h / 2))
      .alphaDecay(0.024).velocityDecay(0.38)
    sim.current = s
    if (prefersReduced()) { s.stop(); s.tick(300); paint() } else s.on('tick', paint)
    paint()
    return () => { s.stop() }
  }, [data, Math.round(w / 40), h]) // eslint-disable-line react-hooks/exhaustive-deps

  const neighbors = useMemo(() => {
    const focus = sel ?? hov
    if (!focus) return null
    const s = new Set([focus])
    graph.edges.forEach((e) => { if (e.from === focus) s.add(e.to); if (e.to === focus) s.add(e.from) })
    return { s, focus }
  }, [sel, hov, graph])

  const drag = (n: SN) => (e: React.PointerEvent) => {
    const el = svg.current; if (!el) return
    const rect = el.getBoundingClientRect(), k = size.current.w / rect.width
    let moved = false
    const mv = (ev: PointerEvent) => { moved = true; n.fx = (ev.clientX - rect.left) * k; n.fy = (ev.clientY - rect.top) * k; sim.current?.alphaTarget(0.2).restart() }
    const up = () => { n.fx = n.fy = null; sim.current?.alphaTarget(0); window.removeEventListener('pointermove', mv); window.removeEventListener('pointerup', up); if (moved) suppress.current = true }
    window.addEventListener('pointermove', mv); window.addEventListener('pointerup', up)
    e.stopPropagation()
  }
  const suppress = useRef(false)
  const label = (n: SN) => `${kinfo(n.kind).name}: ${n.label}${gapIds.has(n.id) ? ' (gap, nothing in your resume covers it)' : ''}`
  const kinds = [...new Set(graph.nodes.map((n) => n.kind))]
  const ekinds = [...new Set(graph.edges.map((e) => e.kind))]

  return (
    <div className="stack">
      <div ref={wrap}>
        <svg ref={svg} className="graph" viewBox={`0 0 ${w} ${h}`} role="group" aria-label="Knowledge graph. Use Tab to move between nodes, Enter to highlight links." onClick={(e) => { if (e.target === e.currentTarget) setSel(null) }}>
          <g>
            {data.l.map((l, i) => {
              const a = l.source as SN, b = l.target as SN, st = einfo(l.kind)
              const on = !neighbors || (neighbors.s.has(a.id) && neighbors.s.has(b.id) && (a.id === neighbors.focus || b.id === neighbors.focus))
              return <line key={i} ref={(el) => { lineEls.current[i] = el }} stroke={st.color} strokeWidth={st.w + l.weight * 0.8} strokeDasharray={st.dash} strokeLinecap="round" opacity={on ? 0.85 : 0.08} className="edge" />
            })}
          </g>
          {data.n.map((n, i) => {
            const gap = gapIds.has(n.id), ki = kinfo(n.kind)
            const dim = neighbors && !neighbors.s.has(n.id)
            const focus = neighbors?.s.has(n.id)
            const showLabel = ['requirement', 'org', 'role'].includes(n.kind) || focus
            return (
              <g key={n.id} className="nd" ref={(el) => { nodeEls.current[i] = el }} transform={`translate(${w / 2 + n.sx},${h / 2 + n.sy})`} style={{ opacity: dim ? 0.18 : 1 }}>
                <g className="n pop" style={{ animationDelay: `${Math.min(i, 30) * 22}ms` }} tabIndex={0} role="button" aria-label={label(n)} aria-pressed={sel === n.id}
                  onPointerDown={drag(n)} onClick={() => { if (suppress.current) { suppress.current = false; return } setSel(sel === n.id ? null : n.id) }}
                  onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); setSel(sel === n.id ? null : n.id) } }}
                  onMouseEnter={() => setHov(n.id)} onMouseMove={(e) => show(e, label(n))} onMouseLeave={() => { hide(); setHov(null) }} onFocus={() => setHov(n.id)} onBlur={() => setHov(null)}>
                  <circle r={n.r + 9} fill="transparent" />
                  {gap && <circle r={n.r + 6} fill="none" stroke="var(--bad)" strokeWidth="1.6" strokeDasharray="3 2.5" />}
                  <Shape shape={ki.shape} r={n.r} fill={gap ? 'var(--panel)' : ki.color} stroke={gap ? 'var(--bad)' : 'var(--panel)'} />
                  {showLabel && <text y={n.r + 15} textAnchor="middle" className={`g-lab ${n.kind === 'requirement' ? 'req' : ''} ${gap ? 'gap' : ''}`}>{short(n.label)}</text>}
                </g>
              </g>
            )
          })}
        </svg>
      </div>
      <div className="legend">
        {kinds.map((k) => <span key={k}><svg width="16" height="16" viewBox="-8 -8 16 16" aria-hidden><Shape shape={kinfo(k).shape} r={4.5} fill={kinfo(k).color} stroke="var(--panel)" sw={1} /></svg>{kinfo(k).name}</span>)}
        <span><svg width="16" height="16" viewBox="-8 -8 16 16" aria-hidden><circle r="5.5" fill="none" stroke="var(--bad)" strokeWidth="1.5" strokeDasharray="3 2" /></svg>Requirement gap</span>
      </div>
      <div className="legend edges">
        {ekinds.map((k) => { const st = einfo(k); return <span key={k}><svg width="26" height="8" aria-hidden><line x1="1" x2="25" y1="4" y2="4" stroke={st.color} strokeWidth={Math.max(1.4, st.w)} strokeDasharray={st.dash} strokeLinecap="round" /></svg>{st.name}</span> })}
      </div>
      {tipNode}
    </div>
  )
}

/** Convex hull (monotone chain). Good enough to outline a handful of points per cluster. */
function hull(pts: [number, number][]): [number, number][] {
  if (pts.length < 3) return pts
  const p = [...pts].sort((a, b) => a[0] - b[0] || a[1] - b[1])
  const cross = (o: number[], a: number[], b: number[]) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
  const lo: [number, number][] = [], up: [number, number][] = []
  for (const q of p) { while (lo.length > 1 && cross(lo[lo.length - 2], lo[lo.length - 1], q) <= 0) lo.pop(); lo.push(q) }
  for (const q of [...p].reverse()) { while (up.length > 1 && cross(up[up.length - 2], up[up.length - 1], q) <= 0) up.pop(); up.push(q) }
  return [...lo.slice(0, -1), ...up.slice(0, -1)]
}
const CSHAPE = ['circle', 'square', 'diamond', 'triangle', 'hex'] as const

export function ClusterMap({ data }: { data: Clusters }) {
  const [wrap, width] = useWidth<HTMLDivElement>()
  const w = Math.max(280, width), h = w < 560 ? 380 : 460
  const [focus, setFocus] = useState<string | null>(null)
  const [hot, setHot] = useState<string | null>(null)
  const { show, hide, node } = useTip()
  const pad = 56
  const px = (x: number) => pad + ((x + 1) / 2) * (w - pad * 2), py = (y: number) => pad * 0.8 + ((1 - y) / 2) * (h - pad * 1.8)
  const ids = data.clusters.map((c) => c.id)
  const idx = (id: string) => Math.max(0, ids.indexOf(id))
  const groups = data.clusters.map((c) => {
    const pts = data.points.filter((p) => p.cluster === c.id)
    const xy = pts.map((p): [number, number] => [px(p.x), py(p.y)])
    const hl = hull(xy)
    const cx = xy.reduce((a, q) => a + q[0], 0) / (xy.length || 1), top = Math.min(...xy.map((q) => q[1]), h)
    return { c, pts, hl, cx, top }
  })
  return (
    <div className="stack">
      <div ref={wrap}>
        <svg className="cmap" viewBox={`0 0 ${w} ${h}`} role="group" aria-label="Map of your bullets grouped by topic. Nearby points say similar things." onClick={(e) => { if (e.target === e.currentTarget) setFocus(null) }}>
          {[0.25, 0.5, 0.75].map((f) => <g key={f}><line className="gridl" x1={pad * 0.5} x2={w - pad * 0.5} y1={h * f} y2={h * f} /><line className="gridl" y1={pad * 0.4} y2={h - pad * 0.4} x1={w * f} x2={w * f} /></g>)}
          {groups.map(({ c, hl, cx, top }, gi) => {
            const col = `var(--c${gi + 1})`, on = !focus || focus === c.id
            const d = hl.length ? hl.map((q, i) => `${i ? 'L' : 'M'}${q[0].toFixed(1)} ${q[1].toFixed(1)}`).join('') + (hl.length > 1 ? 'Z' : 'h0') : ''
            return (
              <g key={c.id} className="hull-g" style={{ opacity: on ? 1 : 0.14, animationDelay: `${gi * 120}ms` }}>
                <path d={d} fill={col} stroke={col} strokeWidth={46} strokeLinejoin="round" strokeLinecap="round" opacity={0.13} />
                <text x={cx} y={Math.max(14, top - 34)} textAnchor="middle" className="c-lab">{c.label}</text>
              </g>
            )
          })}
          {data.points.map((p, i) => {
            const gi = idx(p.cluster), on = !focus || focus === p.cluster, col = `var(--c${gi + 1})`
            return (
              <g key={p.id} transform={`translate(${px(p.x).toFixed(1)},${py(p.y).toFixed(1)})`} style={{ opacity: on ? 1 : 0.14 }}>
                <g className="n pop" style={{ animationDelay: `${300 + i * 45}ms` }} tabIndex={0} role="img" aria-label={`${data.clusters[gi]?.label}: ${p.label}`}
                  onMouseMove={(e) => { setHot(p.id); show(e, <><b>{data.clusters[gi]?.label}</b><br />{p.label}</>) }} onMouseLeave={() => { setHot(null); hide() }}
                  onFocus={(e) => { setHot(p.id); const r = e.currentTarget.getBoundingClientRect(); show({ clientX: r.left, clientY: r.bottom }, <><b>{data.clusters[gi]?.label}</b><br />{p.label}</>) }} onBlur={() => { setHot(null); hide() }}>
                  <circle r={16} fill="transparent" />
                  <g style={{ transform: hot === p.id ? 'scale(1.35)' : 'scale(1)', transition: 'transform .25s var(--ease)' }}>
                    <Shape shape={CSHAPE[gi % CSHAPE.length]} r={7} fill={col} stroke="var(--panel)" />
                  </g>
                </g>
              </g>
            )
          })}
        </svg>
      </div>
      <div className="legend btns" role="group" aria-label="Focus a topic">
        {data.clusters.map((c, gi) => (
          <button key={c.id} type="button" aria-pressed={focus === c.id} className={focus === c.id ? 'on' : ''} onClick={() => setFocus(focus === c.id ? null : c.id)}>
            <svg width="16" height="16" viewBox="-8 -8 16 16" aria-hidden><Shape shape={CSHAPE[gi % CSHAPE.length]} r={5} fill={`var(--c${gi + 1})`} stroke="var(--panel)" sw={1} /></svg>{c.label}<span className="faint num">{c.size}</span>
          </button>
        ))}
      </div>
      {node}
    </div>
  )
}

export function SkillClusters({ resume, covered, semantic, missing }: { resume: Resume; covered: string[]; semantic: string[]; missing: string[] }) {
  const m = (s: string, list: string[]) => list.some((r) => r.toLowerCase().includes(s.toLowerCase()) || s.toLowerCase().includes(r.toLowerCase()))
  return (
    <div className="stack" style={{ gap: 18 }}>
      {resume.skills.map((g) => (
        <div key={g.label} className="stack" style={{ gap: 8 }}>
          <h3>{g.label}</h3>
          <div className="chips">{g.skills.map((s) => <span key={s} className={'chip' + (m(s, covered) ? (m(s, semantic) ? ' sem' : ' hit') : '')}>{s}</span>)}</div>
        </div>
      ))}
      {missing.length > 0 && (
        <div className="stack" style={{ gap: 8 }}>
          <h3>Asked for, not in your resume</h3>
          <div className="chips">{missing.map((s) => <span key={s} className="chip miss">{s}</span>)}</div>
        </div>
      )}
      <p className="hint">Solid outline: matches a requirement by keyword. Dotted outline: matches by meaning.</p>
    </div>
  )
}

export function BarsByDay({ data }: { data: { day: string; count: number }[] }) {
  const [ref, W] = useWidth<HTMLDivElement>()
  const { show, hide, node } = useTip()
  const H = 220, m = { t: 14, r: 8, b: 30, l: 28 }
  const max = Math.max(4, ...data.map((d) => d.count))
  const x = scaleBand<string>().domain(data.map((d) => d.day)).range([m.l, Math.max(m.l + 10, W - m.r)]).padding(0.34)
  const y = scaleLinear().domain([0, max]).nice().range([H - m.b, m.t])
  const fmt = (d: string) => new Date(d + 'T00:00').toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
  const step = W < 500 ? 3 : 2
  return (
    <div ref={ref}>
      {W > 0 && (
        <svg className="chart" width={W} height={H} role="img" aria-label="Jobs per day">
          {y.ticks(4).map((t) => <g key={t}><line className="grid" x1={m.l} x2={W - m.r} y1={y(t)} y2={y(t)} /><text x={m.l - 8} y={y(t) + 4} textAnchor="end">{t}</text></g>)}
          {data.map((d, i) => {
            const bw = x.bandwidth(), bx = x(d.day)!, bh = Math.max(d.count ? 4 : 0, y(0) - y(d.count))
            return (
              <g key={d.day}>
                <rect className="bar-grow" x={bx} y={y(0) - bh} width={bw} height={bh} rx={Math.min(5, bw / 2)} style={{ animationDelay: `${i * 40}ms` }} />
                <rect x={bx - 4} y={m.t} width={bw + 8} height={H - m.b - m.t} fill="transparent" tabIndex={0} aria-label={`${fmt(d.day)}: ${d.count} jobs`}
                  onMouseMove={(e) => show(e, `${fmt(d.day)}: ${d.count} ${d.count === 1 ? 'job' : 'jobs'}`)} onMouseLeave={hide}
                  onFocus={(e) => { const r = e.currentTarget.getBoundingClientRect(); show({ clientX: r.left, clientY: r.top }, `${fmt(d.day)}: ${d.count} jobs`) }} onBlur={hide} />
                {i % step === (data.length - 1) % step && <text x={bx + bw / 2} y={H - 8} textAnchor="middle">{fmt(d.day)}</text>}
              </g>
            )
          })}
        </svg>
      )}
      {node}
    </div>
  )
}

/** Bars: corrections per job in time order, with a dashed least-squares trend line. The caption in the page carries the numbers. */
export function CorrectionsChart({ data }: { data: { label: string; n: number }[] }) {
  const [ref, W] = useWidth<HTMLDivElement>()
  const { show, hide, node } = useTip()
  const H = 220, m = { t: 14, r: 8, b: 30, l: 28 }
  const x = scaleBand<number>().domain(data.map((_, i) => i)).range([m.l, Math.max(m.l + 10, W - m.r)]).padding(0.34)
  const y = scaleLinear().domain([0, Math.max(3, ...data.map((d) => d.n))]).nice().range([H - m.b, m.t])
  const k = data.length, mx = (k - 1) / 2, my = data.reduce((a, d) => a + d.n, 0) / (k || 1)
  const slope = k > 1 ? data.reduce((a, d, i) => a + (i - mx) * (d.n - my), 0) / data.reduce((a, _, i) => a + (i - mx) ** 2, 0) : 0
  const fit = (i: number) => Math.max(0, my + slope * (i - mx))
  const step = k > 12 ? 2 : 1
  return (
    <div ref={ref}>
      {W > 0 && (
        <svg className="chart" width={W} height={H} role="img" aria-label={`Corrections per job, ${k} jobs, trend ${slope < -0.05 ? 'falling' : slope > 0.05 ? 'rising' : 'flat'}`}>
          {y.ticks(4).map((t) => <g key={t}><line className="grid" x1={m.l} x2={W - m.r} y1={y(t)} y2={y(t)} /><text x={m.l - 8} y={y(t) + 4} textAnchor="end">{t}</text></g>)}
          {data.map((d, i) => {
            const bw = x.bandwidth(), bx = x(i)!, bh = Math.max(d.n ? 4 : 0, y(0) - y(d.n)), tip = `${d.label}: ${d.n} ${d.n === 1 ? 'correction' : 'corrections'}`
            return (
              <g key={i}>
                <rect className="bar-grow" x={bx} y={y(0) - bh} width={bw} height={bh} rx={Math.min(5, bw / 2)} style={{ animationDelay: `${i * 40}ms` }} />
                <rect x={bx - 4} y={m.t} width={bw + 8} height={H - m.b - m.t} fill="transparent" tabIndex={0} aria-label={tip} onMouseMove={(e) => show(e, tip)} onMouseLeave={hide}
                  onFocus={(e) => { const r = e.currentTarget.getBoundingClientRect(); show({ clientX: r.left, clientY: r.top }, tip) }} onBlur={hide} />
                {i % step === 0 && <text x={bx + bw / 2} y={H - 8} textAnchor="middle">{i + 1}</text>}
              </g>
            )
          })}
          {k > 1 && <line className="lc-line" x1={x(0)! + x.bandwidth() / 2} x2={x(k - 1)! + x.bandwidth() / 2} y1={y(fit(0))} y2={y(fit(k - 1))} />}
        </svg>
      )}
      {node}
    </div>
  )
}

export const STATUS_COLOR: Record<string, string> = { succeeded: 'var(--local)', failed: 'var(--bad)', cancelled: 'var(--ink-3)', running: 'var(--c1)', queued: 'var(--c1)' }
export function StatusMix({ by }: { by: Record<string, number> }) {
  const entries = Object.entries(by).filter(([, n]) => n > 0).sort((a, b) => b[1] - a[1])
  const total = entries.reduce((a, [, n]) => a + n, 0) || 1
  return (
    <div className="stack">
      <div className="mix" role="img" aria-label={entries.map(([k, n]) => `${k} ${n}`).join(', ')}>
        {entries.map(([k, n], i) => <motion.span key={k} style={{ flex: n, background: STATUS_COLOR[k] ?? 'var(--ink-3)', transformOrigin: 'left' }} initial={{ scaleX: 0 }} animate={{ scaleX: 1 }} transition={{ duration: 0.7, ease: EASE, delay: i * 0.08 }} title={`${k}: ${n}`} />)}
      </div>
      <ul className="clean stack" style={{ gap: 8 }}>
        {entries.map(([k, n]) => (
          <li key={k} className="row between small"><span><i className="sw" style={{ background: STATUS_COLOR[k] ?? 'var(--ink-3)' }} />{k[0].toUpperCase() + k.slice(1)}</span><span className="num muted">{n} ({Math.round((n / total) * 100)}%)</span></li>
        ))}
      </ul>
    </div>
  )
}
