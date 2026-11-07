// Responsive shell state: below the desktop breakpoint the sessions sidebar
// becomes an overlay (a drawer) opened from the chat header. The shell
// closes it on Escape, on a scrim click, and when the viewport grows past the
// breakpoint that made it an overlay.
import { reactive } from 'vue'
import { state } from './state/state'

export const layout = reactive({
  sidebarOpen: false,
  /** The assistant config dialog (opened from the sidebar roster). */
  assistantSettingsOpen: false,
  /** The assistant the dialog opens on (`null`: the one in view). */
  assistantSettingsFor: null as string | null,
  /** The workspace registry dialog. */
  workspaceSettingsOpen: false,
  /** The workspace it opens on (`''`: a new one). */
  workspaceSettingsFor: '' as string,
  /** A new workspace from the composer also joins the open branch. */
  workspaceSettingsAttach: false,
})

export function toggleSidebar(): void {
  layout.sidebarOpen = !layout.sidebarOpen
}

/** Open the assistant dialog, on `id` when given. */
export function openAssistantSettings(id: string | null = null): void {
  layout.assistantSettingsFor = id
  layout.assistantSettingsOpen = true
}

export function closeAssistantSettings(): void {
  layout.assistantSettingsOpen = false
}

/** Open the workspace dialog on `name` (`''`: a new workspace, which joins
 *  the open branch when `attach`). */
export function openWorkspaceSettings(name = '', attach = false): void {
  layout.workspaceSettingsFor = name
  layout.workspaceSettingsAttach = attach
  layout.workspaceSettingsOpen = true
}

export function closeWorkspaceSettings(): void {
  layout.workspaceSettingsOpen = false
}

/** Escape: close the topmost layer. The library may sit over the assistant
 *  dialog (a deck link opens it there), so it goes first, on its own. */
export function closePanels(): void {
  if (state.library.isOpen) {
    state.library.isOpen = false
    state.library.open = null
    state.library.focusDeck = null
    return
  }
  layout.sidebarOpen = false
  layout.assistantSettingsOpen = false
  layout.workspaceSettingsOpen = false
}
