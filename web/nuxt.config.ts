import { fileURLToPath } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import VueI18nPlugin from '@intlify/unplugin-vue-i18n/vite'

// rumu web: a client-only SPA. `nuxt generate` writes the static bundle
// straight into web/dist, which the rumu server embeds in release builds
// (crates/rumu-server/src/ws.rs, SPA fallback to index.html).
export default defineNuxtConfig({
  compatibilityDate: '2026-10-01',
  ssr: false,
  devtools: { enabled: false },
  telemetry: false,

  // The code imports everything explicitly (vue, components, lib/*);
  // keep Nuxt's auto-imports out of the way so that stays true.
  imports: { autoImport: false },
  components: { dirs: [] },

  css: [
    '@fontsource-variable/red-hat-text',
    '@fontsource-variable/literata/opsz.css',
    '@fontsource-variable/jetbrains-mono',
    '~/assets/css/main.css',
  ],

  app: {
    head: {
      title: 'rumu',
      htmlAttrs: { lang: 'en' },
      meta: [
        // resizes-content: a touch keyboard shrinks the page (Chrome on
        // Android), so the composer at its foot rises above the keyboard
        // instead of under it. maximum-scale=1: iOS stops zooming into a
        // field set below 16px on focus (pinch zoom still works there), so
        // fields keep their type size on touch screens.
        {
          name: 'viewport',
          content: 'width=device-width, initial-scale=1, maximum-scale=1, interactive-widget=resizes-content',
        },
        { name: 'color-scheme', content: 'light dark' },
        // The installed app's title bar follows the canvas (main.css).
        { name: 'theme-color', content: '#fbf7f1', media: '(prefers-color-scheme: light)' },
        { name: 'theme-color', content: '#1a1714', media: '(prefers-color-scheme: dark)' },
        { name: 'apple-mobile-web-app-title', content: 'rumu' },
      ],
      link: [
        { rel: 'icon', type: 'image/png', href: '/favicon-96x96.png', sizes: '96x96' },
        { rel: 'icon', type: 'image/svg+xml', href: '/favicon.svg' },
        { rel: 'shortcut icon', href: '/favicon.ico' },
        { rel: 'apple-touch-icon', sizes: '180x180', href: '/apple-touch-icon.png' },
        { rel: 'manifest', href: '/site.webmanifest' },
      ],
      script: [
        {
          // Strip the access token out of the address bar before anything
          // renders (lib/auth.ts reads it from the global). replaceState
          // rewrites the current history entry in place, so the token URL
          // never lands in back/forward history either.
          innerHTML:
            "try{var p=new URLSearchParams(location.hash.slice(1));if(p.has('token')){window.__rumuToken=p.get('token');p.delete('token');var r=p.toString();history.replaceState(history.state,'',location.pathname+location.search+(r?'#'+r:''))}}catch(e){}",
        },
        {
          // The theme, before the first paint: the tokens in main.css read
          // it off <html>. The plugin (app/plugins/appearance.client.ts)
          // takes over from here.
          innerHTML:
            "try{var a=JSON.parse(localStorage.getItem('rumu.appearance')||'{}');var d=a.theme==='dark'||(a.theme!=='light'&&matchMedia('(prefers-color-scheme: dark)').matches);document.documentElement.dataset.theme=d?'dark':'light'}catch(e){}",
        },
      ],
    },
  },

  vite: {
    plugins: [
      tailwindcss(),
      // Precompile the catalog JSON into message functions and leave vue-i18n's
      // compiler out of the bundle. Messages are all static, so nothing needs
      // the runtime compiler.
      VueI18nPlugin({
        include: [
          fileURLToPath(new URL('./app/i18n/en/**', import.meta.url)),
          fileURLToPath(new URL('./app/i18n/zh-CN/**', import.meta.url)),
        ],
        runtimeOnly: true,
      }),
    ],
  },

  devServer: { port: 5173 },

  // lib/update.ts runs its own check for a newer build, on reconnects and
  // page visibility as well as a timer; Nuxt's hourly one would only race it.
  experimental: { checkOutdatedBuildInterval: false },

  // Only the production build writes web/dist: nitro empties its publicDir
  // on every start, so setting it for `nuxt dev` too would wipe the bundle
  // the rumu server is serving.
  $production: {
    nitro: {
      output: {
        publicDir: fileURLToPath(new URL('./dist', import.meta.url)),
      },
    },
  },

  nitro: {
    // Dev: forward the WebSocket and the blob store to the rumu server
    // (`pnpm mock` runs it with a scripted LLM). RUMU_WS overrides the
    // target, e.g. another port.
    devProxy: {
      '/ws': { target: process.env.RUMU_WS ?? 'ws://localhost:7878/ws', ws: true },
      '/auth': {
        target: (process.env.RUMU_WS ?? 'ws://localhost:7878/ws')
          .replace(/^ws/, 'http')
          .replace(/\/ws$/, '/auth'),
      },
      '/blobs': {
        target: (process.env.RUMU_WS ?? 'ws://localhost:7878/ws')
          .replace(/^ws/, 'http')
          .replace(/\/ws$/, '/blobs'),
      },
    },
  },

  // Type-check strictness: unused locals/params are errors and switch arms
  // cannot fall through. Indexed access stays unchecked, matching how the
  // store reads arrays. The lib list drops Nuxt's `webworker` entry: this is
  // a client-only SPA, and dom + webworker together give every DOM value two
  // incompatible shapes.
  typescript: {
    tsConfig: {
      compilerOptions: {
        noUnusedLocals: true,
        noUnusedParameters: true,
        noFallthroughCasesInSwitch: true,
        noUncheckedIndexedAccess: false,
      },
    },
  },
})
