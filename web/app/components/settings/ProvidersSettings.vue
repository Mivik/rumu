<script setup lang="ts">
// Providers: the built-in one (its custom models), then the configured
// ones: a preset (its endpoints filled in) or a custom address whose
// endpoints are detected; its key; which of its listed models are enabled
// (one by one, or every match of a search, new ones included); and the
// protocol a model is held to.
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch, watchEffect } from 'vue'
import { t } from '../../i18n'
import { rumu } from '../../lib/api'
import { layout } from '../../lib/layout'
import { SWAP } from '../../lib/motion'
import {
  CUSTOM_PROVIDER,
  PROTOCOL_LABELS,
  PROTOCOLS,
  editSettings,
  enabledIds,
  models,
  protocolFor,
  ruleMatches,
  rulesEnable,
} from '../../lib/state/models'
import type { PresetInfo } from '../../lib/generated/PresetInfo'
import type { Protocol } from '../../lib/generated/Protocol'
import type { ProviderInfo } from '../../lib/generated/ProviderInfo'
import Icon from '../ui/Icon.vue'
import Modal from '../ui/Modal.vue'
import CustomModels from './CustomModels.vue'
import ModelSettings from './ModelSettings.vue'

interface Form {
  id: string
  /** Shown in place of the preset's name (or the id); empty is automatic. */
  name: string
  preset: string | null
  /** The endpoints in use, the preset's included. */
  endpoints: Partial<Record<Protocol, string>>
  api_key: string
  /** The enabling rules: ids, and patterns with `*`. */
  models: string[]
  /** Models held to a protocol; absent is automatic. */
  protocols: Record<string, Protocol>
}

/** A new provider's rules: every listed model, until narrowed. */
const DEFAULT_RULES = ['*']

/** Listed models shown at once beyond the enabled ones. */
const SHOWN = 100

const providers = computed(() => models.settings?.providers ?? [])
const presets = computed(() => models.settings?.presets ?? [])

/** The open provider's id; `''` adds one. The built-in one opens its
 *  custom models. */
const editing = ref<string | null>(null)
/** Adding: pick a preset or a custom endpoint first. */
const picking = ref(false)
const presetQuery = ref('')
const presetInput = ref<HTMLInputElement | null>(null)
/** The presets the search matches, by name, id or address; the server
 *  lists the common ones first. */
const shownPresets = computed(() => {
  const q = presetQuery.value.trim().toLowerCase()
  if (!q) return presets.value
  return presets.value.filter((p) =>
    [p.name, p.id, ...Object.values(p.endpoints)].some((s) => s?.toLowerCase().includes(q)),
  )
})
const form = reactive<Form>(blank())
const pristine = ref('')
const saved = computed(() => providers.value.find((p) => p.id === editing.value) ?? null)
/** What the name field falls back to when empty: the preset's name, else the id. */
const defaultName = computed(() => presets.value.find((p) => p.id === form.preset)?.name ?? form.id.trim())
const isNew = computed(() => editing.value === '')
const dirty = computed(() => JSON.stringify(form) !== pristine.value)
const busy = ref(false)
/** Listing the provider's models: on saving, or on request. */
const listing = ref(false)
const pendingDelete = ref(false)

watchEffect(() => (layout.settingsBusy = busy.value))
onBeforeUnmount(() => (layout.settingsBusy = false))

const customShown = computed(() => editing.value === CUSTOM_PROVIDER)
const formShown = computed(() => editing.value !== null && !picking.value && !customShown.value)

function blank(): Form {
  return { id: '', name: '', preset: null, endpoints: {}, api_key: '', models: [], protocols: {} }
}

function fill(p: ProviderInfo | null): void {
  Object.assign(
    form,
    p
      ? {
          id: p.id,
          name: p.custom_name ?? '',
          preset: p.preset ?? null,
          endpoints: { ...p.endpoints },
          api_key: '',
          models: [...p.rules],
          protocols: { ...p.protocols },
        }
      : blank(),
  )
  pristine.value = JSON.stringify(form)
  query.value = ''
  address.value = ''
  detectOpen.value = false
  probeMissed.value = false
}

function open(p: ProviderInfo): void {
  pendingDelete.value = false
  picking.value = false
  editing.value = p.id
  fill(p)
}

function openCustom(): void {
  pendingDelete.value = false
  picking.value = false
  editing.value = CUSTOM_PROVIDER
}

function startAdd(): void {
  pendingDelete.value = false
  editing.value = ''
  picking.value = true
  fill(null)
}

watch(picking, (v) => v && (presetQuery.value = ''))
// The picker mounts once the page it replaces has faded out.
watch(presetInput, (el) => el?.focus())

// The page, as crumbs after Settings in the page header.
watchEffect(() => {
  const root = { label: t('settings.providers'), go: () => (editing.value = null) }
  const adding = { label: t('settings.addProvider'), go: () => (picking.value = true) }
  const preset = presets.value.find((p) => p.id === form.preset)
  layout.settingsTrail =
    editing.value === null
      ? []
      : customShown.value
        ? [root, { label: t('settings.custom') }]
        : picking.value
          ? [root, { label: adding.label }]
          : isNew.value
            ? [root, adding, { label: preset?.name ?? t('settings.customProvider') }]
            : [root, { label: saved.value?.name ?? editing.value }]
})
onBeforeUnmount(() => (layout.settingsTrail = []))

function choosePreset(p: PresetInfo | null): void {
  picking.value = false
  fill(null)
  form.preset = p?.id ?? null
  form.id = p?.id ?? ''
  form.endpoints = { ...p?.endpoints }
  form.models = [...DEFAULT_RULES]
}

function protocolsOf(endpoints: Partial<Record<Protocol, string>>): Protocol[] {
  return PROTOCOLS.filter((p) => endpoints[p] !== undefined)
}

// -- endpoints

const shownProtocols = computed(() => protocolsOf(form.endpoints))
const missingProtocols = computed(() => PROTOCOLS.filter((p) => form.endpoints[p] === undefined))
const usableProtocols = computed(() => PROTOCOLS.filter((p) => form.endpoints[p]?.trim()))

/** A custom provider's one address, its endpoints detected from it. */
const address = ref('')
/** The address dialog, on request. */
const detectOpen = ref(false)
const addressInput = ref<HTMLInputElement | null>(null)
const probing = ref(false)
/** Nothing detected: `denied` when the key was refused. */
const probeMissed = ref<false | 'none' | 'denied'>(false)
/** Counts detections, so a closed dialog drops the one in flight. */
let probeId = 0

function openDetect(): void {
  probeMissed.value = false
  detectOpen.value = true
  void nextTick(() => addressInput.value?.focus())
}

function closeDetect(): void {
  probeId += 1
  probing.value = false
  detectOpen.value = false
}

async function detect(): Promise<void> {
  const url = address.value.trim()
  if (!url || probing.value) return
  const id = ++probeId
  probing.value = true
  probeMissed.value = false
  try {
    const params: Record<string, unknown> = { url }
    if (form.api_key) params.api_key = form.api_key
    else if (saved.value) params.id = saved.value.id
    const r = await rumu.request<{ endpoints: Partial<Record<Protocol, string>>; denied: boolean }>(
      'settings.probe',
      params,
    )
    if (id !== probeId) return
    if (protocolsOf(r.endpoints).length) {
      form.endpoints = r.endpoints
      closeDetect()
    } else probeMissed.value = r.denied ? 'denied' : 'none'
  } finally {
    if (id === probeId) probing.value = false
  }
}

// Escape closes the dialog, not the settings around it.
function onKey(e: KeyboardEvent): void {
  if (e.key !== 'Escape' || !detectOpen.value) return
  e.stopPropagation()
  closeDetect()
}

onMounted(() => window.addEventListener('keydown', onKey, true))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true))

// -- models

const query = ref('')
const listed = computed(() => saved.value?.listed ?? [])
const savedModels = computed(() => new Map((saved.value?.models ?? []).map((m) => [m.id, m])))
const enabled = computed(() => new Set(enabledIds(listed.value, form.models)))
/** The pattern rules, shown as chips; ids excluded one by one (`!id`) are
 *  the unchecked boxes instead. */
const patterns = computed(() => form.models.filter((r) => r.includes('*')))
const includePatterns = computed(() => patterns.value.filter((r) => !r.startsWith('!')))

const q = computed(() => query.value.trim())
/** The search without a leading `!` (an exclusion searches what it would
 *  exclude). */
const term = computed(() => q.value.replace(/^!/, ''))
const matches = (id: string): boolean => {
  const s = term.value.toLowerCase()
  if (!s) return true
  if (s.includes('*')) return ruleMatches(s, id.toLowerCase())
  return id.toLowerCase().includes(s) || !!name(id)?.toLowerCase().includes(s)
}

/** Every known id (listed, then added by hand), the enabled ones first.
 *  Ordered when the search or the provider changes, not at each toggle, so
 *  a row stays where it was clicked (and one unchecked stays listed). */
const order = ref<{ on: string[]; off: string[] }>({ on: [], off: [] })
watch(
  [q, saved],
  () => {
    const all = [...listed.value, ...form.models.filter((r) => !r.includes('*') && !listed.value.includes(r))]
    const found = all.filter(matches)
    order.value = {
      on: found.filter((id) => enabled.value.has(id)),
      off: found.filter((id) => !enabled.value.has(id)),
    }
  },
  { immediate: true },
)

/** The ordered ids; beyond the enabled ones, at most [`SHOWN`]. */
const rows = computed(() => {
  const { on, off } = order.value
  return { ids: [...on, ...off.slice(0, SHOWN)], more: Math.max(0, off.length - SHOWN) }
})

type RuleKind = 'prefix' | 'contains' | 'pattern' | 'exclude' | 'exclude_prefix' | 'exclude_contains'

/** How many listed ids adding `rule` would turn on or off. */
function count(rule: string): number {
  const rules = [...form.models, rule]
  return listed.value.filter((id) => rulesEnable(rules, id) !== enabled.value.has(id)).length
}

/** The patterns offered for a search: what starts with it, and what
 *  contains it when that does more, enabling what they would, else
 *  excluding; a search with `*` or `!` is its own. Counts are of the
 *  models each turns on or off. */
const searchRules = computed(() => {
  if (!term.value) return []
  const found: { rule: string; kind: RuleKind; n: number }[] = []
  if (q.value.startsWith('!')) {
    found.push({ rule: q.value, kind: 'exclude', n: count(q.value) })
  } else if (q.value.includes('*')) {
    // Typed as a rule: offered even before anything matches it.
    found.push({ rule: q.value, kind: 'pattern', n: count(q.value) })
  } else {
    const prefix = `${q.value}*`
    const contains = `*${q.value}*`
    const off = !count(contains)
    const mark = off ? '!' : ''
    const [p, c] = [count(mark + prefix), count(mark + contains)]
    if (p) found.push({ rule: mark + prefix, kind: off ? 'exclude_prefix' : 'prefix', n: p })
    if (c > p) found.push({ rule: mark + contains, kind: off ? 'exclude_contains' : 'contains', n: c })
  }
  return found.filter((r) => !form.models.includes(r.rule))
})

/** Enter: the first offer. */
function addSearched(): void {
  const rule = searchRules.value[0]?.rule ?? customId.value
  if (rule) addRule(rule)
}

/** A searched id nothing listed matches, to add by hand. */
const customId = computed(() => {
  const id = q.value
  if (!id || /[*!]/.test(id) || form.models.includes(id) || rows.value.ids.length) return null
  return id
})

/** The pattern that enables `id`, when one does. */
function patternFor(id: string): string | null {
  return includePatterns.value.find((r) => ruleMatches(r, id)) ?? null
}

/** Flip one model with a rule naming it (`id` or `!id`), when the other
 *  rules would not already put it there. */
function toggle(id: string): void {
  const rules = form.models.filter((r) => r !== id && r !== `!${id}`)
  const want = !enabled.value.has(id)
  if (rulesEnable(rules, id) !== want) rules.push(want ? id : `!${id}`)
  form.models = rules
}

const isDefaultRules = computed(() => JSON.stringify(form.models) === JSON.stringify(DEFAULT_RULES))

/** Back to every listed model. */
function resetRules(): void {
  form.models = [...DEFAULT_RULES]
}

function addRule(rule: string): void {
  if (!form.models.includes(rule)) form.models.push(rule)
  query.value = ''
}

function removeRule(rule: string): void {
  form.models = form.models.filter((r) => r !== rule)
}

/** Its name; `null` for one added by hand and not saved yet. */
function name(id: string): string | null {
  return saved.value?.names[id] ?? savedModels.value.get(id)?.name ?? null
}

// -- protocols

/** Choosing one makes sense only between several. */
const choosable = computed(() => usableProtocols.value.length > 1)

/** What an enabled model runs on, as being edited. */
function protocolOf(id: string): Protocol | null {
  const native = savedModels.value.get(id)?.native ?? null
  return protocolFor(form.protocols[id] ?? null, native, usableProtocols.value)
}

function setProtocol(id: string, protocol: Protocol | null): void {
  if (protocol) form.protocols[id] = protocol
  else delete form.protocols[id]
}

// -- saving

const canSave = computed(() => !busy.value && dirty.value && form.id.trim() !== '' && !form.id.includes('/'))

async function save(): Promise<void> {
  if (!canSave.value) return
  busy.value = true
  const id = form.id.trim()
  const endpoints: Record<string, string> = {}
  for (const p of usableProtocols.value) endpoints[p] = form.endpoints[p]!.trim()
  const protocols = Object.fromEntries(Object.entries(form.protocols).filter(([m]) => enabled.value.has(m)))
  const params: Record<string, unknown> = {
    id,
    name: form.name.trim(),
    endpoints,
    models: form.models,
    protocols,
  }
  if (form.preset) params.preset = form.preset
  if (form.api_key) params.api_key = form.api_key
  // Its list changes only with where it is reached and how.
  const relist =
    !saved.value || !!form.api_key || PROTOCOLS.some((p) => endpoints[p] !== saved.value!.endpoints[p])
  const ok = await editSettings('settings.set_provider', params)
  const p = ok ? providers.value.find((x) => x.id === id) : undefined
  if (p) {
    open(p)
    if (relist) await listModels(id)
  } else if (ok) editing.value = null
  busy.value = false
}

async function listModels(id: string): Promise<void> {
  listing.value = true
  await editSettings('settings.refresh_models', { id })
  listing.value = false
}

async function remove(): Promise<void> {
  if (!saved.value) return
  busy.value = true
  const ok = await editSettings('settings.remove_provider', { id: saved.value.id })
  busy.value = false
  if (ok) editing.value = null
}

function removeEndpoint(protocol: Protocol): void {
  delete form.endpoints[protocol]
}

// Opened at adding one (the empty states' link).
watch(
  () => layout.addingProvider,
  (adding) => {
    if (!adding) return
    layout.addingProvider = false
    startAdd()
  },
  { immediate: true },
)

// Opened at a model (a usage card's link): its provider, searched for it.
watch(
  () => layout.showingModel,
  (ref) => {
    if (!ref) return
    layout.showingModel = null
    const slash = ref.indexOf('/')
    if (ref.slice(0, slash) === CUSTOM_PROVIDER) return openCustom()
    const p = providers.value.find((x) => x.id === ref.slice(0, slash))
    if (!p) return
    open(p)
    query.value = ref.slice(slash + 1)
  },
  { immediate: true },
)

async function refreshModels(): Promise<void> {
  if (!saved.value || busy.value) return
  busy.value = true
  await listModels(saved.value.id)
  busy.value = false
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div class="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-4 pt-5 pb-6 sm:px-6">
      <Transition v-bind="SWAP" mode="out-in">
        <!-- the list -->
        <div v-if="editing === null" class="flex flex-col gap-2">
          <button
            type="button"
            class="flex items-center gap-3 rounded-card border border-line-soft bg-surface px-3 py-2.5 text-left transition-colors hover:bg-surface-2"
            @click="openCustom"
          >
            <span class="font-semibold text-ink">{{ t('settings.custom') }}</span>
            <span class="ml-auto text-meta text-ink-3">{{
              t('settings.modelCount', { n: models.settings?.custom.length ?? 0 })
            }}</span>
            <Icon name="chevron" :size="12" class="text-ink-3" />
          </button>
          <button
            v-for="p in providers"
            :key="p.id"
            type="button"
            class="flex items-center gap-3 rounded-card border border-line-soft bg-surface px-3 py-2.5 text-left transition-colors hover:bg-surface-2"
            @click="open(p)"
          >
            <span class="font-semibold text-ink">{{ p.name }}</span>
            <span class="flex flex-wrap gap-1">
              <span
                v-for="protocol in protocolsOf(p.endpoints)"
                :key="protocol"
                class="rounded-xs border border-line px-1 text-micro text-ink-3"
                >{{ PROTOCOL_LABELS[protocol] }}</span
              >
            </span>
            <span class="ml-auto text-meta text-ink-3">{{
              t('settings.modelCount', { n: p.models.length })
            }}</span>
            <Icon
              v-if="!p.has_api_key"
              name="blocked"
              :size="13"
              class="text-danger"
              :title="t('settings.noKey')"
            />
            <Icon name="chevron" :size="12" class="text-ink-3" />
          </button>
          <div class="flex items-center gap-2 pt-1">
            <button type="button" class="btn" @click="startAdd">
              <Icon name="plus" :size="13" />
              {{ t('settings.addProvider') }}
            </button>
          </div>
        </div>

        <!-- adding: pick a preset or a custom endpoint -->
        <div v-else-if="picking" class="flex flex-col gap-2">
          <div class="overflow-hidden rounded-control border border-line-soft">
            <div class="relative border-b border-line-soft">
              <Icon
                name="search"
                :size="13"
                class="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-ink-3"
              />
              <input
                ref="presetInput"
                v-model="presetQuery"
                type="search"
                class="w-full rounded-none border-0 bg-transparent py-2 pl-8 text-secondary"
                :placeholder="t('settings.searchProviders')"
                autocomplete="off"
                spellcheck="false"
                @keydown.enter.prevent="shownPresets[0] && choosePreset(shownPresets[0])"
              />
            </div>
            <div class="max-h-96 overflow-y-auto py-1">
              <button
                v-for="p in shownPresets"
                :key="p.id"
                type="button"
                class="flex w-full items-center gap-2 px-3 py-1.5 text-left transition-colors hover:bg-surface-2"
                @click="choosePreset(p)"
              >
                <span class="min-w-0 truncate text-secondary text-ink">{{ p.name }}</span>
                <span class="flex flex-none gap-1">
                  <span
                    v-for="protocol in protocolsOf(p.endpoints)"
                    :key="protocol"
                    class="rounded-xs border border-line px-1 text-micro text-ink-3"
                    >{{ PROTOCOL_LABELS[protocol] }}</span
                  >
                </span>
              </button>
              <p v-if="!shownPresets.length" class="px-3 py-3 text-center text-meta text-ink-3 italic">
                {{ t('settings.noProvidersFound') }}
              </p>
            </div>
          </div>
          <button
            type="button"
            class="-mx-2.5 flex items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-secondary text-ink-2 transition-colors hover:bg-surface-2"
            @click="choosePreset(null)"
          >
            <Icon name="plus" :size="13" class="text-ink-3" />
            {{ t('settings.customProvider') }}
          </button>
        </div>

        <CustomModels v-else-if="customShown" />

        <!-- the provider's form -->
        <form v-else-if="formShown" class="flex flex-col gap-5" @submit.prevent="save">
          <div class="grid gap-3" :class="{ 'sm:grid-cols-2': isNew }">
            <label v-if="isNew" class="flex flex-col gap-1.5">
              <span class="field-label">{{ t('settings.providerId') }}</span>
              <input
                v-model="form.id"
                type="text"
                class="w-full font-mono text-meta"
                autocomplete="off"
                spellcheck="false"
              />
            </label>
            <label class="flex flex-col gap-1.5">
              <span class="field-label">{{ t('settings.providerName') }}</span>
              <input
                v-model="form.name"
                type="text"
                class="w-full"
                :placeholder="defaultName"
                autocomplete="off"
                spellcheck="false"
              />
            </label>
            <label class="flex flex-col gap-1.5" :class="{ 'sm:col-span-2': isNew }">
              <span class="field-label">{{ t('settings.apiKey') }}</span>
              <input
                v-model="form.api_key"
                type="password"
                class="w-full font-mono text-meta"
                :placeholder="saved?.has_api_key ? `•••••••• ${t('settings.apiKeySaved')}` : ''"
                autocomplete="off"
                spellcheck="false"
              />
            </label>
          </div>

          <!-- endpoints -->
          <div class="flex flex-col gap-2">
            <div class="flex items-center gap-2">
              <span class="field-label">{{ t('settings.endpoints') }}</span>
              <button
                v-if="!form.preset"
                type="button"
                class="ml-auto inline-flex items-center gap-1.5 rounded-control border border-brand/40 bg-brand-soft px-2.5 py-1 text-meta font-semibold text-brand-strong transition-colors hover:border-brand/60 hover:bg-brand-soft/70"
                @click="openDetect"
              >
                <Icon name="search" :size="12" />
                {{ t('settings.detectFromAddress') }}
              </button>
            </div>
            <div
              class="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-2 sm:grid-cols-[8rem_minmax(0,1fr)_auto]"
            >
              <template v-for="protocol in shownProtocols" :key="protocol">
                <span class="col-span-2 -mb-1 text-meta text-ink-3 sm:col-span-1 sm:mb-0">{{
                  PROTOCOL_LABELS[protocol]
                }}</span>
                <input
                  v-model="form.endpoints[protocol]"
                  type="url"
                  class="w-full font-mono text-meta"
                  autocomplete="off"
                  spellcheck="false"
                />
                <button
                  type="button"
                  class="icon-btn size-7 justify-self-center"
                  :title="t('settings.removeEndpoint')"
                  :aria-label="t('settings.removeEndpoint')"
                  @click="removeEndpoint(protocol)"
                >
                  <Icon name="close" :size="12" />
                </button>
              </template>
            </div>
            <div v-if="missingProtocols.length" class="flex flex-wrap gap-1.5">
              <button
                v-for="protocol in missingProtocols"
                :key="protocol"
                type="button"
                class="inline-flex items-center gap-1 rounded-full border border-dashed border-line px-2.5 py-0.5 text-meta text-ink-3 transition-colors hover:border-ink-3 hover:text-ink"
                @click="form.endpoints[protocol] = ''"
              >
                <Icon name="plus" :size="11" />
                {{ PROTOCOL_LABELS[protocol] }}
              </button>
            </div>
          </div>

          <!-- models: listed once saved -->
          <div v-if="saved" class="flex flex-col gap-2">
            <div class="flex items-center gap-2">
              <span class="field-label">{{ t('settings.models') }}</span>
              <span class="text-meta text-ink-3">{{
                t('settings.modelsSummary', { n: enabled.size, m: listed.length })
              }}</span>
              <button
                v-if="!isDefaultRules"
                type="button"
                class="ml-auto rounded-control px-2 py-0.5 text-meta text-ink-3 transition-colors hover:bg-surface-2 hover:text-ink"
                @click="resetRules"
              >
                {{ t('settings.resetRules') }}
              </button>
              <button
                type="button"
                class="icon-btn size-6"
                :class="{ 'ml-auto': isDefaultRules }"
                :disabled="busy"
                :title="t('settings.refreshModels')"
                :aria-label="t('settings.refreshModels')"
                @click="refreshModels"
              >
                <Icon name="regenerate" :size="13" :class="{ 'animate-spin': listing }" />
              </button>
            </div>
            <p v-if="saved.list_error" class="flex items-start gap-1.5 text-meta text-danger">
              <Icon name="error" :size="13" class="mt-0.5 flex-none" />
              <span class="min-w-0 truncate" :title="saved.list_error">{{ saved.list_error }}</span>
            </p>

            <div v-if="patterns.length" class="flex flex-wrap items-center gap-1.5">
              <span class="text-meta text-ink-3">{{ t('settings.rules') }}</span>
              <span
                v-for="rule in patterns"
                :key="rule"
                class="inline-flex items-center gap-0.5 rounded-full bg-surface-2 py-0.5 pr-0.5 pl-2.5 font-mono text-meta text-ink-2"
              >
                {{ rule }}
                <button
                  type="button"
                  class="icon-btn size-5"
                  :aria-label="t('settings.removeRule')"
                  :title="t('settings.removeRule')"
                  @click="removeRule(rule)"
                >
                  <Icon name="close" :size="10" />
                </button>
              </span>
            </div>

            <div class="overflow-hidden rounded-control border border-line-soft">
              <div class="relative border-b border-line-soft">
                <Icon
                  name="search"
                  :size="13"
                  class="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-ink-3"
                />
                <input
                  v-model="query"
                  type="search"
                  class="w-full rounded-none border-0 bg-transparent py-2 pl-8 text-secondary"
                  :placeholder="t('settings.searchOrAddModel')"
                  autocomplete="off"
                  spellcheck="false"
                  @keydown.enter.prevent="addSearched"
                />
              </div>
              <div v-if="searchRules.length || customId" class="flex flex-col border-b border-line-soft py-1">
                <button
                  v-for="r in searchRules"
                  :key="r.rule"
                  type="button"
                  class="flex items-center gap-2 px-3 py-1.5 text-left text-secondary text-ink transition-colors hover:bg-surface-2"
                  @click="addRule(r.rule)"
                >
                  <Icon
                    :name="r.kind.startsWith('exclude') ? 'close' : 'plus'"
                    :size="13"
                    :class="r.kind.startsWith('exclude') ? 'text-ink-3' : 'text-brand'"
                  />
                  <i18n-t :keypath="`settings.enable_${r.kind}`" scope="global" tag="span">
                    <template #q>
                      <span class="font-mono">{{ term }}</span>
                    </template>
                    <template #n>{{ r.n }}</template>
                  </i18n-t>
                  <span class="font-mono text-micro text-ink-3">{{ r.rule }}</span>
                </button>
                <button
                  v-if="customId"
                  type="button"
                  class="flex items-center gap-2 px-3 py-1.5 text-left text-secondary text-ink transition-colors hover:bg-surface-2"
                  @click="addRule(customId)"
                >
                  <Icon name="plus" :size="13" class="text-ink-3" />
                  <i18n-t keypath="settings.addModelId" scope="global" tag="span">
                    <template #id>
                      <span class="font-mono">{{ customId }}</span>
                    </template>
                  </i18n-t>
                </button>
              </div>
              <div class="max-h-80 overflow-y-auto py-1">
                <div
                  v-for="id in rows.ids"
                  :key="id"
                  class="flex items-center gap-2 py-0.5 pr-1.5 pl-3"
                  :class="enabled.has(id) ? '' : 'opacity-70 hover:opacity-100'"
                >
                  <label class="flex min-w-0 flex-1 cursor-pointer items-center gap-2.5 py-0.5">
                    <input type="checkbox" :checked="enabled.has(id)" @change="toggle(id)" />
                    <span v-if="name(id)" class="max-w-1/2 flex-none truncate text-secondary text-ink">{{
                      name(id)
                    }}</span>
                    <span class="min-w-0 truncate font-mono text-micro text-ink-3">{{ id }}</span>
                    <span
                      v-if="includePatterns.length > 1 && enabled.has(id) && !form.models.includes(id)"
                      class="flex-none rounded-xs border border-line px-1 font-mono text-micro text-ink-3"
                      :title="t('settings.enabledBy', { rule: patternFor(id) })"
                      >{{ patternFor(id) }}</span
                    >
                  </label>
                  <span
                    v-if="choosable && enabled.has(id)"
                    class="flex-none text-micro"
                    :class="form.protocols[id] ? 'text-ink-2' : 'text-ink-3'"
                    >{{ PROTOCOL_LABELS[protocolOf(id)!] ?? '—' }}</span
                  >
                  <ModelSettings
                    :provider="saved.id"
                    :id="id"
                    :info="savedModels.get(id)"
                    :held="form.protocols[id] ?? null"
                    :usable="usableProtocols"
                    @protocol="setProtocol(id, $event)"
                  />
                </div>
                <p v-if="rows.more" class="px-3 py-1.5 text-meta text-ink-3 italic">
                  {{ t('settings.moreModels', { n: rows.more }) }}
                </p>
                <p
                  v-if="listing && !listed.length"
                  class="flex items-center justify-center gap-2 px-3 py-3 text-meta text-ink-3"
                >
                  <span
                    class="size-3.5 animate-spin rounded-full border-2 border-brand border-t-transparent"
                  />
                  {{ t('settings.loadingModels') }}
                </p>
                <p
                  v-else-if="!rows.ids.length && !searchRules.length && !customId"
                  class="px-3 py-3 text-center text-meta text-ink-3 italic"
                >
                  {{ t('settings.noModels') }}
                </p>
              </div>
            </div>
          </div>
        </form>
      </Transition>
    </div>

    <!-- arrives with the form, which waits for the page it replaces to fade -->
    <Transition
      enter-active-class="transition-opacity duration-300 delay-150 ease-soft"
      enter-from-class="opacity-0"
      leave-active-class="transition-opacity duration-150 ease-soft"
      leave-to-class="opacity-0"
    >
      <footer
        v-if="formShown"
        class="flex flex-none items-center gap-2 border-t border-line-soft px-4 py-3 sm:px-6"
      >
        <template v-if="saved">
          <div v-if="pendingDelete" class="flex items-center gap-2">
            <button type="button" class="btn" :disabled="busy" @click="pendingDelete = false">
              {{ t('common.cancel') }}
            </button>
            <button type="button" class="btn btn-danger" :disabled="busy" @click="remove">
              {{ t('settings.deleteProvider', { name: saved.name }) }}
            </button>
          </div>
          <button
            v-else
            type="button"
            class="inline-flex items-center gap-1.5 rounded-control border border-danger/30 bg-danger-soft px-2.5 py-1 text-meta font-semibold text-danger transition-colors hover:border-danger/50 hover:bg-danger-soft/70"
            :title="t('settings.deleteProvider', { name: saved.name })"
            :disabled="busy"
            @click="pendingDelete = true"
          >
            <Icon name="trash" :size="13" />
            {{ t('settings.delete') }}
          </button>
        </template>
        <span class="flex-1" />
        <button v-if="dirty && saved" type="button" class="btn" :disabled="busy" @click="open(saved)">
          {{ t('settings.discard') }}
        </button>
        <button type="button" class="btn btn-primary" :disabled="!canSave" @click="save">
          <span
            v-if="busy"
            class="size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent"
          />
          {{ isNew ? t('settings.add') : t('settings.save') }}
        </button>
      </footer>
    </Transition>

    <Modal
      :open="detectOpen"
      :label="t('settings.detectFromAddress')"
      :z="60"
      wrapper-class="p-3 sm:p-6"
      panel-class="w-full max-w-[440px] rounded-sheet"
      @close="closeDetect"
    >
      <form class="flex flex-col gap-4 p-5" @submit.prevent="detect">
        <header class="flex items-center gap-2">
          <h3 class="text-title font-bold">{{ t('settings.detectFromAddress') }}</h3>
          <button
            type="button"
            class="icon-btn ml-auto size-7"
            :title="t('common.close')"
            :aria-label="t('common.close')"
            @click="closeDetect"
          >
            <Icon name="close" :size="13" />
          </button>
        </header>
        <label class="flex flex-col gap-1.5">
          <span class="field-label">{{ t('settings.address') }}</span>
          <input
            ref="addressInput"
            v-model="address"
            type="url"
            class="w-full font-mono text-meta"
            placeholder="https://gateway.example/v1"
            autocomplete="off"
            spellcheck="false"
          />
        </label>
        <p v-if="probeMissed" class="flex items-start gap-1.5 text-meta text-danger">
          <Icon name="error" :size="13" class="mt-0.5 flex-none" />
          {{ probeMissed === 'denied' ? t('settings.detectDenied') : t('settings.detectNone') }}
        </p>
        <footer class="flex items-center justify-end gap-2">
          <button type="button" class="btn" @click="closeDetect">{{ t('common.cancel') }}</button>
          <button type="submit" class="btn btn-primary" :disabled="probing || !address.trim()">
            <Icon name="search" :size="13" />
            {{ probing ? t('settings.detecting') : t('settings.detect') }}
          </button>
        </footer>
      </form>
    </Modal>
  </div>
</template>
