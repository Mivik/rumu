// Markdown rendering: marked + DOMPurify, fences highlighted by highlight.js.
//
// Link rewriting happens on the HTML string *before* DOMPurify (it strips
// unknown-scheme hrefs): `@rumu:` addresses (links, inline code, bare text)
// become `data-rumu` anchors, links relative to a rendered card become
// `data-card-deck`/`data-card-path` anchors, and other workspace-qualified
// paths (`proj:src/a.rs`) become `data-ws-ref` anchors. lib/links.ts follows
// them on click.
//
// Highlighting is synchronous once a grammar is in memory: answer text
// re-renders on every stream chunk, so an async highlighter (shiki) would
// flicker. Grammars are code-split and imported the first time a fence asks
// for one; until it lands the block renders as plain text, then re-renders
// (`grammarVersion`). The token palette lives in main.css and reuses the
// semantic color vars.

import DOMPurify from 'dompurify'
import hljs from 'highlight.js/lib/core'
import { marked, type Token } from 'marked'
import { shallowRef } from 'vue'
import { t } from '../i18n'
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

// Same strokes as Icon.vue's `copy` / `ok`; the click handler in the client
// plugin toggles between them (and copies the fence's source).
const SVG_OPEN =
  '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">'
const COPY_ICON = `${SVG_OPEN}<rect x="9" y="9" width="10.5" height="10.5" rx="2"/><path d="M15 9V6.5A2.5 2.5 0 0 0 12.5 4h-6A2.5 2.5 0 0 0 4 6.5v6A2.5 2.5 0 0 0 6.5 15H9"/></svg>`
const CHECK_ICON = `${SVG_OPEN}<path d="M5.5 13l4.2 4.2L18.5 8"/></svg>`

/** A fence: the (highlighted) code plus its hover copy button. */
function codeBlock(body: string, language?: string): string {
  const label = escapeHtml(t('common.copyCode'))
  return (
    '<div class="md-code">' +
    `<pre><code${language ? ` class="language-${language}"` : ''}>${body}</code></pre>` +
    `<button type="button" class="md-copy" title="${label}" aria-label="${label}">` +
    `<span class="md-copy-icon">${COPY_ICON}</span>` +
    `<span class="md-copy-check">${CHECK_ICON}</span>` +
    '</button></div>'
  )
}

/** One line (or a whole block) highlighted to HTML for a known grammar;
 * plain escaped text otherwise, with the grammar load kicked off so a
 * re-render (via `grammarVersion`) picks it up. */
export function highlightLine(text: string, lang: string): string {
  void grammarVersion.value // tracked: re-render when a grammar lands
  if (!lang || !hljs.getLanguage(lang)) {
    loadGrammar(lang)
    return escapeHtml(text)
  }
  try {
    return hljs.highlight(text, { language: lang, ignoreIllegals: true }).value
  } catch {
    return escapeHtml(text)
  }
}

/** The grammar name a file path's extension (or extensionless basename,
 * e.g. `Makefile`) maps to — "" when nothing matches. */
export function langForPath(path: string): string {
  const name = path.slice(path.lastIndexOf('/') + 1)
  const ext = (name.includes('.') ? name.slice(name.lastIndexOf('.') + 1) : name).toLowerCase()
  return ALIASES[ext] ?? ext
}

function renderCode(text: string, info: string | undefined): string {
  // `rust title="…"`, `ts{1,5}` → the leading word.
  const raw = info?.trim().split(/\s+/)[0]?.replace(/\{.*$/, '').toLowerCase() ?? ''
  const name = ALIASES[raw] ?? raw
  return codeBlock(highlightLine(text, name), name)
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

const RUMU_PREFIX = `${INTERNAL_WORKSPACE}:`

/** A whole `@rumu:` address, as bare text or alone in inline code. Trailing
 * sentence punctuation is not part of it. */
const RUMU_ADDRESS = /^@rumu:[^\s<>()[\]`"']*[^\s<>()[\]`"'.,;:!?]/

/** An address as a plain link, for the href pass in `renderMarkdown`. */
function addressLink(address: string, inner: string): string {
  return `<a href="${escapeHtml(address)}">${inner}</a>`
}

/** Code spans inside link text: already links, never addresses. */
const IN_LINK = Symbol('inLink')

function markInLink(tokens: Token[] | undefined): void {
  for (const t of tokens ?? []) {
    ;(t as Token & { [IN_LINK]?: true })[IN_LINK] = true
    if ('tokens' in t) markInLink(t.tokens)
  }
}

marked.use({
  walkTokens: (token) => {
    if (token.type === 'link') markInLink(token.tokens)
  },
  renderer: {
    // `` `@rumu:/cards/x` `` is how tool output and prompts write addresses.
    codespan(token) {
      const code = `<code>${escapeHtml(token.text)}</code>`
      if ((token as Token & { [IN_LINK]?: true })[IN_LINK]) return code
      const m = RUMU_ADDRESS.exec(token.text)
      return m && m[0] === token.text ? addressLink(token.text, code) : code
    },
  },
  extensions: [
    {
      name: 'rumuAddress',
      level: 'inline',
      start: (src) => {
        const i = src.indexOf(RUMU_PREFIX)
        return i < 0 ? undefined : i
      },
      tokenizer(src) {
        if (this.lexer.state.inLink) return undefined
        const m = RUMU_ADDRESS.exec(src)
        return m ? { type: 'rumuAddress', raw: m[0] } : undefined
      },
      renderer: (token) => addressLink(token.raw, escapeHtml(token.raw)),
    },
  ],
})

/** The card a body is rendered as: relative links resolve inside its deck. */
export interface CardBase {
  deck: string
  path: string
}

/** A body-relative link (`./x`, `../d/y.md`) → the deck-local card path. */
function resolveRelative(base: CardBase, href: string): string | null {
  const segs = base.path.split('/')
  segs.pop()
  for (const part of href.replace(/[?#].*$/, '').split('/')) {
    if (!part || part === '.') continue
    if (part === '..') segs.pop()
    else segs.push(part)
  }
  const path = segs.join('/').replace(/\.md$/, '')
  return path || null
}

/** The anchor attributes for one href, or null to leave it a plain link. */
function anchorFor(href: string, base: CardBase | undefined): string | null {
  // `@rumu:/…` / `@rumu:…`: resolved on click (lib/links.ts).
  if (href.startsWith(RUMU_PREFIX)) {
    const abs = `/${href.slice(RUMU_PREFIX.length).replace(/^\/+/, '')}`
    return `data-rumu="${escapeHtml(abs)}"`
  }
  const scheme = /^([a-z][a-z0-9.-]*):/i.exec(href)?.[1]
  if (scheme) {
    // Other workspace-qualified paths (`proj:src/a.rs`): copied on click.
    return EXTERNAL_SCHEMES.has(scheme.toLowerCase()) ? null : `data-ws-ref="${escapeHtml(href)}"`
  }
  if (!base || href.startsWith('#') || href.startsWith('//')) return null
  // Inside a card, `/cards/…` is the `@rumu` path.
  if (href.startsWith('/')) return href.startsWith('/cards/') ? anchorFor(RUMU_PREFIX + href, base) : null
  let decoded = href
  try {
    decoded = decodeURI(href)
  } catch {
    // A malformed escape: resolve the link as written.
  }
  const path = resolveRelative(base, decoded)
  return path ? `data-card-deck="${escapeHtml(base.deck)}" data-card-path="${escapeHtml(path)}"` : null
}

/**
 * Chat text keeps single newlines as line breaks (`breaks`); hand-written
 * documents such as knowledge cards are hard-wrapped, so they pass
 * `breaks: false`. When rendering a card, pass it as `base` so links
 * relative to it (`./x`, `../x/y.md`) open cards of the same deck.
 */
export function renderMarkdown(
  src: string,
  { breaks = true, base }: { breaks?: boolean; base?: CardBase } = {},
): string {
  const html = marked.parse(src ?? '', { async: false, breaks })
  const linked = html.replace(/<a href="([^"]*)"([^>]*)>/g, (whole, rawHref: string, rest: string) => {
    const href = rawHref.replaceAll('&amp;', '&').replaceAll('&quot;', '"')
    const attrs = anchorFor(href, base)
    if (!attrs) return whole
    // Hovering shows the address, unless the link has its own title.
    const title = rest.includes(' title=') ? '' : ` title="${rawHref}"`
    return `<a href="#" ${attrs}${title}${rest}>`
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
