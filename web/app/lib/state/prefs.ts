// The composer's last pick, remembered in this browser so a new session
// starts where the previous one left off: the model, the project last in
// view, the assistant last used in each project, and the sidebar groups left
// collapsed.

import { reactive, watch } from 'vue'
import type { ModelUse } from '../generated/ModelUse'

const KEY = 'rumu.composer'

interface Prefs {
  model: ModelUse | null
  /** The project whose group was last in view (`null`: no project). */
  lastProject: string | null
  /** The assistant last used in each project (`''` is the no-project group). */
  assistantByProject: Record<string, string>
  /** Collapsed sidebar groups, by project id (`''` is the no-project group). */
  collapsedProjects: Record<string, boolean>
}

function load(): Partial<Prefs> {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? '{}') as Partial<Prefs>
  } catch {
    return {}
  }
}

export const prefs = reactive<Prefs>({
  model: null,
  lastProject: null,
  assistantByProject: {},
  collapsedProjects: {},
  ...load(),
})

watch(prefs, (p) => localStorage.setItem(KEY, JSON.stringify(p)), { deep: true })
