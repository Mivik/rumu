// New-version notice and the service worker. Every `pnpm build` gives the
// bundle a fresh Nuxt build id and rewrites /_nuxt/builds/latest.json; when
// the id there differs from the one this page booted with, the page is
// running an old bundle and offers a reload (Toasts.vue). The check runs on
// every (re)connection, whenever the page comes back into view, and every few
// minutes while it is in view, so an installed app left open on a phone
// notices a rebuild without a server restart.
import { reactive } from 'vue'

const INTERVAL = 5 * 60_000

export const update = reactive({
  /** A newer build is on the server. */
  ready: false,
  /** The user waved the notice away; it stays away until the next load. */
  dismissed: false,
})

let current = ''
let worker: ServiceWorkerRegistration | null = null

export function watchUpdates(buildId: string): void {
  current = buildId
  if (import.meta.dev) return
  // Service workers need a secure context (HTTPS or localhost); over plain
  // http the notice still works, there is just no offline shell.
  if ('serviceWorker' in navigator) {
    navigator.serviceWorker.register('/sw.js').then(
      (r) => (worker = r),
      () => {},
    )
  }
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') void checkUpdate()
  })
  setInterval(() => {
    if (document.visibilityState === 'visible') void checkUpdate()
  }, INTERVAL)
}

export async function checkUpdate(): Promise<void> {
  if (import.meta.dev || update.ready || !current) return
  try {
    const res = await fetch('/_nuxt/builds/latest.json', { cache: 'no-store' })
    if (!res.ok) return
    const { id } = (await res.json()) as { id?: string }
    if (!id || id === current) return
    update.ready = true
    // Fetch the new worker now, so its precache is warm by the reload.
    void worker?.update().catch(() => {})
  } catch {
    // Offline or mid-restart; the next check will tell.
  }
}

export function applyUpdate(): void {
  location.reload()
}
