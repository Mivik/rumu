// The composer's last pick, remembered in this browser so a new session
// starts where the previous one left off: the model, and the workspaces per
// assistant (an assistant without one starts on its deck's).

import { reactive, watch } from 'vue'
import type { ModelUse } from '../generated/ModelUse'

export interface WorkspacePick {
  /** Attached workspace names, in order. */
  attached: string[]
  /** The default among them (`null`: the first). */
  default: string | null
}

const KEY = 'rumu.composer'

interface Prefs {
  model: ModelUse | null
  workspaces: Record<string, WorkspacePick>
}

function load(): Partial<Prefs> {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? '{}') as Partial<Prefs>
  } catch {
    return {}
  }
}

export const prefs = reactive<Prefs>({ model: null, workspaces: {}, ...load() })

watch(prefs, (p) => localStorage.setItem(KEY, JSON.stringify(p)), { deep: true })
