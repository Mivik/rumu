// Transient UI state: toasts, the card drawer, the image lightbox, the
// composer's focus requests, and the message it is editing.

import { ref, shallowRef, watch } from 'vue'
import type { Attachment, Node, ToastKind } from '../types'
import { RpcError } from '../api'
import { blobUrl } from '../attachments'
import { mountedId, mountedRef } from '../cards'
import { state, type LightboxImage, type Toast } from './state'

/** Rising on every request, so the composer takes focus however often it is
 *  asked — even when it was already mounted (landing → landing). */
export const composerFocus = ref(0)

/** The sent user message the composer is editing, if any: one at a time,
 *  since the composer is the editor. Another session ends the edit. */
export const editTarget = shallowRef<Node | null>(null)

watch(
  () => state.sessionId,
  () => (editTarget.value = null),
)

/** A message to bring into view once its session shows it (a clicked
 *  `@rumu:/sessions/<s>/<node>` address): its id or a prefix.
 *  Opening another session drops it. */
export const focusTarget = shallowRef<{ session: string; node: string } | null>(null)

watch(
  () => state.sessionId,
  (id) => {
    if (focusTarget.value && !id?.startsWith(focusTarget.value.session)) focusTarget.value = null
  },
)

/** Put the caret in the composer: returning home from the sidebar should land
 *  the user in the writing box, not on the button they just clicked. */
export function requestComposerFocus(): void {
  composerFocus.value++
}

/** The open "stop the background work and go on?" popup: how many things
 *  hold the main line, the control the user acted on (it floats beside it),
 *  and how the asker learns the answer. */
export const stopConfirm = shallowRef<{
  count: number
  anchor: Element | null
  answer: (ok: boolean) => void
} | null>(null)

/** The button last pressed (or the focused element, for a key): what a
 *  `withStop` popup floats beside, so no caller has to pass it along. */
let lastControl: Element | null = null
if (typeof window !== 'undefined') {
  window.addEventListener(
    'pointerdown',
    (e) => (lastControl = (e.target as Element | null)?.closest('button, [role="button"]') ?? null),
    true,
  )
  window.addEventListener('keydown', () => (lastControl = document.activeElement), true)
}

/**
 * Run a request that moves the main line (`branch.move`, `branch.regenerate`,
 * `branch.rewind`). When background tasks or sub-agents of the part it
 * leaves hold it (`-32011`), ask beside the control that started it whether
 * to stop them, and send it again with `stop` if so. `undefined` when the
 * user said no; other failures throw.
 */
export async function withStop<T>(request: (stop: boolean) => Promise<T>): Promise<T | undefined> {
  const anchor = lastControl
  try {
    return await request(false)
  } catch (err) {
    if (!(err instanceof RpcError) || err.code !== -32011) throw err
    const count = (err.data as { tasks?: string[] } | undefined)?.tasks?.length || 1
    stopConfirm.value?.answer(false)
    const ok = await new Promise<boolean>((answer) => (stopConfirm.value = { count, anchor, answer }))
    stopConfirm.value = null
    return ok ? await request(true) : undefined
  }
}

let toastSeq = 1

export function toast(kind: ToastKind, message: string, detail?: string): void {
  const t: Toast = { id: toastSeq++, kind, message, detail }
  state.toasts.push(t)
  setTimeout(() => dismissToast(t.id), kind === 'error' ? 8000 : 4000)
}

export function dismissToast(id: number): void {
  const i = state.toasts.findIndex((t) => t.id === id)
  if (i >= 0) state.toasts.splice(i, 1)
}

// ---- card windows (shared UI state) ----

let windowSeq = 1

/** Open a mounted card (a chip's id) in a new window over the ones already
 *  showing. */
export function openDrawer(cardId: string, useRef?: string): void {
  const ref = mountedRef(cardId)
  state.drawers.push({
    key: windowSeq++,
    cardId,
    deck: ref?.deck ?? null,
    path: ref?.path || null,
    useRef,
  })
}

/** Open a card by its deck-local address in a new window: a body link inside
 *  a card keeps the card it was clicked in open below. */
export function openCardWindow(deck: string, path: string): void {
  state.drawers.push({ key: windowSeq++, cardId: mountedId(deck, path), deck, path })
}

/** Mark a window as closing: the one below it owns the scrim while it fades. */
export function beginDrawerClose(key: number): void {
  const window = state.drawers.find((w) => w.key === key)
  if (window) window.closing = true
}

/** Close one card window (by key), or every window when no key is given. */
export function closeDrawer(key?: number): void {
  state.drawers = key === undefined ? [] : state.drawers.filter((window) => window.key !== key)
}

/** Show an image full-size: a stored attachment, or a local preview (an
 *  object URL) of one still being attached. */
export function openLightbox(image: Attachment | LightboxImage): void {
  state.lightbox =
    'hash' in image
      ? { src: blobUrl(image.hash), name: image.name, width: image.width, height: image.height }
      : image
}

export function closeLightbox(): void {
  state.lightbox = null
}
