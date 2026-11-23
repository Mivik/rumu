// Load the external plugins' web sides (lib/plugins) and follow them.
import { defineNuxtPlugin } from '#app'
import { startPlugins } from '../lib/plugins'

export default defineNuxtPlugin(() => {
  startPlugins()
})
