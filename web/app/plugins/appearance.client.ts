// The theme and the accent, on <html> before the app paints: the tokens in
// main.css read them there (lib/state/appearance).
import { defineNuxtPlugin } from '#app'
import { applyAppearance } from '../lib/state/appearance'

export default defineNuxtPlugin(() => {
  applyAppearance()
})
