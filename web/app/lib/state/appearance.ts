// How the app looks and behaves.
//
// The theme follows the system until the user picks one; the two folds are
// how the conversation shows its work — an edit's diff, and the steps a
// turn took. All of it belongs to this browser: the server never sees it,
// so a phone and a desk each read the way their owner likes.

import { reactive, watch } from 'vue'

/** The theme: the system's, or a fixed one. */
export type ThemeMode = 'system' | 'light' | 'dark'
export interface Appearance {
  theme: ThemeMode
  /** An edit's diff starts folded. */
  foldEdits: boolean
  /** The work steps fold once the answer is done. */
  foldWork: boolean
}

const KEY = 'rumu.appearance'

function load(): Partial<Appearance> {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? '{}') as Partial<Appearance>
    // Only the keys we know: a stale one is dropped.
    return { theme: raw.theme, foldEdits: raw.foldEdits, foldWork: raw.foldWork }
  } catch {
    return {}
  }
}

export const appearance = reactive<Appearance>({
  theme: 'system',
  foldEdits: false,
  foldWork: true,
  ...load(),
})

/** The theme in effect: the user's choice, or the system's. */
export function resolvedTheme(): 'light' | 'dark' {
  if (appearance.theme !== 'system') return appearance.theme
  return matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

/** Put the theme on <html>, where the tokens read it. */
export function applyAppearance(): void {
  document.documentElement.dataset.theme = resolvedTheme()
}

watch(
  appearance,
  () => {
    localStorage.setItem(KEY, JSON.stringify(appearance))
    applyAppearance()
  },
  { deep: true },
)

// The system's theme changes while the app is open: follow it.
matchMedia('(prefers-color-scheme: dark)').addEventListener('change', applyAppearance)
