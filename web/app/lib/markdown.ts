// Markdown rendering: marked + DOMPurify, fences highlighted by highlight.js.
//
// Link rewriting happens on the HTML string *before* DOMPurify (it strips
// unknown-scheme hrefs): workspace addresses (`@rumu:`, `@scratch:`) in
// links, inline code and bare text become `data-rumu`/`data-ws-ref` anchors,
// links relative to a rendered card become `data-card-deck`/
// `data-card-path` anchors, and other workspace-qualified paths
// (`proj:src/a.rs`) become `data-ws-ref` anchors. lib/links.ts follows them
// on click.
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
import { state, type LightboxImage } from './state/state'
import { INTERNAL_WORKSPACE, SCRATCH_WORKSPACE } from './types'

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
export const grammarVersion = shallowRef(0)
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

// ---- math: KaTeX, code-split like the grammars. Until it lands a formula
// renders as its escaped source, then re-renders (`katexVersion`).

type Katex = typeof import('katex').default
let katex: Katex | null = null
let katexLoading = false
const katexVersion = shallowRef(0)

function loadKatex(): void {
  if (katex || katexLoading) return
  katexLoading = true
  Promise.all([import('katex'), import('katex/dist/katex.min.css')])
    .then(([mod]) => {
      katex = mod.default
      katexVersion.value++
    })
    .catch(() => {})
    .finally(() => (katexLoading = false))
}

interface MathToken {
  type: 'math' | 'inlineMath'
  raw: string
  text: string
  display: boolean
}

function renderMath({ raw, text, display }: MathToken): string {
  void katexVersion.value // tracked: re-render when KaTeX lands
  if (!katex) {
    loadKatex()
    return escapeHtml(raw)
  }
  return katex.renderToString(text, { displayMode: display, throwOnError: false })
}

/** Inline delimiters, display ones first. A single `$` follows pandoc's
 * rule so prices don't open formulas: no space just inside either `$`, no
 * digit right after the closing one, no blank line in between. */
const INLINE_MATH: [RegExp, boolean][] = [
  [/^\$\$([\s\S]+?)\$\$/, true],
  [/^\\\[([\s\S]+?)\\\]/, true],
  [/^\\\(([\s\S]+?)\\\)/, false],
  [/^\$(?![\s$])((?:\\[\s\S]|[^\\$\n]|\n(?!\s*\n))+?)(?<!\s)\$(?!\d)/, false],
]
/** A display formula on lines of its own. */
const BLOCK_MATH = /^ {0,3}(?:\$\$([\s\S]+?)\$\$|\\\[([\s\S]+?)\\\])[ \t]*(?:\n+|$)/

marked.use({
  extensions: [
    {
      name: 'math',
      level: 'block',
      tokenizer(src): MathToken | undefined {
        const m = BLOCK_MATH.exec(src)
        if (!m) return undefined
        return { type: 'math', raw: m[0], text: (m[1] ?? m[2])!, display: true }
      },
      renderer: (token) => `<div class="md-math">${renderMath(token as MathToken)}</div>`,
    },
    {
      name: 'inlineMath',
      level: 'inline',
      start: (src) => {
        const i = src.search(/\$|\\[([]/)
        return i < 0 ? undefined : i
      },
      tokenizer(src): MathToken | undefined {
        for (const [re, display] of INLINE_MATH) {
          const m = re.exec(src)
          if (m) return { type: 'inlineMath', raw: m[0], text: m[1]!, display }
        }
        return undefined
      },
      renderer: (token) => renderMath(token as MathToken),
    },
  ],
})

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
const SCRATCH_PREFIX = `${SCRATCH_WORKSPACE}:`

/** Where an address ends rather than continues: brackets and sentence
 * punctuation of both widths. Chinese prose puts `，` straight after a path
 * (`见 @scratch:a.md，2026-11-13）`), with no space to end the token, so these
 * have to stop it mid-line. `.` is not here — `a.md` needs it. */
const ADDRESS_END = '\\s<>()[\\]`"\'，、。；：！？（）【】「」『』《》〈〉…“”‘’,;:!?'

/** A whole workspace address (`@rumu:`, `@scratch:`), as bare text or alone
 * in inline code: the text up to the next end mark, without a trailing `.`
 * (which only ever ends the sentence). */
const ADDRESS = new RegExp(`^@(?:rumu|scratch):[^${ADDRESS_END}]*[^${ADDRESS_END}.]`)
/** Where the first workspace address starts (the tokenizer's `start`). */
const ADDRESS_START = /@(?:rumu|scratch):/

/** `proj:src/a.rs` (whole, in inline code) for a registered workspace
 *  `proj`: the names follow the URI-scheme rule, never a common scheme. */
function isWorkspacePath(text: string): boolean {
  const m = /^([a-z][a-z0-9.-]*):[^\s<>()[\]`"']+$/.exec(text)
  return !!m && state.workspaces.some((w) => w.name === m[1])
}

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
      const m = ADDRESS.exec(token.text)
      return (m && m[0] === token.text) || isWorkspacePath(token.text) ? addressLink(token.text, code) : code
    },
  },
  extensions: [
    {
      name: 'workspaceAddress',
      level: 'inline',
      start: (src) => {
        const i = src.search(ADDRESS_START)
        return i < 0 ? undefined : i
      },
      tokenizer(src) {
        if (this.lexer.state.inLink) return undefined
        const m = ADDRESS.exec(src)
        return m ? { type: 'workspaceAddress', raw: m[0] } : undefined
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
  // `@scratch:…`: a workspace address like `proj:`, but the scheme test
  // below does not see it (a scheme starts with a letter, not `@`).
  if (href.startsWith(SCRATCH_PREFIX)) return `data-ws-ref="${escapeHtml(href)}"`
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

/** What `![alt](address)` shows for a workspace address, or null. */
export type ImageResolver = (address: string) => LightboxImage | null

/** The box an image in the text is first drawn in (CSS narrows it further
 *  on a small screen), so nothing shifts as it loads. */
const IMAGE_MAX_W = 480
const IMAGE_MAX_H = 320

/** A workspace address as an image: what `image` resolves it to (opening
 *  full-size on click), else a link to the address — never a broken image.
 *  `alt` and `rawSrc` are as marked escaped them. */
function addressImage(rawSrc: string, alt: string, rest: string, image?: ImageResolver): string {
  const src = rawSrc.replaceAll('&amp;', '&').replaceAll('&quot;', '"')
  const found = image?.(src)
  if (!found) return addressLink(src, alt || rawSrc)
  let size = ''
  if (found.width && found.height) {
    const s = Math.min(1, IMAGE_MAX_W / found.width, IMAGE_MAX_H / found.height)
    size = ` width="${Math.max(1, Math.round(found.width * s))}" height="${Math.max(1, Math.round(found.height * s))}"`
  }
  const title = rest.includes(' title=') ? rest : ` title="${rawSrc}"`
  return `<img src="${escapeHtml(found.src)}" alt="${alt}"${title}${size} data-lightbox loading="lazy">`
}

/**
 * Chat text keeps single newlines as line breaks (`breaks`); hand-written
 * documents such as knowledge cards are hard-wrapped, so they pass
 * `breaks: false`. When rendering a card, pass it as `base` so links
 * relative to it (`./x`, `../x/y.md`) open cards of the same deck. An image
 * at a workspace address (`![alt](@scratch:a.png)`) shows what `image`
 * resolves it to, else becomes a link to the address.
 */
export function renderMarkdown(
  src: string,
  { breaks = true, base, image }: { breaks?: boolean; base?: CardBase; image?: ImageResolver } = {},
): string {
  const html = marked
    .parse(src ?? '', { async: false, breaks })
    .replace(
      /<img src="([^"]*)" alt="([^"]*)"([^>]*)>/g,
      (whole, rawSrc: string, alt: string, rest: string) =>
        anchorFor(rawSrc.replaceAll('&amp;', '&'), undefined)
          ? addressImage(rawSrc, alt, rest, image)
          : whole,
    )
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
