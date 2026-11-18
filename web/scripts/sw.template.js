// rumu's service worker: keeps the app shell (index.html and the hashed
// bundle) in Cache Storage, so the installed app opens fast and, with the
// server out of reach, still comes up and shows its reconnecting state.
//
// build-sw.mjs writes this file to dist/sw.js after `nuxt generate`,
// prepending BUILD (the Nuxt build id) and SHELL (the paths to precache).
// Every build changes those bytes, which is what makes the browser install a
// fresh worker. The page learns about new builds on its own (lib/update.ts).
/* global BUILD, SHELL */

const PREFIX = 'rumu-shell-'
const CACHE = PREFIX + BUILD
const ASSETS = new Set(SHELL)

self.addEventListener('install', (event) => {
  // Safe to take over at once: pages load index.html from the network when
  // they can, and the precached files are content-hashed, so a page never
  // gets one build's file under another build's name.
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll(SHELL))
      .then(() => self.skipWaiting()),
  )
})

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(keys.filter((k) => k.startsWith(PREFIX) && k !== CACHE).map((k) => caches.delete(k))),
      )
      .then(() => self.clients.claim()),
  )
})

self.addEventListener('fetch', (event) => {
  const req = event.request
  if (req.method !== 'GET') return
  const url = new URL(req.url)
  if (url.origin !== self.location.origin) return
  // /ws, /auth and /blobs stay with the network and the server's own headers.
  if (req.mode === 'navigate') {
    if (/^\/(ws|auth|blobs)(\/|$)/.test(url.pathname)) return
    event.respondWith(page(req))
  } else if (ASSETS.has(url.pathname)) {
    event.respondWith(caches.match(url.pathname).then((hit) => hit ?? fetch(req)))
  }
})

/** Network first, so the page matches the server it talks to; the cached
 *  shell only when the server cannot be reached. */
async function page(req) {
  try {
    return await fetch(req)
  } catch (err) {
    const shell = await caches.match('/index.html')
    if (shell) return shell
    throw err
  }
}
