// Responsive shell state: below the desktop breakpoints the side columns
// become overlays (drawers) opened from the chat header. Only one is open at
// a time; the shell closes them on Escape, on a scrim click, and when the
// viewport grows past the breakpoint that made them overlays.
import { reactive } from 'vue'

export const layout = reactive({
  sidebarOpen: false,
  inspectorOpen: false,
})

export function toggleSidebar(): void {
  layout.sidebarOpen = !layout.sidebarOpen
  if (layout.sidebarOpen) layout.inspectorOpen = false
}

export function toggleInspector(): void {
  layout.inspectorOpen = !layout.inspectorOpen
  if (layout.inspectorOpen) layout.sidebarOpen = false
}

export function closePanels(): void {
  layout.sidebarOpen = false
  layout.inspectorOpen = false
}
