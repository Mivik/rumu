// Drops, from anywhere in the app.
//
// A file dragged into the window is attached to the composer's buffer — the
// edit's, else the open session's draft — wherever it lands: the transcript,
// the sidebar, the library. The window answers the drag itself, so the
// browser never opens the file in the tab (which would take the app away),
// and the composer sees the drag as it passes over it.
//
// Only drags that carry files from outside are ours: the sidebar's and the
// library's own drags (sessions, cards) move text, and an image dragged off
// the page carries a file but started here — it is not an attachment.

import { ref } from 'vue'

/** A file drag is over the window. */
export const dragging = ref(false)

/** Where a dropped file goes: the composers register their buffers here, and
 *  the one mounted last (a sub-agent's, over the main one) takes the drop. */
const sinks: ((files: File[]) => void)[] = []

/** A drag started on this page is under way (`dragstart` fires only here). */
let fromPage = false

/** A composer tells us where the dropped files should land. The returned
 *  function unregisters it — only it: the composer under it takes over again. */
export function setDropSink(fn: (files: File[]) => void): () => void {
  sinks.push(fn)
  return () => {
    const i = sinks.lastIndexOf(fn)
    if (i >= 0) sinks.splice(i, 1)
  }
}

/** A drag began on this page, or ended. */
export function setDragFromPage(on: boolean): void {
  fromPage = on
}

/** Whether the drag carries files from outside the page. */
export function carriesFiles(e: DragEvent): boolean {
  return !fromPage && !!e.dataTransfer?.types?.includes('Files')
}

/** Hand the dropped files to the composer on top, if there is one. */
export function dropFiles(files: File[]): void {
  sinks.at(-1)?.(files)
}
