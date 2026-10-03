import type { Resume } from '../types'
const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]!)
// `**x**` markup becomes bold; stray markers are dropped.
const rich = (s: string) => esc(s).replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>').replace(/\*\*/g, '')
// Stand-in for the PDF in demo mode: the same content, laid out as a page.
export function resumeHtml(r: Resume) {
  const p = r.profile
  return `<!doctype html><meta charset="utf-8"><style>
body{font:11pt/1.45 Georgia,serif;color:#111;margin:0;padding:44px 52px;max-width:760px}
h1{font:600 24pt/1.1 system-ui,sans-serif;margin:0 0 2px;letter-spacing:-.02em}
.r{font:11pt system-ui,sans-serif;color:#333}.c{font:9.5pt system-ui,sans-serif;color:#555;margin:2px 0 14px}
h2{font:600 9.5pt system-ui,sans-serif;border-bottom:1px solid #999;margin:16px 0 6px;padding-bottom:2px;letter-spacing:.02em}
.j{display:flex;justify-content:space-between;font-weight:600;margin-top:8px}.j span:last-child{font-weight:400;color:#555}
ul{margin:3px 0 0;padding-left:18px}li{margin:2px 0}</style>
<h1>${esc(p.name)}</h1><div class="r">${esc(p.role)}</div><div class="c">${esc(p.email)} | ${esc(p.phone)} | ${esc(p.location)}</div>
<h2>Summary</h2>${r.summary.map((s) => `<p>${rich(s)}</p>`).join('')}
<h2>Experience</h2>${r.experience.map((e) => `<div class="j"><span>${esc(e.role)}, ${esc(e.organization)}${e.client ? ' (' + esc(e.client) + ')' : ''}</span><span>${esc(e.dateLabel ?? '')}</span></div><ul>${e.bullets.map((b) => `<li>${rich(b)}</li>`).join('')}</ul>`).join('')}
<h2>Skills</h2>${r.skills.map((g) => `<p style="margin:2px 0"><b>${esc(g.label)}:</b> ${g.skills.map(esc).join(', ')}</p>`).join('')}`
}
