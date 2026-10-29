// Responsive shell state: below the desktop breakpoint the sessions sidebar
// becomes an overlay (a drawer) opened from the chat header. The shell
// closes it on Escape, on a scrim click, and when the viewport grows past the
// breakpoint that made it an overlay.
import { reactive } from 'vue'

export const layout = reactive({
  sidebarOpen: false,
})

export function toggleSidebar(): void {
  layout.sidebarOpen = !layout.sidebarOpen
}

export function closePanels(): void {
  layout.sidebarOpen = false
}
