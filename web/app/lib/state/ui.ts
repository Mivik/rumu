// Transient UI state: toasts, the card drawer, the image lightbox, the
// composer's focus requests, and the message it is editing.

import { ref, shallowRef, watch } from 'vue'
import type { Attachment, Node, ToastKind } from '../types'
import { blobUrl } from '../attachments'
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

/** Put the caret in the composer: returning home from the sidebar should land
 *  the user in the writing box, not on the button they just clicked. */
export function requestComposerFocus(): void {
  composerFocus.value++
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
  const card = state.cards.find((c) => c.id === cardId)
  state.drawers.push({
    key: windowSeq++,
    cardId,
    deck: card?.deck ?? null,
    path: card?.path ?? null,
    useRef,
  })
}

/** Open a card by its deck-local address in a new window: a body link inside
 *  a card keeps the card it was clicked in open below. */
export function openCardWindow(deck: string, path: string): void {
  const card = state.cards.find((c) => c.deck === deck && c.path === path)
  state.drawers.push({ key: windowSeq++, cardId: card?.id ?? null, deck, path })
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
