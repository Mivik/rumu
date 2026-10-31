// Transient UI state: toasts, the card catalog and drawer, the image
// lightbox, and timeline focus requests.

import type { Attachment, ToastKind } from '../types'
import { blobUrl } from '../attachments'
import { state, type LightboxImage, type Toast } from './state'

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

// ---- card catalog + drawer + timeline focus (shared UI state) ----

export function openCatalog(): void {
  state.catalogOpen = true
}

export function closeCatalog(): void {
  state.catalogOpen = false
}

export function openDrawer(cardId: string, useNodeId?: string): void {
  state.drawer = { cardId, useNodeId }
}

export function closeDrawer(): void {
  state.drawer = null
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

let focusSeq = 0

export function focusNode(nodeId: string): void {
  state.focus = { nodeId, seq: ++focusSeq }
}
