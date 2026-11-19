<script setup lang="ts">
// The app's answer to a file drag.
//
// A file dragged into the window is attached to the composer's buffer — the
// edit's, else the open session's draft — wherever it lands: the
// transcript, the sidebar, the library. The window takes the drag itself, so
// the browser never opens the file in the tab (which would take the app
// away), and the outline says the drop is ours.
//
// Only drags that carry files are ours: the sidebar's and the library's own
// drags (sessions, cards) move text, not files, and are left alone.
import { onBeforeUnmount, onMounted } from 'vue'
import { t } from '../i18n'
import { carriesFiles, dragging, dropFiles } from '../lib/state/drops'

// `dragenter` and `dragleave` fire for every element the pointer crosses, so
// the depth is counted: the outline clears when the drag really leaves.
let depth = 0

function onDragenter(e: DragEvent): void {
  if (!carriesFiles(e)) return
  e.preventDefault()
  depth += 1
  dragging.value = true
}

function onDragover(e: DragEvent): void {
  // Without this the browser opens the file in the tab instead of dropping
  // it here.
  if (!carriesFiles(e)) return
  e.preventDefault()
}

function onDragleave(e: DragEvent): void {
  if (!carriesFiles(e)) return
  // Leaving the window has no element to enter.
  if (!e.relatedTarget) depth = 0
  else depth = Math.max(0, depth - 1)
  if (!depth) dragging.value = false
}

function onDrop(e: DragEvent): void {
  if (!carriesFiles(e)) return
  e.preventDefault()
  depth = 0
  dragging.value = false
  dropFiles([...(e.dataTransfer?.files ?? [])])
}

/** The drag ended where it started, or was cancelled: the outline goes. */
function onDragend(): void {
  depth = 0
  dragging.value = false
}

onMounted(() => {
  window.addEventListener('dragenter', onDragenter)
  window.addEventListener('dragover', onDragover)
  window.addEventListener('dragleave', onDragleave)
  window.addEventListener('drop', onDrop)
  window.addEventListener('dragend', onDragend)
})

onBeforeUnmount(() => {
  window.removeEventListener('dragenter', onDragenter)
  window.removeEventListener('dragover', onDragover)
  window.removeEventListener('dragleave', onDragleave)
  window.removeEventListener('drop', onDrop)
  window.removeEventListener('dragend', onDragend)
})
</script>

<template>
  <!-- the outline sits over the app but never over a pointer: the drop must
       reach the window -->
  <div v-if="dragging" class="pointer-events-none fixed inset-0 z-60">
    <div class="absolute inset-3 rounded-sheet border-2 border-dashed border-brand/40" />
    <p
      class="absolute inset-x-0 top-1/2 mx-auto w-fit -translate-y-1/2 rounded-sheet border border-line bg-surface px-4 py-2 text-message text-ink shadow-soft"
    >
      {{ t('chat.drop.hint') }}
    </p>
  </div>
</template>
