// Desktop notifications. Uses the Tauri notification plugin when the app runs inside Tauri, otherwise the Web Notification API.
// Both are looked up at call time, so there is no hard dependency on either.
interface TauriNotif { isPermissionGranted?: () => Promise<boolean>; requestPermission?: () => Promise<string>; sendNotification?: (o: { title: string; body?: string }) => void }
const tauri = (): TauriNotif | undefined => (window as unknown as { __TAURI__?: { notification?: TauriNotif } }).__TAURI__?.notification
const KEY = 'veil.notif.ask'

export const osSupported = () => !!tauri() || 'Notification' in window
export const osPermission = (): 'granted' | 'denied' | 'default' | 'unsupported' => (tauri() ? ((localStorage.getItem('veil.tauri.notif') as 'granted' | 'denied' | null) ?? 'default') : 'Notification' in window ? Notification.permission : 'unsupported')
export const askedBefore = () => { try { return localStorage.getItem(KEY) === '1' } catch { return false } }
export const markAsked = () => { try { localStorage.setItem(KEY, '1') } catch { /* private mode */ } }

export async function osRequest(): Promise<boolean> {
  const t = tauri()
  try {
    if (t?.requestPermission) { const r = await t.requestPermission(); const ok = r === 'granted'; localStorage.setItem('veil.tauri.notif', ok ? 'granted' : 'denied'); return ok }
    if ('Notification' in window) return (await Notification.requestPermission()) === 'granted'
  } catch { /* ignore */ }
  return false
}
export function osNotify(title: string, body: string, onClick?: () => void) {
  if (osPermission() !== 'granted') return
  const t = tauri()
  try {
    if (t?.sendNotification) { t.sendNotification({ title, body }); return }
    const n = new Notification(title, { body, tag: 'veil-job' })
    n.onclick = () => { window.focus(); onClick?.(); n.close() }
  } catch { /* some browsers only allow this from a service worker */ }
}
