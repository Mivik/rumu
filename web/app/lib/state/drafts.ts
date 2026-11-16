// The composer's unsent writing, one draft per session and one for the
// landing page (key `''`), so switching sessions neither carries a draft
// along nor drops it. Drafts outlive the composer (landing and chat mount
// different ones) and, in this browser, a reload: the text and the uploaded
// images go to localStorage; a file still uploading or failed does not
// survive one. A session that is gone takes its draft with it.

import { computed, reactive, watch } from 'vue'
import type { Attachment } from '../types'
import { blobUrl } from '../attachments'
import { state } from './state'

/** One image being attached: the file, its local preview, and where its
 * upload stands. An image the edited message already carries has no file. */
export interface PendingImage {
  id: number
  file?: File
  name: string
  /** Object URL of the file's bytes, or the stored blob, for the tray's preview. */
  url: string
  status: 'uploading' | 'done' | 'failed'
  attachment?: Attachment
  error?: string
  ctrl: AbortController
}

export interface Draft {
  text: string
  images: PendingImage[]
}

interface Stored {
  text: string
  images: { name: string; attachment: Attachment }[]
}

const KEY = 'rumu.drafts'

let imageSeq = 0

export function nextImageId(): number {
  return ++imageSeq
}

/** A tile for an image already in the blob store. */
export function storedImage(attachment: Attachment, name = attachment.name ?? ''): PendingImage {
  return reactive<PendingImage>({
    id: nextImageId(),
    name,
    url: blobUrl(attachment.hash),
    status: 'done',
    attachment,
    ctrl: new AbortController(),
  })
}

/** Drop a tile: stop its upload and free its local preview. */
export function releaseImage(item: PendingImage): void {
  item.ctrl.abort()
  if (item.file) URL.revokeObjectURL(item.url)
}

function load(): Record<string, Draft> {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? '{}') as Record<string, Stored>
    return Object.fromEntries(
      Object.entries(raw).map(([k, d]) => [
        k,
        { text: d.text, images: d.images.map((i) => storedImage(i.attachment, i.name)) },
      ]),
    )
  } catch {
    return {}
  }
}

const drafts = reactive<Record<string, Draft>>(load())

export function draftFor(key: string): Draft {
  return (drafts[key] ??= reactive({ text: '', images: [] }))
}

const keyOf = (session: string | null): string => session ?? ''

// The open session's draft exists before anything reads it.
watch(() => keyOf(state.sessionId), draftFor, { immediate: true, flush: 'sync' })

/** The open session's draft, or the landing page's. */
export const activeDraft = computed(() => drafts[keyOf(state.sessionId)]!)

function stored(d: Draft): Stored | null {
  const images = d.images
    .filter((i) => i.status === 'done')
    .map((i) => ({ name: i.name, attachment: i.attachment! }))
  return d.text.trim() || images.length ? { text: d.text, images } : null
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
  for (const item of d.images) releaseImage(item)
  delete drafts[session]
}

/** Keep only the landing draft and those of `sessions`, the full list. */
export function pruneDrafts(sessions: readonly { id: string }[]): void {
  const live = new Set(sessions.map((s) => s.id))
  for (const key of Object.keys(drafts)) {
    if (key && !live.has(key)) dropDraft(key)
  }
}
