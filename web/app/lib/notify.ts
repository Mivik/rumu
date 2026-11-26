// System notifications for sessions that need the user or are done: a
// session's status moving to waiting (a question or an approval), from
// running to done, or to failed. Only while a page is open — there is no Web
// Push — and not for the session the user is looking at. What to notify about
// is kept per browser, as the permission is. Every open tab notifies; the tag
// makes the browser show one of them.
import { reactive, watch } from 'vue'
import { t } from '../i18n'
import type { SessionInfo } from './types'
import { state } from './state/state'

const KEY = 'rumu.notify'

export type NotifyEvent = 'waiting' | 'done' | 'failed'
export const NOTIFY_EVENTS: NotifyEvent[] = ['waiting', 'done', 'failed']

interface NotifyPrefs {
  enabled: boolean
  events: Record<NotifyEvent, boolean>
}

export const notifyPrefs = reactive<NotifyPrefs>({
  enabled: false,
  events: { waiting: true, done: true, failed: true },
  ...load(),
})

watch(notifyPrefs, (p) => localStorage.setItem(KEY, JSON.stringify(p)), { deep: true })

function load(): Partial<NotifyPrefs> {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? '{}')
  } catch {
    return {}
  }
}

/** Notifications need a secure context (HTTPS or localhost). */
export const notifySupported = typeof window !== 'undefined' && 'Notification' in window

/** The browser's answer, refreshed when the setting is turned on. */
export const permission = reactive({
  value: (notifySupported ? Notification.permission : 'denied') as NotificationPermission,
})

/** Turn notifications on, asking the browser first; off when it says no. */
export async function enableNotify(on: boolean): Promise<void> {
  if (!on || !notifySupported) {
    notifyPrefs.enabled = false
    return
  }
  permission.value = await Notification.requestPermission()
  notifyPrefs.enabled = permission.value === 'granted'
}

function eventOf(prev: SessionInfo['status'], next: SessionInfo['status']): NotifyEvent | null {
  if (prev === next) return null
  if (next === 'waiting') return 'waiting'
  if (next === 'failed') return 'failed'
  if (prev === 'running' && (next === 'idle' || next === 'background')) return 'done'
  return null
}

/** `session.updated`: notify about the move from `prev` to `next`. */
export function notifyStatus(prev: SessionInfo | undefined, next: SessionInfo): void {
  if (!prev || !notifyPrefs.enabled || !notifySupported || Notification.permission !== 'granted') return
  const event = eventOf(prev.status, next.status)
  if (!event || !notifyPrefs.events[event]) return
  const looking =
    document.visibilityState === 'visible' &&
    document.hasFocus() &&
    state.sessionId === next.id &&
    !state.page
  if (looking) return
  void show(next.title || t('common.untitled'), {
    body: t(`settings.notify_${event}`),
    tag: `${next.id}:${event}`,
    icon: '/web-app-manifest-192x192.png',
    data: { session: next.id },
  })
}

/** Through the service worker when there is one (mobile browsers allow no
 *  other way); a click there comes back as a message (`onNotifyClick`). */
async function show(title: string, options: NotificationOptions): Promise<void> {
  const reg = 'serviceWorker' in navigator ? await navigator.serviceWorker.getRegistration() : undefined
  if (reg) return reg.showNotification(title, options)
  const n = new Notification(title, options)
  n.onclick = () => {
    window.focus()
    open?.((options.data as { session: string }).session)
    n.close()
  }
}

let open: ((session: string) => void) | undefined

/** What a click on a notification does: open its session. */
export function onNotifyClick(handler: (session: string) => void): void {
  open = handler
  navigator.serviceWorker?.addEventListener('message', (e) => {
    const data = e.data as { type?: string; session?: string } | null
    if (data?.type === 'notification.click' && data.session) handler(data.session)
  })
}
