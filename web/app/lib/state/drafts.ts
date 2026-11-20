// The composer's unsent writing, one draft per session and one for the
// landing page (key `''`), so switching sessions neither carries a draft
// along nor drops it. Drafts outlive the composer (landing and chat mount
// different ones) and, in this browser, a reload: the text and the uploaded
// attachments go to localStorage; a file still uploading or failed does not
// survive one. A session that is gone takes its draft with it.

import { computed, reactive, watch } from 'vue'
import type { Attachment } from '../types'
import { blobUrl } from '../attachments'
import { state } from './state'

/** One attachment waiting to be sent: an image or a text document. The file,
 *  its local preview, and where its upload stands. An attachment the edited
 *  message already carries has no file. */
export interface PendingAttachment {
  id: number
  file?: File
  name: string
  /** Object URL of the file's bytes (an image, as a picture), or the stored
   *  blob; a document has no picture and shows as a chip. */
  url: string
  status: 'uploading' | 'done' | 'failed'
  attachment?: Attachment
  error?: string
  ctrl: AbortController
}

export interface Draft {
  text: string
  attachments: PendingAttachment[]
}

interface Stored {
  text: string
  attachments?: { name: string; attachment: Attachment }[]
  /** The name `attachments` had before documents; a draft from then still
   *  comes back. */
  images?: { name: string; attachment: Attachment }[]
}

const KEY = 'rumu.drafts'

let attachmentSeq = 0

export function nextAttachmentId(): number {
  return ++attachmentSeq
}

/** A tile for an attachment already in the blob store. */
export function storedAttachment(attachment: Attachment, name = attachment.name ?? ''): PendingAttachment {
  return reactive<PendingAttachment>({
    id: nextAttachmentId(),
    name,
    url: blobUrl(attachment.hash),
    status: 'done',
    attachment,
    ctrl: new AbortController(),
  })
}

/** Drop a tile: stop its upload and free its local preview. */
export function releaseAttachment(item: PendingAttachment): void {
  item.ctrl.abort()
  if (item.file && item.url) URL.revokeObjectURL(item.url)
}

function load(): Record<string, Draft> {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? '{}') as Record<string, Stored>
    return Object.fromEntries(
      Object.entries(raw).map(([k, d]) => [
        k,
        {
          text: d.text,
          attachments: (d.attachments ?? d.images ?? []).map((i) => storedAttachment(i.attachment, i.name)),
        },
      ]),
    )
  } catch {
    return {}
  }
}

const drafts = reactive<Record<string, Draft>>(load())

export function draftFor(key: string): Draft {
  return (drafts[key] ??= reactive({ text: '', attachments: [] }))
}

const keyOf = (session: string | null): string => session ?? ''

// The open session's draft exists before anything reads it.
watch(() => keyOf(state.sessionId), draftFor, { immediate: true, flush: 'sync' })

/** The open session's draft, or the landing page's. */
export const activeDraft = computed(() => drafts[keyOf(state.sessionId)]!)

/** The new session written but not sent yet: it stays in the sidebar, and in
 *  its project, while other sessions are open. */
export const landingWritten = computed(() => {
  const d = drafts['']
  return !!d && (d.text.trim() !== '' || d.attachments.length > 0)
})

function stored(d: Draft): Stored | null {
  const attachments = d.attachments
    .filter((i) => i.status === 'done')
    .map((i) => ({ name: i.name, attachment: i.attachment! }))
  return d.text.trim() || attachments.length ? { text: d.text, attachments } : null
}

let saveTimer: ReturnType<typeof setTimeout> | undefined

function save(): void {
  clearTimeout(saveTimer)
  saveTimer = undefined
  const out: Record<string, Stored> = {}
  for (const [k, d] of Object.entries(drafts)) {
    const s = stored(d)
    if (s) out[k] = s
  }
  if (Object.keys(out).length) localStorage.setItem(KEY, JSON.stringify(out))
  else localStorage.removeItem(KEY)
}

watch(drafts, () => {
  clearTimeout(saveTimer)
  saveTimer = setTimeout(save, 300)
})
// A reload within the debounce keeps the last keystrokes.
addEventListener('pagehide', () => saveTimer !== undefined && save())

/** Forget the draft of a session that is gone. */
export function dropDraft(session: string): void {
  const d = drafts[session]
  if (!d || session === keyOf(state.sessionId)) return
  for (const item of d.attachments) releaseAttachment(item)
  delete drafts[session]
}

/** Keep only the landing draft and those of `sessions`, the full list. */
export function pruneDrafts(sessions: readonly { id: string }[]): void {
  const live = new Set(sessions.map((s) => s.id))
  for (const key of Object.keys(drafts)) {
    if (key && !live.has(key)) dropDraft(key)
  }
}
