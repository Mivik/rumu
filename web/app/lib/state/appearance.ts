// How the app looks: the theme, and how much of a turn's work shows (what
// starts open). They live in the server's settings (`appearance`), so every
// device opening this rumu reads the same way; `system` still follows each
// device's own theme.
//
// The theme is also kept in this browser, so the head script in
// nuxt.config.ts puts it on <html> before the first paint, long before the
// settings arrive.

import { computed, inject, provide, ref, watch, type ComputedRef, type InjectionKey } from 'vue'
import type { Appearance } from '../generated/Appearance'
import type { Theme } from '../generated/Theme'
import type { WorkView } from '../generated/WorkView'
import { editSettings, models } from './models'

const THEME_KEY = 'rumu.theme'

/** The presets of the work view, the middle one the default. */
export const WORK_PRESETS = {
  quiet: {
    steps: 'folded',
    always_show_edits: false,
    thinking: 'folded',
    calls: 'folded',
    edits: 'folded',
    limit_edits: true,
  },
  standard: {
    steps: 'auto',
    always_show_edits: true,
    thinking: 'auto',
    calls: 'folded',
    edits: 'open',
    limit_edits: true,
  },
  verbose: {
    steps: 'open',
    always_show_edits: true,
    thinking: 'open',
    calls: 'auto',
    edits: 'open',
    limit_edits: false,
  },
} satisfies Record<string, WorkView>
export type WorkPreset = keyof typeof WORK_PRESETS

const DEFAULTS: Appearance = { theme: 'system', work: WORK_PRESETS.standard }

/** The settings in effect: the defaults until the server's arrive. */
export const appearance = computed<Appearance>(() => models.settings?.appearance ?? DEFAULTS)
export const work = computed(() => appearance.value.work)

/** The preset a work view matches, if any. */
export function presetOf(w: WorkView): WorkPreset | null {
  const hit = Object.entries(WORK_PRESETS).find(([, p]) =>
    (Object.keys(p) as (keyof WorkView)[]).every((k) => p[k] === w[k]),
  )
  return (hit?.[0] as WorkPreset | undefined) ?? null
}

export function sameWork(a: WorkView, b: WorkView): boolean {
  return (Object.keys(a) as (keyof WorkView)[]).every((k) => a[k] === b[k])
}

const WORK_KEY: InjectionKey<ComputedRef<WorkView>> = Symbol('work-view')

/** Show the conversation's components under them a work view other than
 *  the saved one (the settings' preview of an unapplied change). */
export function provideWorkView(view: ComputedRef<WorkView>): void {
  provide(WORK_KEY, view)
}

/** The work view a conversation component follows. */
export function useWorkView(): ComputedRef<WorkView> {
  return inject(WORK_KEY, work)
}

export function setAppearance(next: Appearance): Promise<boolean> {
  return editSettings('settings.set_appearance', { appearance: next })
}

function storedTheme(): Theme {
  const theme = localStorage.getItem(THEME_KEY)
  return theme === 'light' || theme === 'dark' ? theme : 'system'
}

let systemDark: MediaQueryList

/** A theme the settings section is trying on, not yet applied. */
export const themeDraft = ref<Theme | null>(null)

/** Put the theme on <html>, where the tokens in main.css read it, and on
 *  the browser's own chrome (the `theme-color` metas). */
function applyTheme(): void {
  const saved = models.settings?.appearance.theme ?? storedTheme()
  localStorage.setItem(THEME_KEY, saved)
  const theme = themeDraft.value ?? saved
  const dark = theme === 'dark' || (theme === 'system' && systemDark.matches)
  document.documentElement.dataset.theme = dark ? 'dark' : 'light'
  for (const m of document.querySelectorAll<HTMLMetaElement>('meta[name="theme-color"]')) {
    const scheme = (m.dataset.scheme ??= m.media.includes('dark') ? 'dark' : 'light')
    m.media = theme === 'system' ? `(prefers-color-scheme: ${scheme})` : theme === scheme ? 'all' : 'not all'
  }
}

/** Follow the setting, and the system's theme while it is the one in use. */
export function followTheme(): void {
  systemDark = matchMedia('(prefers-color-scheme: dark)')
  systemDark.addEventListener('change', applyTheme)
  watch([() => models.settings?.appearance.theme, themeDraft], applyTheme, { immediate: true })
}
