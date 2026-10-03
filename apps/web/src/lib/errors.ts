import { ApiError, mode } from '../api'
import type { Level, NotifAction } from '../types'

export interface Friendly {
  code: string; known: boolean; title: string; message: string; hint: string; level: 'error' | 'warn' | 'info'
  retryable: boolean; critical: boolean; actions: NotifAction[]
}
type Spec = { title: string; message: string; hint: string; level: Friendly['level']; retry?: boolean; critical?: boolean; act?: 'settings' | 'pii' | 'open' | 'connect' }
const S: Record<string, Spec> = {
  'ai.rate_limited': { level: 'warn', retry: true, title: 'The AI provider is busy', message: 'It rate-limited this request. Nothing was lost.', hint: 'Wait a minute and retry from this step, or switch provider.' },
  'ai.model_unavailable': { level: 'error', act: 'settings', title: 'The AI model is no longer available', message: 'The provider retired the default model.', hint: 'Set RESUME_GEMINI_MODEL or RESUME_ANTHROPIC_MODEL to a current model and restart the app.' },
  'ai.auth': { level: 'error', act: 'settings', title: 'The AI key was rejected', message: 'The provider did not accept the saved API key.', hint: 'Check or replace the key in Settings, then retry.' },
  'ai.budget_exceeded': { level: 'warn', act: 'settings', title: 'Budget cap reached', message: 'Estimated spend has hit your monthly cap, so the job was not sent.', hint: 'Raise the cap in Settings, or use the mock provider.' },
  'ai.timeout': { level: 'warn', retry: true, title: 'The AI took too long', message: 'The provider did not answer in time.', hint: 'Retry from this step. Long resumes can take a second attempt.' },
  'ai.bad_reply': { level: 'warn', retry: true, title: 'The AI reply was unreadable', message: 'The provider sent back something that could not be used.', hint: 'Retry from this step. Your original resume is untouched.' },
  'redact.leak': { level: 'error', critical: true, act: 'pii', title: 'Stopped: a personal value was about to be sent', message: 'The safety check found a real value in the text meant for the AI, so nothing was sent.', hint: 'Open the PII ledger, add the value to Always redact, then start a new job. This job is never retried automatically.' },
  'grounding.reverted': { level: 'info', title: 'Kept your original wording on one bullet', message: 'A rewrite added a claim that is not in your resume, so it was reverted.', hint: 'Compare the Result tab to see which bullet stayed as written.' },
  'render.fit_failed': { level: 'warn', retry: true, title: 'Could not fit the resume on its page', message: 'The layout step could not make the content fit.', hint: 'Retry the PDF step, or shorten a long bullet in your resume.' },
  'render.tex_failed': { level: 'error', retry: true, title: 'The PDF layout step failed', message: 'The typesetting engine returned an error.', hint: 'The DOCX may still be fine. Retry, and if it repeats check the LaTeX install.' },
  'drive.not_connected': { level: 'warn', act: 'settings', title: 'Google Drive is not connected', message: 'The upload step has no Google account to save to.', hint: 'Connect Drive in Settings, then retry the upload.' },
  'drive.quota': { level: 'warn', retry: true, title: 'Your Google Drive is full', message: 'Google refused the upload because the account is out of space.', hint: 'Free some space in Drive, then retry the upload. Your files are safe locally.' },
  'fetch.login_wall': { level: 'info', title: 'This site needs you to log in', message: 'The page could not be fetched without signing in.', hint: 'Paste the job description text instead.' },
  'fetch.blocked': { level: 'warn', title: 'The site blocked the request', message: 'It refused to serve the page to this app.', hint: 'Paste the job description text instead.' },
  'import.low_confidence': { level: 'warn', title: 'That file was hard to read', message: 'Some details may be missing or in the wrong place.', hint: 'Review the warnings before using the imported resume.' },
  'net.offline': { level: 'warn', title: 'You are offline', message: 'There is no network connection right now.', hint: 'Anything local still works. Try again when you are back online.' },
  'net.pinning_failed': { level: 'error', critical: true, title: 'The desktop\'s identity changed', message: 'The certificate it presented is not the one you paired with, so nothing was sent.', hint: 'If you did not reset it on purpose, do not continue. Otherwise unpair and pair again with a new code.' },
  'companion.revoked': { level: 'error', title: 'This phone is no longer paired', message: 'The desktop does not recognise this phone any more. It was probably removed or reset there.', hint: 'Unpair here, then pair again with a new code from the desktop.' },
  'pair.rejected': { level: 'warn', title: 'That code did not work', message: 'The code is wrong, already used, or expired.', hint: 'Generate a new code on the desktop (Settings, Mobile companion) and try again.' },
  'pair.locked': { level: 'warn', title: 'Too many attempts', message: 'The desktop is blocking pairing for a few minutes.', hint: 'Wait about ten minutes, then try again with a fresh code.' },
  'pair.bad_payload': { level: 'warn', title: 'Pairing details are incomplete', message: 'Host, port, fingerprint and code are all needed.', hint: 'Paste the whole pairing text from the desktop, or fill in every field.' },
  'pair.failed': { level: 'error', title: 'The desktop refused pairing', message: 'It answered, but not with a success.', hint: 'Check that Mobile companion is on, then try a new code.' },
  'server.unreachable': { level: 'error', title: 'Cannot reach the local server', message: 'The app could not talk to its backend.', hint: 'Check that Veil is still running, then try again.' },
}
export const isCritical = (code?: string) => !!code && !!S[code]?.critical

export function friendly(code?: string | null, fb?: { title?: string; message?: string; hint?: string; retryable?: boolean }, jobId?: string, step?: string): Friendly {
  let sp = code ? S[code] : undefined
  if (code === 'server.unreachable' && mode.companion && sp) sp = { ...sp, title: 'Cannot reach your desktop', message: 'The desktop did not answer.', hint: 'Check that the desktop app is running and on the same network (or Tailscale), then retry.' }
  const actions: NotifAction[] = []
  if (sp?.retry && !sp.critical && jobId) actions.push({ label: step ? 'Retry from this step' : 'Retry', action: 'retry', target: step })
  if (sp?.act === 'settings') actions.push({ label: 'Open Settings', action: 'settings' })
  if (sp?.act === 'pii') actions.push({ label: 'Open PII ledger', action: 'open', target: '/pii' })
  if (jobId) actions.push({ label: 'Open job', action: 'open' })
  return {
    code: code ?? 'unknown', known: !!sp, level: sp?.level ?? 'error', critical: !!sp?.critical, retryable: !!sp?.retry && !sp.critical,
    title: sp?.title ?? fb?.title ?? 'Something went wrong', message: sp?.message ?? fb?.message ?? 'The action did not complete.',
    hint: sp?.hint ?? fb?.hint ?? 'Try again. If it keeps happening, open the details and share the code.', actions,
  }
}

/** Turns anything thrown by a request into friendly copy. */
export function fromError(e: unknown, jobId?: string, step?: string): Friendly {
  if (e instanceof ApiError) return friendly(e.code, { message: e.message, hint: e.hint }, jobId, step)
  if (e instanceof TypeError || (e instanceof Error && /fetch|network|load failed/i.test(e.message))) return friendly(navigator.onLine ? 'server.unreachable' : 'net.offline')
  return friendly(undefined, { message: e instanceof Error ? e.message : undefined }, jobId, step)
}
export const toLevel = (f: Friendly): Level => (f.level === 'info' ? 'info' : f.level === 'warn' ? 'warn' : 'error')
