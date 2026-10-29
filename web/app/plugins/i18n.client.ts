// Install the shared i18n instance and put the detected locale on <html lang>
// (nuxt.config only bakes the initial `en` into the static HTML; later
// switches are handled in setLocale).
import { defineNuxtPlugin } from '#app'
import { i18n } from '../i18n'

export default defineNuxtPlugin((nuxtApp) => {
  nuxtApp.vueApp.use(i18n)
  document.documentElement.lang = i18n.global.locale.value
})
