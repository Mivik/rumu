// Image attachments: the placement rule and the blob store.
//
// A content's text carries `[Image #N]` anchors (N 1-based into
// `attachments`). The first valid anchor of each attachment is replaced by
// its image; repeated, out-of-range or malformed anchors stay literal text,
// and attachments without an anchor follow all of the text, in order. This
// mirrors `Content::segments` in rumu-core exactly — the model projection,
// transcripts and this UI all follow it.
//
// The web composer itself never places anchors mid-text: like the Claude and
// ChatGPT web apps, a message's images lead it (`withLeadingAnchors`), and
// the draft the user types is plain text.

import type { Attachment, Content } from './types'

/** Where an attachment's bytes live: the content-addressed blob store. */
export function blobUrl(hash: string): string {
  return `/blobs/${hash}`
}

/** `text` behind the anchors of `count` images — the form every
 *  web-composed message takes. The anchors touch: the projection cuts the
 *  text at each one, and a whitespace-only piece between two images would be
 *  an empty text block (Anthropic rejects those). */
export function withLeadingAnchors(text: string, count: number): string {
  if (!count) return text
  const anchors = Array.from({ length: count }, (_, i) => `[Image #${i + 1}]`).join('')
  return text ? `${anchors}\n${text}` : anchors
}

/** The text a user typed: a message's text without the leading anchors the
 *  composer put there (`withLeadingAnchors`'s inverse). */
export function typedText(text: string, count: number): string {
  let rest = text
  for (let n = 1; n <= count; n++) {
    const anchor = `[Image #${n}]`
    if (!rest.startsWith(anchor)) break
    rest = rest.slice(anchor.length).replace(/^\n/, '')
  }
  return rest
}

const OPEN = '[Image #'

function isDigit(c: number): boolean {
  return c >= 0x30 && c <= 0x39
}

/** One piece of a content, in reading order: text, or an image — `n` is its
 *  1-based position, `anchored` false when its anchor was missing from the
 *  text and it follows the text instead. */
export type ContentSegment =
  { type: 'text'; text: string } | { type: 'image'; n: number; attachment: Attachment; anchored: boolean }

/** Split a content into text and images, in reading order (see the module
 *  comment for the rule). Never emits an empty text segment. */
export function contentSegments(content: Content): ContentSegment[] {
  const text = content.text
  const items = content.attachments ?? []
  const placed = new Array<boolean>(items.length).fill(false)
  const out: ContentSegment[] = []
  let last = 0
  let scan = 0
  for (;;) {
    const at = text.indexOf(OPEN, scan)
    if (at < 0) break
    const digits = at + OPEN.length
    let end = digits
    while (end < text.length && isDigit(text.charCodeAt(end))) end++
    const raw = text.slice(digits, end)
    const n = raw && !raw.startsWith('0') && text[end] === ']' ? Number(raw) : 0
    if (!(n >= 1 && n <= items.length && !placed[n - 1])) {
      scan = at + 1
      continue
    }
    if (last < at) out.push({ type: 'text', text: text.slice(last, at) })
    placed[n - 1] = true
    out.push({ type: 'image', n, attachment: items[n - 1]!, anchored: true })
    last = end + 1
    scan = last
  }
  if (last < text.length) out.push({ type: 'text', text: text.slice(last) })
  for (let i = 0; i < items.length; i++) {
    if (!placed[i]) out.push({ type: 'image', n: i + 1, attachment: items[i]!, anchored: false })
  }
  return out
}

/** An attachment as plain text — the form text-only readers (previews, the
 *  clipboard) see it in. */
export function attachmentPlaceholder(a: Attachment): string {
  return a.name ? `[image: ${a.name} ${a.width}×${a.height}]` : `[image: ${a.width}×${a.height}]`
}

/** The content with every image as its placeholder — `Content::rendered`. */
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

/**
 * Upload an image file to the blob store (`POST /blobs`, same origin). The
 * server normalizes it once (max 2000px edge, 20 MiB in) and answers with
 * the stored `Attachment`; a refusal is a 400 carrying `{error}`.
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
