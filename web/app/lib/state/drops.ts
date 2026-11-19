// Drops, from anywhere in the app.
//
// A file dragged into the window is attached to the composer's buffer — the
// edit's, else the open session's draft — wherever it lands: the transcript,
// the sidebar, the library. The window answers the drag itself, so the
// browser never opens the file in the tab (which would take the app away),
// and the composer sees the drag as it passes over it.
//
// Only drags that carry files are ours: the sidebar's and the library's own
// drags (sessions, cards) move text, not files, and are left alone.

import { ref } from 'vue'

/** A file drag is over the window. */
export const dragging = ref(false)

/** Where a dropped file goes: the composer registers its buffer here. */
let sink: ((files: File[]) => void) | null = null

/** The composer tells us where the dropped files should land. */
export function setDropSink(fn: ((files: File[]) => void) | null): void {
  sink = fn
}

/** Whether the drag carries files (the app's own drags do not). */
export function carriesFiles(e: DragEvent): boolean {
  return !!e.dataTransfer?.types?.includes('Files')
}

/** Hand the dropped files to the composer, if it is there. */
export function dropFiles(files: File[]): void {
  sink?.(files)
}
