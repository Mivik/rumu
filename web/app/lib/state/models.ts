// Model settings (`settings.*`) and the composer's model pick. A branch runs
// on its last `model.use` marker, else its assistant's model; a null pick is
// the assistant's. A pick made in a session stays pending until the next
// prompt or regeneration carries it; the server records it only when it
// changes the branch's model. Every model picked is also remembered (prefs)
// as the one a new session starts on when its assistant has no model.

import { reactive } from 'vue'
import { t } from '../../i18n'
import { rumu } from '../api'
import type { Effort } from '../generated/Effort'
import type { ModelInfo } from '../generated/ModelInfo'
import type { ModelUse } from '../generated/ModelUse'
import type { Protocol } from '../generated/Protocol'
import type { SettingsInfo } from '../generated/SettingsInfo'
import type { BranchId, Node } from '../types'
import { prefs } from './prefs'
import { landingAssistant } from './projects'
import { state } from './state'
import { toast } from './ui'

/** The protocols, in display order. */
export const PROTOCOLS: Protocol[] = ['anthropic', 'openai', 'openai_responses', 'gemini']

/** Tried after a model's native protocol, in order. */
const FALLBACK: Protocol[] = ['openai', 'anthropic', 'openai_responses', 'gemini']

/** The built-in provider, whose models are the aliases: `rumu/<name>`. */
export const ALIAS_PROVIDER = 'rumu'

export const PROTOCOL_LABELS: Record<Protocol, string> = {
  anthropic: 'Anthropic',
  openai: 'OpenAI',
  openai_responses: 'OpenAI Responses',
  gemini: 'Gemini',
}

export const models = reactive({
  settings: null as SettingsInfo | null,
  /** Unsent picks by session id. */
  pending: {} as Record<string, ModelUse | null>,
  /** The landing page's pick, for the assistant it was made with. */
  landing: null as { assistant: string; use: ModelUse | null } | null,
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

/** Every alias or enabled model, by ref. */
export function modelByRef(ref: string): ModelInfo | null {
  const s = models.settings
  for (const list of [s?.aliases ?? [], ...(s?.providers ?? []).map((p) => p.models)]) {
    const m = list.find((x) => x.ref === ref)
    if (m) return m
  }
  return null
}

/** The model of the assistant the composer runs as. */
export function assistantModel(): ModelUse | null {
  const assistant = state.sessionId ? state.pathState?.assistant : landingAssistant.value
  return state.assistants.find((a) => a.id === assistant)?.model ?? null
}

/** What the next prompt runs on: the session's unsent pick or its branch's
 *  model; on the landing page the pick made there, else the assistant's
 *  model, else the remembered pick, while it still exists. `null` is the
 *  assistant's model. */
export function currentPick(): ModelUse | null {
  const session = state.sessionId
  if (session) {
    if (session in models.pending) return models.pending[session] ?? null
    return state.pathState?.model ?? null
  }
  const assistant = landingAssistant.value
  if (models.landing?.assistant === assistant) return models.landing.use
  if (assistantModel()) return null
  return prefs.model && exists(prefs.model) ? prefs.model : null
}

function exists(use: ModelUse): boolean {
  return !models.settings || !!modelByRef(use.model)
}

export function pick(use: ModelUse | null): void {
  if (use) prefs.model = use
  const session = state.sessionId
  if (!session) {
    models.landing = { assistant: landingAssistant.value, use }
    return
  }
  if (sameUse(state.pathState?.model ?? null, use)) delete models.pending[session]
  else models.pending[session] = use
}

/** The model `path` runs on: its last `model.use` marker; none, or a
 *  `model.reset` after it, is the assistant's (`null`). */
export function modelOfPath(path: Node[]): ModelUse | null {
  for (let i = path.length - 1; i >= 0; i--) {
    const b = path[i]!.body
    const data = b.type === 'note' ? b.content.data : undefined
    if (data?.tag === 'model.use') return data.data as unknown as ModelUse
    if (data?.tag === 'model.reset') return null
  }
  return null
}

/** What the next message to sub-agent `branch` (its path `path`) runs on:
 *  the pick made in its panel, kept by branch until sent, else its model. */
export function branchPick(branch: BranchId | null, path: Node[]): ModelUse | null {
  if (branch && branch in models.pending) return models.pending[branch] ?? null
  return modelOfPath(path)
}

export function pickFor(branch: BranchId | null, path: Node[], use: ModelUse | null): void {
  if (!branch) return
  if (use) prefs.model = use
  if (sameUse(modelOfPath(path), use)) delete models.pending[branch]
  else models.pending[branch] = use
}

export function sameUse(a: ModelUse | null, b: ModelUse | null): boolean {
  if (!a || !b) return a === b
  return a.model === b.model && (a.effort ?? null) === (b.effort ?? null)
}

/** A pick resolved for display: the model it names (the assistant's when
 *  none), and the effort it runs at. */
export interface Resolved {
  label: string
  model: ModelInfo | null
  modelRef: string | null
  effort: Effort | null
}

export function resolvePick(use: ModelUse | null): Resolved | null {
  const s = models.settings
  if (!s) return null
  const run = use ?? assistantModel()
  const ref = run?.model ?? null
  const model = ref ? modelByRef(ref) : null
  const effort = run?.effort ?? model?.target?.effort ?? 'high'
  const efforts = model?.efforts ?? []
  return {
    label: model?.name ?? ref ?? t('settings.noModel'),
    model,
    modelRef: ref,
    effort: efforts.length ? nearestEffort(effort, efforts) : null,
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
