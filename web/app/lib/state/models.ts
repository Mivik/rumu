// Model settings (`settings.*`) and the composer's model pick. A branch runs
// on its last `model.use` marker, or the default profile. A pick made in
// the composer stays pending until the next prompt carries it; the server
// records it only when it changes the branch's model.

import { reactive } from 'vue'
import { t } from '../../i18n'
import { rumu } from '../api'
import type { Effort } from '../generated/Effort'
import type { ModelInfo } from '../generated/ModelInfo'
import type { ModelUse } from '../generated/ModelUse'
import type { Protocol } from '../generated/Protocol'
import type { SettingsInfo } from '../generated/SettingsInfo'
import { state } from './state'
import { toast } from './ui'

/** The protocols, in display order. */
export const PROTOCOLS: Protocol[] = ['anthropic', 'openai', 'openai_responses', 'gemini']

/** Tried after a model's native protocol, in order. */
const FALLBACK: Protocol[] = ['openai', 'anthropic', 'openai_responses', 'gemini']

export const PROTOCOL_LABELS: Record<Protocol, string> = {
  anthropic: 'Anthropic',
  openai: 'OpenAI',
  openai_responses: 'OpenAI Responses',
  gemini: 'Gemini',
}

export const models = reactive({
  settings: null as SettingsInfo | null,
  /** Pending picks by branch id; `''` is the landing composer's. */
  pending: {} as Record<string, ModelUse>,
})

/** Does a model id rule (a `*` wildcard) match an id? */
export function ruleMatches(rule: string, id: string): boolean {
  if (!rule.includes('*')) return rule === id
  const pattern = rule
    .split('*')
    .map((s) => s.replace(/[.+?^${}()|[\]\\]/g, '\\$&'))
    .join('.*')
  return new RegExp(`^${pattern}$`).test(id)
}

/** Whether `rules` enable `id`, as the server decides: a rule naming it
 *  (`!id` over `id`), else a matching `!` pattern excludes it and a matching
 *  pattern enables it. */
export function rulesEnable(rules: string[], id: string): boolean {
  if (rules.includes(`!${id}`)) return false
  if (rules.includes(id)) return true
  let enabled = false
  for (const r of rules.filter((x) => x.includes('*'))) {
    if (r.startsWith('!')) {
      if (ruleMatches(r.slice(1), id)) return false
    } else if (ruleMatches(r, id)) enabled = true
  }
  return enabled
}

/** The protocol a model runs on, given the protocols with endpoints: its
 *  held one, else its native one, else the first fallback. */
export function protocolFor(
  held: Protocol | null,
  native: Protocol | null,
  usable: Protocol[],
): Protocol | null {
  if (held) return usable.includes(held) ? held : null
  return [native, ...FALLBACK].find((p): p is Protocol => !!p && usable.includes(p)) ?? null
}

/** The enabled ids: of `listed`, then the exact ids not listed, in rule
 *  order. */
export function enabledIds(listed: string[], rules: string[]): string[] {
  const ids = listed.filter((id) => rulesEnable(rules, id))
  for (const r of rules) {
    if (!r.startsWith('!') && !r.includes('*') && !ids.includes(r) && rulesEnable(rules, r)) ids.push(r)
  }
  return ids
}

export async function refreshSettings(): Promise<SettingsInfo | null> {
  try {
    models.settings = await rumu.request<SettingsInfo>('settings.get')
  } catch (err) {
    toast('error', t('settings.loadError'), (err as Error).message)
  }
  return models.settings
}

export function onSettingsUpdated(p: { settings: SettingsInfo }): void {
  models.settings = p.settings
}

/** Every enabled model, by ref. */
export function modelByRef(ref: string): ModelInfo | null {
  for (const p of models.settings?.providers ?? []) {
    const m = p.models.find((x) => x.ref === ref)
    if (m) return m
  }
  return null
}

function branchKey(): string {
  return state.activeBranch ?? ''
}

/** What the next prompt runs on: the pending pick, the branch's marker, or
 *  `null` for the default profile. */
export function currentPick(): ModelUse | null {
  return models.pending[branchKey()] ?? state.pathState?.model ?? null
}

export function pick(use: ModelUse): void {
  const recorded = state.pathState?.model
  if (recorded && sameUse(recorded, use)) delete models.pending[branchKey()]
  else models.pending[branchKey()] = use
}

/** Take the pending pick for a prompt on `branch` (the landing pick moves
 *  to the session's first branch). */
export function takePending(branch: string): ModelUse | undefined {
  const use = models.pending[branch] ?? models.pending['']
  delete models.pending[branch]
  delete models.pending['']
  return use
}

export function sameUse(a: ModelUse, b: ModelUse): boolean {
  return JSON.stringify(a.target) === JSON.stringify(b.target) && (a.effort ?? null) === (b.effort ?? null)
}

/** A pick resolved for display: what it names, the model behind it, and
 *  the effort it runs at. */
export interface Resolved {
  label: string
  profile: string | null
  model: ModelInfo | null
  modelRef: string | null
  effort: Effort | null
}

export function resolvePick(use: ModelUse | null): Resolved | null {
  const s = models.settings
  if (!s) return null
  let profile: string | null = null
  let ref: string | null = null
  let effort: Effort | null = use?.effort ?? null
  if (!use) {
    profile = s.roles.default ?? null
  } else if ('profile' in use.target) {
    profile = use.target.profile
  } else {
    ref = use.target.model
  }
  if (profile !== null) {
    const p = s.profiles.find((x) => x.name === profile)
    ref = p?.model ?? null
    effort = effort ?? p?.effort ?? null
  }
  const model = ref ? modelByRef(ref) : null
  const efforts = model?.efforts ?? []
  const shown = efforts.length ? nearestEffort(effort ?? 'high', efforts) : null
  return {
    label: profile ?? model?.name ?? ref ?? t('settings.noModel'),
    profile,
    model,
    modelRef: ref,
    effort: shown,
  }
}

const ORDER: Effort[] = ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']

/** The effort in `allowed` nearest to `e` (the higher on a tie), as the
 *  server resolves it. */
export function nearestEffort(e: Effort, allowed: Effort[]): Effort {
  const i = ORDER.indexOf(e)
  let best = allowed[0]
  for (const a of allowed) {
    const d = Math.abs(ORDER.indexOf(a) - i)
    const bd = Math.abs(ORDER.indexOf(best) - i)
    if (d < bd || (d === bd && ORDER.indexOf(a) > ORDER.indexOf(best))) best = a
  }
  return best
}

/** Run a settings edit; the reply (also pushed) replaces the cached copy. */
export async function editSettings(method: string, params: Record<string, unknown>): Promise<boolean> {
  try {
    models.settings = await rumu.request<SettingsInfo>(method, params)
    return true
  } catch (err) {
    toast('error', t('settings.saveError'), (err as Error).message)
    return false
  }
}
