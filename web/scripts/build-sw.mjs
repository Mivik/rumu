// Write dist/sw.js after `nuxt generate`: the service worker template
// (sw.template.js) with this build's id and the app shell to precache.
// Fonts only as woff2: every browser that runs a service worker takes it, so
// the woff/ttf fallbacks fontsource ships would only be dead weight.
import { readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const dist = fileURLToPath(new URL('../dist/', import.meta.url))
const { id } = JSON.parse(readFileSync(`${dist}_nuxt/builds/latest.json`, 'utf8'))

const top = readdirSync(dist).filter((f) => f === 'index.html' || /\.(webmanifest|png|svg)$/.test(f))
const bundle = readdirSync(`${dist}_nuxt`).filter((f) => /\.(js|css|woff2)$/.test(f))
const shell = [...top.map((f) => `/${f}`), ...bundle.map((f) => `/_nuxt/${f}`)].sort()

const template = readFileSync(new URL('./sw.template.js', import.meta.url), 'utf8')
const head = `const BUILD = ${JSON.stringify(id)}\nconst SHELL = ${JSON.stringify(shell)}\n\n`
writeFileSync(`${dist}sw.js`, head + template)
console.log(`sw.js: build ${id}, ${shell.length} files precached`)
