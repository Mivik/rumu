// Markdown rendering: marked + DOMPurify, fences highlighted by highlight.js.
//
// Link rewriting happens on the HTML string *before* DOMPurify (it strips
// unknown-scheme hrefs): card links become in-app `data-card-id` anchors,
// other workspace-qualified paths (`proj:src/a.rs`, `@rumu:/sessions/…`)
// become inert `data-ws-ref` anchors.
//
// Highlighting is synchronous once a grammar is in memory: answer text
// re-renders on every stream chunk, so an async highlighter (shiki) would
// flicker. Grammars are code-split and imported the first time a fence asks
// for one; until it lands the block renders as plain text, then re-renders
// (`grammarVersion`). The token palette lives in main.css and reuses the
// semantic color vars.

import DOMPurify from 'dompurify'
import hljs from 'highlight.js/lib/core'
import { marked } from 'marked'
import { shallowRef } from 'vue'
import { INTERNAL_WORKSPACE } from './types'

/** One chunk per grammar, fetched on first use. Core stays in the entry
 * chunk: it's small, and every highlight needs it anyway. */
const GRAMMARS = {
  bash: () => import('highlight.js/lib/languages/bash'),
  c: () => import('highlight.js/lib/languages/c'),
  cpp: () => import('highlight.js/lib/languages/cpp'),
  css: () => import('highlight.js/lib/languages/css'),
  diff: () => import('highlight.js/lib/languages/diff'),
  dockerfile: () => import('highlight.js/lib/languages/dockerfile'),
  go: () => import('highlight.js/lib/languages/go'),
  ini: () => import('highlight.js/lib/languages/ini'),
  java: () => import('highlight.js/lib/languages/java'),
  javascript: () => import('highlight.js/lib/languages/javascript'),
  json: () => import('highlight.js/lib/languages/json'),
  kotlin: () => import('highlight.js/lib/languages/kotlin'),
  lua: () => import('highlight.js/lib/languages/lua'),
  makefile: () => import('highlight.js/lib/languages/makefile'),
  markdown: () => import('highlight.js/lib/languages/markdown'),
  php: () => import('highlight.js/lib/languages/php'),
  python: () => import('highlight.js/lib/languages/python'),
  ruby: () => import('highlight.js/lib/languages/ruby'),
  rust: () => import('highlight.js/lib/languages/rust'),
  sql: () => import('highlight.js/lib/languages/sql'),
  typescript: () => import('highlight.js/lib/languages/typescript'),
  xml: () => import('highlight.js/lib/languages/xml'),
  yaml: () => import('highlight.js/lib/languages/yaml'),
}

/** Fence aliases → the grammar name above. hljs's own alias table only helps
 * *after* a grammar is registered, so lookups before a lazy import land need
 * this map. Aliases not listed here still work once their grammar is loaded. */
const ALIASES: Record<string, string> = {
  'c++': 'cpp',
  cc: 'cpp',
  h: 'c',
  hpp: 'cpp',
  console: 'bash',
  shell: 'bash',
  'shell-session': 'bash',
  shellscript: 'bash',
  sh: 'bash',
  zsh: 'bash',
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  ts: 'typescript',
  tsx: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  jsonc: 'json',
  json5: 'json',
  py: 'python',
  rs: 'rust',
  yml: 'yaml',
  md: 'markdown',
  mkd: 'markdown',
  toml: 'ini',
  golang: 'go',
  kt: 'kotlin',
  kts: 'kotlin',
  rb: 'ruby',
  docker: 'dockerfile',
  mk: 'makefile',
  make: 'makefile',
  html: 'xml',
  xhtml: 'xml',
  svg: 'xml',
}

/** Bumped when a lazy grammar finishes registering, so every computed that
 * rendered a code block (and thus read it) re-runs and highlights the block. */
const grammarVersion = shallowRef(0)
const loading = new Set<string>()

function loadGrammar(name: string): void {
  if (!name || loading.has(name) || hljs.getLanguage(name)) return
  const load = GRAMMARS[name as keyof typeof GRAMMARS]
  if (!load) return
  loading.add(name)
  load()
    .then((mod) => {
      hljs.registerLanguage(name, mod.default)
      grammarVersion.value++
    })
    // Unknown network failure: allow a retry on the next render.
    .catch(() => {})
    .finally(() => loading.delete(name))
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
}

function renderCode(text: string, info: string | undefined): string {
  void grammarVersion.value // tracked: re-render when a grammar lands

  // `rust title="…"`, `ts{1,5}` → the leading word.
  const raw = info?.trim().split(/\s+/)[0]?.replace(/\{.*$/, '').toLowerCase() ?? ''
  const name = ALIASES[raw] ?? raw
  if (!name || !hljs.getLanguage(name)) {
    loadGrammar(name)
    return `<pre><code>${escapeHtml(text)}</code></pre>`
  }
  try {
    const body = hljs.highlight(text, { language: name, ignoreIllegals: true }).value
    return `<pre><code class="language-${name}">${body}</code></pre>`
  } catch {
    return `<pre><code>${escapeHtml(text)}</code></pre>`
  }
}

marked.setOptions({ gfm: true })
marked.use({ renderer: { code: ({ text, lang }) => renderCode(text, lang) } })

/** URI schemes that are real links, never workspace addresses. */
const EXTERNAL_SCHEMES = new Set([
  'http',
  'https',
  'mailto',
  'ftp',
  'tel',
  'data',
  'javascript',
  'ssh',
  'git',
  'file',
])

/** `dev/rumu/kernel` → `dev/rumu` (the card's directory). */
function cardDir(cardId: string): string[] {
  const segs = cardId.split('/')
  segs.pop()
  return segs
}

/** Resolve a card-body-relative link against the current card's directory. */
function resolveRelative(cardId: string, href: string): string | null {
  const segs = cardDir(cardId)
  for (const part of href.split('/')) {
    if (!part || part === '.') continue
    if (part === '..') segs.pop()
    else segs.push(part)
  }
  const id = segs.join('/').replace(/\.md$/, '')
  return id ? id : null
}

/** Rewrite one href to `{ cardId }` (an in-app card link) or leave it. */
function cardLinkFor(href: string, currentCard: string | undefined): string | null {
  // `@rumu:/cards/<id>` / `@rumu:cards/<id>`, optional `.md`.
  const prefix = `${INTERNAL_WORKSPACE}:`
  if (href.startsWith(prefix)) {
    const rest = href.slice(prefix.length).replace(/^\/?/, '')
    const m = /^cards\/([^#?]+?)(?:\.md)?(?:[?#].*)?$/.exec(rest)
    if (m) return m[1]!
  }
  if (!currentCard) return null
  // Rendering a card: `/cards/<id>` and relative links resolve inside the
  // card library, relative to the current card's directory.
  let m = /^\/cards\/([^#?]+?)(?:\.md)?(?:[?#].*)?$/.exec(href)
  if (m) return m[1]!
  if (
    href.startsWith(prefix) ||
    /^[a-z][a-z0-9.-]*:/i.test(href) ||
    href.startsWith('//') ||
    href.startsWith('#')
  ) {
    return null
  }
  return resolveRelative(currentCard, decodeURI(href))
}

/**
 * Chat text keeps single newlines as line breaks (`breaks`); hand-written
 * documents such as knowledge cards are hard-wrapped, so they pass
 * `breaks: false`. When rendering a card, pass its `cardId` so links
 * relative to that card (`./x`, `../x/y.md`) open cards too.
 */
export function renderMarkdown(
  src: string,
  { breaks = true, cardId }: { breaks?: boolean; cardId?: string } = {},
): string {
  const html = marked.parse(src ?? '', { async: false, breaks })
  const linked = html.replace(/<a href="([^"]*)"([^>]*)>/g, (whole, rawHref: string, rest: string) => {
    const href = rawHref.replaceAll('&amp;', '&')
    const card = cardLinkFor(href, cardId)
    if (card) {
      return `<a href="#" data-card-id="${card}"${rest}>`
    }
    // Other workspace-qualified paths (`proj:src/a.rs`, `@rumu:/sessions/…`)
    // render as inert references.
    const ws = href.startsWith(`${INTERNAL_WORKSPACE}:`)
      ? INTERNAL_WORKSPACE
      : /^([a-z][a-z0-9.-]*):/i.exec(href)?.[1]
    if (ws && !EXTERNAL_SCHEMES.has(ws.toLowerCase())) {
      return `<a href="#" data-ws-ref="${rawHref}" title="${rawHref}"${rest}>`
    }
    return whole
  })
  return DOMPurify.sanitize(linked)
}

/** One-line JSON for compact display; falls back to String(v). */
export function prettyJson(v: unknown): string {
  if (v === undefined) return ''
  if (typeof v === 'string') return v
  try {
    return JSON.stringify(v, null, 2)
  } catch {
    return String(v)
  }
}
