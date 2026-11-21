// The theme on <html>, kept in step with the settings and the system's
// (lib/state/appearance); the head script in nuxt.config.ts set it first.
import { defineNuxtPlugin } from '#app'
import { followTheme } from '../lib/state/appearance'

export default defineNuxtPlugin(() => {
  followTheme()
})
