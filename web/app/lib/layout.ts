// Responsive shell state: below the desktop breakpoint the sessions sidebar
// becomes an overlay (a drawer) opened from the sheet's header. The shell
// closes it on Escape, on a scrim click, and when the viewport grows past the
// breakpoint that made it an overlay. The management pages themselves are
// routes (router.ts); what is left here is the settings page's hand-offs.
import { onBeforeUnmount, reactive, watchEffect, type Ref } from 'vue'
import { t } from '../i18n'
import { openPage, pageUp } from './router'
import { state } from './state/state'

/** The desktop breakpoint (Tailwind's `lg`). */
export const WIDE = '(min-width: 64rem)'

export const layout = reactive({
  sidebarOpen: false,
  /** At the desktop breakpoint: the sidebar is static, and settings and an
   *  open project, workspace or assistant are dialogs over the desk. */
  wide: window.matchMedia(WIDE).matches,
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
  /** The open project, workspace or assistant form has unsaved edits. */
  itemDirty: false,
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

/** An item of the projects or assistants page is open as a dialog. */
export function itemDialog(): boolean {
  const p = state.page
  return layout.wide && !!p && (!!p.item || p.fresh) && p.page !== 'decks' && p.page !== 'settings'
}

/** For an item's form: publish its unsaved state while it is mounted. */
export function trackItemDirty(dirty: Ref<boolean>): void {
  watchEffect(() => (layout.itemDirty = dirty.value))
  onBeforeUnmount(() => (layout.itemDirty = false))
}

/** Leave the open item for its page's list, asking first when its form has
 *  unsaved edits. */
export function leaveItem(): void {
  if (!layout.itemDirty || window.confirm(t('common.discardPrompt'))) pageUp()
}

/** Escape closes the sidebar drawer, then the side panel. A card window is a
 *  native <dialog>: its own cancel handler closes it (asking about unsaved
 *  edits first), so leave Escape to it. */
export function closePanels(): void {
  if (state.drawers.length) return
  if (layout.sidebarOpen) layout.sidebarOpen = false
  else {
    state.artifact = null
    state.taskDetail = null
  }
}
