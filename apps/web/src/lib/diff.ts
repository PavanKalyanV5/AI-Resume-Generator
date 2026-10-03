export type Op = { t: string; k: 'same' | 'del' | 'add' }
const tok = (s: string) => s.split(/(\s+)/).filter(Boolean)
// Word-level LCS diff. Quadratic, fine for bullet-sized text.
export function wordDiff(a: string, b: string): Op[] {
  const x = tok(a), y = tok(b)
  const dp = Array.from({ length: x.length + 1 }, () => new Uint16Array(y.length + 1))
  for (let i = x.length - 1; i >= 0; i--)
    for (let j = y.length - 1; j >= 0; j--)
      dp[i][j] = x[i] === y[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1])
  const out: Op[] = []
  let i = 0, j = 0
  while (i < x.length && j < y.length) {
    if (x[i] === y[j]) { out.push({ t: x[i], k: 'same' }); i++; j++ }
    else if (dp[i + 1][j] >= dp[i][j + 1]) out.push({ t: x[i++], k: 'del' })
    else out.push({ t: y[j++], k: 'add' })
  }
  while (i < x.length) out.push({ t: x[i++], k: 'del' })
  while (j < y.length) out.push({ t: y[j++], k: 'add' })
  // absorb whitespace between two changes of the same kind, then merge runs
  out.forEach((o, i) => { if (o.k === 'same' && !o.t.trim() && i && out[i - 1].k !== 'same' && out[i - 1].k === out[i + 1]?.k) o.k = out[i - 1].k })
  const merged: Op[] = []
  for (const o of out) {
    const l = merged[merged.length - 1]
    if (l && l.k === o.k) l.t += o.t; else merged.push({ ...o })
  }
  // keep trailing spaces of a change outside the highlight
  const res: Op[] = []
  for (const o of merged) {
    const m = o.k !== 'same' && /\s+$/.exec(o.t)
    if (m) { res.push({ t: o.t.slice(0, m.index), k: o.k }, { t: m[0], k: 'same' }) } else res.push(o)
  }
  return res
}
export const changedWords = (ops: Op[]) => ops.filter((o) => o.k !== 'same' && o.t.trim()).length

