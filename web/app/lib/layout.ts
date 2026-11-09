// Responsive shell state: below the desktop breakpoint the sessions sidebar
// becomes an overlay (a drawer) opened from the sheet's header. The shell
// closes it on Escape, on a scrim click, and when the viewport grows past the
// breakpoint that made it an overlay. The management pages themselves are
// routes (router.ts); what is left here is the settings page's hand-offs.
import { reactive } from 'vue'
import { openPage } from './router'
import { state } from './state/state'

export const layout = reactive({
  sidebarOpen: false,
  /** Set to open the providers section at adding one; taken by it. */
  addingProvider: false,
  /** Set to open the providers section at a model (`provider/model-id`);
   *  taken by it. */
  showingModel: null as string | null,
  /** The open section's page, when below the section's own: its crumbs,
   *  each but the last going back to its page. */
  settingsTrail: [] as { label: string; go?: () => void }[],
  /** A settings edit in flight: the section stays on its page until done. */
  settingsBusy: false,
})

export function toggleSidebar(): void {
  layout.sidebarOpen = !layout.sidebarOpen
}

/** Settings, at adding a provider. */
export function addProvider(): void {
  layout.addingProvider = true
  openPage('settings', 'providers')
}

/** Settings, at a model's provider, searched for the model. */
export function showModel(ref: string): void {
  layout.showingModel = ref
  openPage('settings', 'providers')
}

/** Escape closes the sidebar drawer. A card window is a native <dialog>: its
 *  own cancel handler closes it (asking about unsaved edits first), so leave
 *  Escape to it. */
export function closePanels(): void {
  if (state.drawers.length) return
  layout.sidebarOpen = false
}
