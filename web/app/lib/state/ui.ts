// Transient UI state: toasts, the card drawer, and timeline focus requests.

import type { ToastKind } from '../types'
import { state, type Toast } from './state'

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

// ---- card drawer + timeline focus (shared UI state) ----

export function openDrawer(cardId: string, useNodeId?: string): void {
  state.drawer = { cardId, useNodeId }
}

export function closeDrawer(): void {
  state.drawer = null
}

let focusSeq = 0

export function focusNode(nodeId: string): void {
  state.focus = { nodeId, seq: ++focusSeq }
}
