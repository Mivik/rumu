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
  /** The project settings dialog. */
  projectSettingsOpen: false,
  /** The project it opens on. */
  projectSettingsFor: '' as string,
  /** The project dialog's add-folder dialog, over it. */
  projectFolderOpen: false,
  /** The settings dialog. */
  settingsOpen: false,
  settingsSection: 'profiles' as 'profiles' | 'providers',
  /** Set to open the providers section at adding one; taken by it. */
  addingProvider: false,
  /** Set to open the providers section at a model (`provider/model-id`);
   *  taken by it. */
  showingModel: null as string | null,
  /** The open section's page, when below the section's own: its crumbs,
   *  each but the last going back to its page. */
  settingsTrail: [] as { label: string; go?: () => void }[],
  /** A settings edit in flight: the dialog stays on its page until done. */
  settingsBusy: false,
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

/** Open the workspace dialog on `name` (`''`: a new workspace). */
export function openWorkspaceSettings(name = ''): void {
  layout.workspaceSettingsFor = name
  layout.workspaceSettingsOpen = true
}

export function closeWorkspaceSettings(): void {
  layout.workspaceSettingsOpen = false
  closeSettings()
}

/** Open the project settings dialog on `id`. */
export function openProjectSettings(id: string): void {
  layout.projectSettingsFor = id
  layout.projectSettingsOpen = true
}

export function closeProjectSettings(): void {
  layout.projectSettingsOpen = false
  layout.projectFolderOpen = false
  closeSettings()
}

export function openSettings(): void {
  layout.settingsOpen = true
}

/** Settings, at adding a provider. */
export function addProvider(): void {
  layout.settingsSection = 'providers'
  layout.addingProvider = true
  layout.settingsOpen = true
}

/** Settings, at a model's provider, searched for the model. */
export function showModel(ref: string): void {
  layout.settingsSection = 'providers'
  layout.showingModel = ref
  layout.settingsOpen = true
}

export function closeSettings(): void {
  if (layout.settingsBusy) return
  layout.settingsOpen = false
}

/** Escape: close the topmost layer. The library may sit over the assistant
 *  dialog (a deck link opens it there), so it goes first, on its own. */
export function closePanels(): void {
  // A card window is a native <dialog>: its own cancel handler closes it
  // (asking about unsaved edits first), so leave Escape to it.
  if (state.drawers.length) return
  if (state.library.isOpen) {
    state.library.isOpen = false
    state.library.open = null
    state.library.focusDeck = null
    return
  }
  if (layout.projectFolderOpen) {
    layout.projectFolderOpen = false
    return
  }
  layout.sidebarOpen = false
  layout.assistantSettingsOpen = false
  layout.workspaceSettingsOpen = false
  layout.projectSettingsOpen = false
  closeSettings()
}
