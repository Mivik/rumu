// Boot the store (WS connection + push handlers) once, client-side.
import { defineNuxtPlugin } from '#app'
import { openDrawer, startApp } from '../lib/store'

export default defineNuxtPlugin(() => {
  startApp()

  // Links inside rendered markdown: card links open the card drawer;
  // other workspace-qualified paths (`proj:src/a.rs`, `@rumu:/sessions/…`)
  // are inert — the click is only prevented.
  document.addEventListener('click', (e) => {
    const target = e.target as HTMLElement | null
    const link = target?.closest?.('a') as HTMLAnchorElement | null
    if (!link) return
    if (link.dataset.cardId) {
      e.preventDefault()
      openDrawer(link.dataset.cardId)
    } else if (link.dataset.wsRef !== undefined) {
      e.preventDefault()
    }
  })
})
