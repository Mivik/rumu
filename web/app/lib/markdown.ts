// Markdown rendering: marked + DOMPurify.
//
// Link rewriting happens on the HTML string *before* DOMPurify (it strips
// unknown-scheme hrefs): card links become in-app `data-card-id` anchors,
// other workspace-qualified paths (`proj:src/a.rs`, `@rumu:/session/…`)
// become inert `data-ws-ref` anchors.

import DOMPurify from 'dompurify'
import { marked } from 'marked'
import { INTERNAL_WORKSPACE } from './types'

marked.setOptions({ gfm: true })

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
  // `@rumu:/card/<id>` / `@rumu:card/<id>`, optional `.md`.
  const prefix = `${INTERNAL_WORKSPACE}:`
  if (href.startsWith(prefix)) {
    const rest = href.slice(prefix.length).replace(/^\/?/, '')
    const m = /^card\/([^#?]+?)(?:\.md)?(?:[?#].*)?$/.exec(rest)
    if (m) return m[1]!
  }
  if (!currentCard) return null
  // Rendering a card: `/card/<id>` and relative links resolve inside the
  // card library, relative to the current card's directory.
  let m = /^\/card\/([^#?]+?)(?:\.md)?(?:[?#].*)?$/.exec(href)
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
    // Other workspace-qualified paths (`proj:src/a.rs`, `@rumu:/session/…`)
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
