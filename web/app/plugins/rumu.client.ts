// Boot the store (WS connection + push handlers) once, client-side.
import { defineNuxtPlugin } from '#app'
import { t } from '../i18n'
import { openDrawer, startApp } from '../lib/store'
import { toast } from '../lib/state/ui'
import { copyText } from '../lib/util'

/** Copy a fence's source; the button acknowledges inline for a moment. */
async function copyCode(btn: HTMLButtonElement): Promise<void> {
  const code = btn.closest('.md-code')?.querySelector('code')?.textContent ?? ''
  if (!(await copyText(code))) {
    toast('error', t('common.copyFailed'))
    return
  }
  btn.dataset.copied = ''
  btn.title = t('common.copiedCode')
  btn.setAttribute('aria-label', t('common.copiedCode'))
  setTimeout(() => {
    // A streaming re-render may have replaced the button by now.
    if (!btn.isConnected) return
    delete btn.dataset.copied
    btn.title = t('common.copyCode')
    btn.setAttribute('aria-label', t('common.copyCode'))
  }, 1400)
}

export default defineNuxtPlugin(() => {
  startApp()

  // Clicks inside rendered markdown: copy a fence; card links open the card
  // drawer; other workspace-qualified paths (`proj:src/a.rs`, `@rumu:/sessions/…`)
  // are inert — the click is only prevented.
  document.addEventListener('click', (e) => {
    const target = e.target as HTMLElement | null

    const copy = target?.closest?.('.md-copy') as HTMLButtonElement | null
    if (copy) {
      void copyCode(copy)
      return
    }

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
