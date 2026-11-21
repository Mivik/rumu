// Boot the store (WS connection + push handlers) once, client-side.
import { watch } from 'vue'
import { defineNuxtPlugin, useRuntimeConfig } from '#app'
import { t } from '../i18n'
import { auth, captureToken, initAuth, setToken } from '../lib/auth'
import { rumu } from '../lib/api'
import { followLink } from '../lib/links'
import { startApp } from '../lib/store'
import { openLightbox, toast } from '../lib/state/ui'
import { checkUpdate, watchUpdates } from '../lib/update'
import { copyText } from '../lib/util'
import { installWidgets } from '../lib/widgets'

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
  initAuth()
  // Opening the token link into the running page changes only the fragment,
  // which does not reload the document — so watch for it here too.
  window.addEventListener('hashchange', () => {
    const token = captureToken()
    if (token !== null && auth.status !== 'ready') {
      setToken(token)
      rumu.reconnect()
    }
  })
  startApp()
  installWidgets()

  // A reconnection is often a server restart, i.e. maybe a new build.
  watchUpdates(useRuntimeConfig().app.buildId)
  watch(rumu.connected, (up) => {
    if (up) void checkUpdate()
  })

  // Clicks inside rendered markdown: copy a fence; follow rumu addresses and
  // card links (lib/links.ts).
  document.addEventListener('click', (e) => {
    const target = e.target as HTMLElement | null

    const copy = target?.closest?.('.md-copy') as HTMLButtonElement | null
    if (copy) {
      void copyCode(copy)
      return
    }

    // An image in the text (lib/markdown.ts): open it full-size.
    const img = target?.closest?.('img[data-lightbox]') as HTMLImageElement | null
    if (img) {
      openLightbox({ src: img.getAttribute('src')!, name: img.alt || null })
      return
    }

    const link = target?.closest?.('a') as HTMLAnchorElement | null
    if (!link) return
    if (followLink(link)) e.preventDefault()
  })
})
