// Attachments: images and text documents — the placement rule and the blob
// store.
//
// A content's text carries anchors: `[Image #N]` names the N-th image and
// `[Document #N]` the N-th document, each counted within its own kind
// (1-based). The first valid anchor of an attachment places it; repeated,
// out-of-range or malformed anchors stay literal text, and so does an anchor
// whose kind has no such attachment. An attachment with no anchor follows all
// of the text, in order. This mirrors `Content::segments` in rumu-core
// exactly — the model projection, transcripts and this UI all follow it.
//
// The web composer itself never places anchors mid-text: like the Claude and
// ChatGPT web apps, a message's attachments lead it (`withLeadingAnchors`),
// and the draft the user types is plain text.

import type { Attachment, AttachmentKind, Content } from './types'

/** Where an attachment's bytes live: the content-addressed blob store. */
export function blobUrl(hash: string): string {
  return `/blobs/${hash}`
}

/** The short extension of the common kinds — the kernel's table
 *  (`Attachment::scratch_path`), so a document lands in `@scratch` under the
 *  name a tool expects (`notes.md` → `…md`). Anything else keeps the MIME
 *  subtype as it was. */
const EXTENSIONS = new Map<string, string>([
  ['text/markdown', 'md'],
  ['text/plain', 'txt'],
  ['text/html', 'html'],
  ['text/css', 'css'],
  ['text/javascript', 'js'],
  ['text/typescript', 'ts'],
  ['text/csv', 'csv'],
  ['text/tab-separated-values', 'tsv'],
  ['text/x-python', 'py'],
  ['text/x-rust', 'rs'],
  ['text/x-shellscript', 'sh'],
  ['text/x-diff', 'diff'],
  ['application/json', 'json'],
  ['application/xml', 'xml'],
  ['application/yaml', 'yaml'],
  ['application/toml', 'toml'],
  ['application/sql', 'sql'],
  ['image/jpeg', 'jpg'],
])

/** Where a prompt's attachment (or an MCP result's) is copied in the
 *  session's `@scratch`: `.inbox/attachments/<hash prefix>.<ext>`. Mirrors
 *  `Attachment::scratch_path` in rumu-core. */
export function scratchPath(a: Attachment): string {
  const slash = a.mime.indexOf('/')
  const sub = slash < 0 ? 'bin' : a.mime.slice(slash + 1)
  const ext = EXTENSIONS.get(a.mime) ?? sub
  return `.inbox/attachments/${a.hash.slice(0, 12)}.${ext}`
}

/** The kind of an attachment; records written before kinds existed are
 *  images. */
export function kindOf(a: Attachment): AttachmentKind {
  return a.kind ?? 'image'
}

/** The kinds an anchor can name. */
export type AnchorKind = 'image' | 'document'

/** The in-text anchor of the `n`-th attachment of a kind: `[Image #3]`. */
function anchorOf(kind: AnchorKind, n: number): string {
  return kind === 'document' ? `[Document #${n}]` : `[Image #${n}]`
}

/** The anchors a message leads with: one per attachment, in attachment
 *  order, each numbered within its own kind (`[Image #1][Document #1]
 *  [Image #2]`). */
function leadingAnchors(attachments: Attachment[]): string[] {
  const counts: Record<AnchorKind, number> = { image: 0, document: 0 }
  const out: string[] = []
  for (const a of attachments) {
    const kind = kindOf(a)
    if (kind !== 'image' && kind !== 'document') continue
    counts[kind] += 1
    out.push(anchorOf(kind, counts[kind]))
  }
  return out
}

/** The text behind the anchors of `attachments` — the form every
 *  web-composed message takes. The anchors touch: the projection cuts the
 *  text at each one, and a whitespace-only piece between two attachments
 *  would be an empty text block (Anthropic rejects those). */
export function withLeadingAnchors(text: string, attachments: Attachment[]): string {
  const anchors = leadingAnchors(attachments)
  if (!anchors.length) return text
  const head = anchors.join('')
  return text ? `${head}\n${text}` : head
}

/** The text a user typed: a message's text without the leading anchors the
 *  composer put there (`withLeadingAnchors`'s inverse). */
export function typedText(text: string, attachments: Attachment[]): string {
  let rest = text
  for (const anchor of leadingAnchors(attachments)) {
    if (!rest.startsWith(anchor)) break
    rest = rest.slice(anchor.length).replace(/^\n/, '')
  }
  return rest
}

/** The anchors a text may carry, and the kind each one names. */
const ANCHORS: { open: string; kind: AnchorKind }[] = [
  { open: '[Image #', kind: 'image' },
  { open: '[Document #', kind: 'document' },
]

function isDigit(c: number): boolean {
  return c >= 0x30 && c <= 0x39
}

/** One piece of a content, in reading order: text, or an attachment — `n`
 *  is its 1-based number among the attachments of its kind, `anchored` false
 *  when its anchor was missing from the text and it follows the text
 *  instead. */
export type ContentSegment =
  | { type: 'text'; text: string }
  | { type: 'attachment'; n: number; attachment: Attachment; anchored: boolean }

/** Split a content into text and attachments, in reading order (see the
 *  module comment for the rule). Never emits an empty text segment. */
export function contentSegments(content: Content): ContentSegment[] {
  const text = content.text
  const items = content.attachments ?? []
  // The attachments of each kind, in order: the numbers an anchor counts.
  const groups = new Map<AttachmentKind, number[]>()
  items.forEach((a, i) => {
    const kind = kindOf(a)
    const group = groups.get(kind)
    if (group) group.push(i)
    else groups.set(kind, [i])
  })
  const placed = new Set<number>()
  const out: ContentSegment[] = []
  let last = 0
  let scan = 0
  for (;;) {
    // The next anchor of either kind.
    let at = -1
    let open = ''
    let kind: AnchorKind | null = null
    for (const a of ANCHORS) {
      const i = text.indexOf(a.open, scan)
      if (i < 0) continue
      if (at < 0 || i < at) {
        at = i
        open = a.open
        kind = a.kind
      }
    }
    if (at < 0 || kind === null) break
    const digits = at + open.length
    let end = digits
    while (end < text.length && isDigit(text.charCodeAt(end))) end++
    const raw = text.slice(digits, end)
    const n = raw && !raw.startsWith('0') && text[end] === ']' ? Number(raw) : 0
    const group = groups.get(kind) ?? []
    const index = n >= 1 && n <= group.length ? group[n - 1]! : -1
    // A repeated, out-of-range or malformed anchor stays literal text.
    if (index < 0 || placed.has(index)) {
      scan = at + 1
      continue
    }
    if (last < at) out.push({ type: 'text', text: text.slice(last, at) })
    placed.add(index)
    out.push({ type: 'attachment', n, attachment: items[index]!, anchored: true })
    last = end + 1
    scan = last
  }
  if (last < text.length) out.push({ type: 'text', text: text.slice(last) })
  // An attachment with no anchor follows all of the text, in order.
  for (let i = 0; i < items.length; i++) {
    if (placed.has(i)) continue
    const group = groups.get(kindOf(items[i]!)) ?? []
    out.push({
      type: 'attachment',
      n: group.indexOf(i) + 1,
      attachment: items[i]!,
      anchored: false,
    })
  }
  return out
}

/** An attachment as plain text — the form text-only readers (previews, the
 *  clipboard) see it in. A document has no pixels, so it is named alone. */
export function attachmentPlaceholder(a: Attachment): string {
  const kind = kindOf(a)
  if (kind === 'document') return a.name ? `[document: ${a.name}]` : '[document]'
  const at = a.name ? `${a.name} ${a.width}×${a.height}` : `${a.width}×${a.height}`
  return `[${kind}: ${at}]`
}

/** The content with every attachment as its placeholder — `Content::rendered`. */
export function renderedContent(content: Content | undefined): string {
  if (!content?.attachments?.length) return content?.text ?? ''
  let out = ''
  for (const seg of contentSegments(content)) {
    if (seg.type === 'text') {
      out += seg.text
    } else {
      if (!seg.anchored && out !== '' && !out.endsWith('\n')) out += '\n'
      out += attachmentPlaceholder(seg.attachment)
    }
  }
  return out
}

/** The MIME types a browser may hand over for a text document. The server
 *  decides from the bytes (UTF-8, no NUL); this is the client's guess, so a
 *  file may still be refused. */
const DOCUMENT_MIME = new Set([
  'application/json',
  'application/ld+json',
  'application/xml',
  'application/xhtml+xml',
  'application/yaml',
  'application/x-yaml',
  'application/toml',
  'application/sql',
  'application/javascript',
  'application/x-javascript',
  'application/ecmascript',
  'application/typescript',
  'application/x-sh',
  'application/x-shellscript',
  'application/x-python',
  'application/x-rust',
  'application/x-diff',
  'application/x-patch',
  'application/graphql',
])

/** The names a document's type is read from (`document_mime` in the
 *  kernel). */
const DOCUMENT_NAME = new Set([
  'md',
  'markdown',
  'txt',
  'text',
  'log',
  'json',
  'jsonl',
  'yaml',
  'yml',
  'toml',
  'xml',
  'csv',
  'tsv',
  'html',
  'htm',
  'css',
  'js',
  'mjs',
  'cjs',
  'ts',
  'tsx',
  'jsx',
  'py',
  'rs',
  'sh',
  'bash',
  'zsh',
  'sql',
  'diff',
  'patch',
  'ini',
  'conf',
  'cfg',
])

/** Whether a file is a text document: what the server would store as it is.
 *  A file with no type of its own is judged by its name. */
export function isDocument(file: File): boolean {
  const type = file.type.toLowerCase()
  if (type.startsWith('text/')) return true
  if (DOCUMENT_MIME.has(type)) return true
  if (!type || type === 'application/octet-stream')
    return DOCUMENT_NAME.has(file.name.split('.').pop()?.toLowerCase() ?? '')
  return false
}

/** The names an image's type is read from, for a file the browser hands
 *  over without a type of its own. */
const IMAGE_NAME = new Set([
  'png',
  'jpg',
  'jpeg',
  'gif',
  'webp',
  'svg',
  'bmp',
  'avif',
  'heic',
  'heif',
  'tiff',
  'ico',
])

/** Whether a file is an image: a browser's type, or a name that says so. */
export function isImage(file: File): boolean {
  const type = file.type.toLowerCase()
  if (type.startsWith('image/')) return true
  if (!type || type === 'application/octet-stream')
    return IMAGE_NAME.has(file.name.split('.').pop()?.toLowerCase() ?? '')
  return false
}

/** What the file picker offers: images, and the text kinds a document can
 *  be. */
export const ACCEPT_FILES =
  'image/*,text/*,.md,.markdown,.txt,.log,.json,.yaml,.yml,.toml,.xml,.csv,.tsv,.html,.htm,.css,.js,.mjs,.cjs,.ts,.tsx,.py,.rs,.sh,.bash,.sql,.diff,.patch'

/**
 * Upload a file to the blob store (`POST /blobs`, same origin). The server
 * sniffs the bytes: an image is normalized once (max 2000px edge, 20 MiB in)
 * and a text document is stored as it is; the JSON `Attachment` naming the
 * stored blob comes back. A refusal is a 400 carrying `{error}`.
 */
export async function uploadBlob(file: File, signal?: AbortSignal): Promise<Attachment> {
  const res = await fetch(`/blobs?name=${encodeURIComponent(file.name)}`, {
    method: 'POST',
    headers: { 'Content-Type': file.type || 'application/octet-stream' },
    body: file,
    signal,
  })
  const data = (await res.json().catch(() => null)) as (Partial<Attachment> & { error?: string }) | null
  if (!res.ok) throw new Error(data?.error || `${res.status} ${res.statusText}`.trim() || 'upload failed')
  // A document has no pixels: zero is a valid width and height.
  if (
    !data ||
    typeof data.hash !== 'string' ||
    typeof data.width !== 'number' ||
    typeof data.height !== 'number'
  ) {
    throw new Error('malformed upload response')
  }
  return data as Attachment
}
