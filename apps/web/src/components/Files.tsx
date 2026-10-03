import { CloudArrowUp, FilePdf, FileDoc } from '@phosphor-icons/react'
import { api, mode } from '../api'
import { useApp } from '../store'
import type { DriveLink } from '../types'

/** PDF opens in a tab, DOCX downloads. In demo mode no files exist, so the buttons explain that. */
export function FileButtons({ id, files, size }: { id: string; files: string[]; size?: 'sm' }) {
  const { toast } = useApp()
  const pdf = files.includes('resume.pdf'), docx = files.includes('resume.docx')
  const demo = () => toast({ kind: 'info', text: 'Demo mode does not generate files. With the backend running, this opens the real document.' })
  const cls = `btn ${size ?? ''}`
  const one = (ok: boolean, name: string, label: string, Icon: typeof FilePdf, open: boolean) => {
    if (!ok) return <button className={cls} disabled title="Not ready yet"><Icon size={15} aria-hidden />{label}</button>
    if (mode.demo) return <button className={cls} onClick={demo}><Icon size={15} aria-hidden />{label}</button>
    return <a className={cls} href={api.fileUrl(id, name)} {...(open ? { target: '_blank', rel: 'noreferrer' } : { download: true })}><Icon size={15} aria-hidden />{label}</a>
  }
  return <>{one(pdf, 'resume.pdf', 'PDF', FilePdf, true)}{one(docx, 'resume.docx', 'DOCX', FileDoc, false)}</>
}

export function DriveLinks({ links }: { links: DriveLink[] | null | undefined }) {
  if (!links?.length) return null
  return (
    <>
      {links.map((l) => (
        <a key={l.url + l.name} className="chip-link z-drive" href={l.url} target="_blank" rel="noreferrer" title={l.name}>
          <CloudArrowUp size={14} aria-hidden /><span>{/\.docx$/i.test(l.name) ? 'DOCX in Drive' : /\.pdf$/i.test(l.name) ? 'PDF in Drive' : 'Open in Drive'}</span>
        </a>
      ))}
    </>
  )
}
