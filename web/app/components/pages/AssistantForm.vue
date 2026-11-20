<script setup lang="ts">
// One assistant on the assistants page (`/assistants/<id>`, `?new` for a new
// one). Each assistant is a deck (`decks/<id>/deck.yaml`) with a pinned
// `instructions.md`; the form edits its name, instructions, model, MCP
// servers, skills, who may ask it and, last, the decks it mounts. Saves go through
// `assistant.upsert` / `assistant.delete`; `assistant.set_default` picks the
// one sessions run as by default.
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { t } from '../../i18n'
import { rumu } from '../../lib/api'
import { CAT_SHAPE_MAX, assistantCat, assistantTone, randomCat } from '../../lib/assistantFace'
import { CAT_ITEMS, itemSlot } from '../../lib/catItems'
import { trackItemDirty } from '../../lib/layout'
import { PROJECT_TONES, toneColor, type ProjectTone } from '../../lib/projectIcon'
import { openPage, replacePage } from '../../lib/router'
import { modelByRef, models, nearestEffort, refreshSettings, rulesEnable } from '../../lib/state/models'
import {
  connected,
  defaultAssistant,
  openLibraryAtDeck,
  refreshAssistants,
  state,
  viewAssistant,
} from '../../lib/store'
import type { Assistant, AssistantOptions, DeckCat, DeckMount, ModelUse } from '../../lib/types'
import type { Effort } from '../../lib/generated/Effort'
import { copyText } from '../../lib/util'
import AssistantAvatar from '../brand/AssistantAvatar.vue'
import ModelField from '../settings/ModelField.vue'
import Icon from '../ui/Icon.vue'
import Select from '../ui/Select.vue'
import type { SelectOption } from '../ui/Select.vue'

interface Form {
  id: string
  name: string
  mounts: DeckMount[]
  instructions: string
  /** The model its sessions run on, until one picks another; null is none. */
  model: ModelUse | null
  /** Each bound server with the rules enabling its tools. */
  mcp: Record<string, string[]>
  /** The rules enabling skills. */
  skills: string[]
  allow_ask: string[]
  /** Its own cat; null draws one from the id. */
  cat: DeckCat | null
}

function blank(): Form {
  return {
    id: '',
    name: '',
    mounts: [],
    instructions: '',
    model: null,
    mcp: {},
    skills: [],
    allow_ask: [],
    cat: null,
  }
}

const options = ref<AssistantOptions>({ decks: [] })
const form = reactive<Form>(blank())
const pristine = ref(JSON.stringify(blank()))
const pendingDelete = ref(false)
const saving = ref(false)
const deleting = ref(false)
const loadError = ref('')
const saveError = ref('')

/** The default assistant first: it is the one everything falls back to. */
const assistants = computed(() => [...state.assistants].sort((a, b) => Number(b.default) - Number(a.default)))
const isNew = computed(() => !!state.page?.fresh)
/** The assistant open on the page (none for a new one, or a stale id). */
const current = computed(() =>
  isNew.value ? null : (state.assistants.find((a) => a.id === state.page?.item) ?? null),
)
const editing = computed(() => current.value?.id ?? '')

/** Its tone; a new one is plain until it has an id. */
const tone = computed(() => (form.id ? assistantTone(form.id) : 'var(--ink-3)'))
const near = ref(false)

/** The cat on show: its own, the one from its id, or a plain one for a new
 * assistant. The first change makes it its own. */
const cat = computed<DeckCat>(
  () =>
    form.cat ?? (form.id ? assistantCat({ id: form.id }) : { items: [], tone: 'brand', wide: 0, ears: 0 }),
)
const catTone = computed(() => toneColor((cat.value.tone ?? 'brand') as ProjectTone))
function editCat(change: Partial<DeckCat>): void {
  form.cat = { ...cat.value, items: [...(cat.value.items ?? [])], ...change }
}
/** Items of one slot replace each other. */
function toggleItem(name: string): void {
  const items = cat.value.items ?? []
  const slot = itemSlot(name)
  editCat({
    items: items.includes(name)
      ? items.filter((i) => i !== name)
      : [...items.filter((i) => !slot || itemSlot(i) !== slot), name],
  })
}
function shuffleCat(): void {
  form.cat = randomCat(Math.random().toString(36))
}

const copied = ref(false)
async function copyId(): Promise<void> {
  if (!(await copyText(form.id))) return
  copied.value = true
  setTimeout(() => (copied.value = false), 1200)
}
const isDefault = computed(() => !!current.value?.default)
/** The last assistant cannot go: sessions need one to run as. */
const canDelete = computed(() => !isNew.value && state.assistants.length > 1)
const dirty = computed(() => JSON.stringify(form) !== pristine.value)
trackItemDirty(dirty)
const canSave = computed(
  () => !saving.value && dirty.value && !!form.name.trim() && !badMounts.value.size && !badDecks.value.size,
)

/** Decks a mount row may name; the assistant's own deck is the fixed `/cards/self`. */
const deckOptions = computed<SelectOption[]>(() =>
  options.value.decks
    .filter((d) => d.id !== form.id)
    .map((d) => ({ value: d.id, label: d.name, icon: 'memory' })),
)

/** Mount rows showing the deck dropdown instead of the deck link. */
const editingMounts = ref(new Set<DeckMount>())

/** Jump to a deck's row in the card library (Back returns here). */
function openDeck(id: string): void {
  openLibraryAtDeck(id)
}

const PREFIX = '/cards/'
const MOUNT_NAME = /^[a-z][a-z0-9-]*(\/[a-z][a-z0-9-]*)*$/

/** A mount point without the fixed `/cards/` prefix. */
function mountName(at: string): string {
  return at.startsWith(PREFIX) ? at.slice(PREFIX.length) : at
}

/** Equal, or one inside the other: such mounts cannot coexist. */
function overlaps(a: string, b: string): boolean {
  return a === b || a.startsWith(`${b}/`) || b.startsWith(`${a}/`)
}

function deckName(id: string): string {
  return options.value.decks.find((d) => d.id === id)?.name ?? id.slice(0, 8)
}

/** Rows (by index) with no deck yet, or a deck mounted twice. */
const badDecks = computed(
  () =>
    new Set(
      form.mounts.flatMap((m, i) =>
        !m.deck || form.mounts.some((other, j) => j !== i && other.deck === m.deck) ? [i] : [],
      ),
    ),
)
const duplicateDeck = computed(() => form.mounts.some((m, i) => m.deck && badDecks.value.has(i)))

/** Rows (by index) whose mount point is malformed or collides with another,
 *  `self` included. A just-added row is left alone until it names a deck. */
const badMounts = computed(() => {
  const names = form.mounts.map((m) => mountName(m.at))
  return new Set(
    form.mounts.flatMap((m, i) => {
      const name = names[i]!
      if (!m.deck && !name) return []
      const bad =
        !MOUNT_NAME.test(name) ||
        overlaps(name, 'self') ||
        names.some((other, j) => j !== i && overlaps(name, other))
      return bad ? [i] : []
    }),
  )
})

/** A new row starts in the dropdown; its mount point is named on pick. */
function addMount(): void {
  form.mounts.push({ deck: '', at: '' })
  editingMounts.value.add(form.mounts.at(-1)!)
}

function pickDeck(mount: DeckMount, id: string): void {
  mount.deck = id
  if (!mount.at) {
    const deck = deckName(id)
    const taken = ['self', ...form.mounts.map((m) => mountName(m.at)).filter(Boolean)]
    let name = deck
    for (let n = 2; taken.some((other) => overlaps(name, other)); n++) name = `${deck}-${n}`
    mount.at = PREFIX + name
  }
  // A repeat stays open so it can be fixed; saving is refused meanwhile.
  if (form.mounts.filter((m) => m.deck === id).length === 1) editingMounts.value.delete(mount)
}

function toggleEditMount(mount: DeckMount): void {
  if (!editingMounts.value.delete(mount)) editingMounts.value.add(mount)
}

function removeMount(index: number): void {
  editingMounts.value.delete(form.mounts[index]!)
  form.mounts.splice(index, 1)
}

const askOptions = computed(() => assistants.value.filter((a) => a.id !== form.id))

/** The configured MCP servers, then names the assistant keeps that are
 *  no longer configured. */
const mcpOptions = computed(() => {
  const configured = (models.settings?.mcp ?? []).map((s) => s.name)
  return [
    ...configured.map((name) => ({ name, gone: false })),
    ...Object.keys(form.mcp)
      .filter((n) => !configured.includes(n))
      .map((name) => ({ name, gone: true })),
  ]
})

/** The bound servers that list tools, each with the ones its rules enable. */
const mcpTools = computed(() =>
  (models.settings?.mcp ?? [])
    .filter((s) => form.mcp[s.name] && s.tools.length)
    .map((s) => ({
      name: s.name,
      tools: s.tools,
      on: s.tools.filter((tool) => rulesEnable(form.mcp[s.name]!, tool.name)).length,
    })),
)
/** The servers whose tool list is open. */
const openTools = ref(new Set<string>())

/** Bind a server with all its tools, or unbind it. Kept sorted, as the
 *  server stores them, so binding back is no change. */
function toggleServer(name: string): void {
  const next = { ...form.mcp }
  if (next[name]) delete next[name]
  else next[name] = ['*']
  form.mcp = Object.fromEntries(Object.entries(next).sort(([a], [b]) => (a < b ? -1 : 1)))
}

/** `rules` with `id` flipped by a rule naming it, when the other rules
 *  would not already put it there. */
function flip(rules: string[], id: string): string[] {
  const rest = rules.filter((r) => r !== id && r !== `!${id}`)
  const want = !rulesEnable(rules, id)
  if (rulesEnable(rest, id) !== want) rest.push(want ? id : `!${id}`)
  return rest
}

function toggleTool(server: string, tool: string): void {
  form.mcp[server] = flip(form.mcp[server]!, tool)
}

const modelRef = computed({
  get: () => form.model?.model ?? '',
  // Another model starts at its own effort.
  set: (ref: string) => (form.model = ref ? { model: ref } : null),
})
const modelInfo = computed(() => (form.model ? modelByRef(form.model.model) : null))
const shownEffort = computed<Effort | null>(() => {
  const m = modelInfo.value
  if (!m?.efforts.length) return null
  return nearestEffort(form.model?.effort ?? m.target?.effort ?? 'high', m.efforts)
})

/** The usable skills, then names the rules keep that none is. */
const skillOptions = computed(() => {
  const found = [
    ...new Set(
      (models.settings?.skills.skills ?? []).filter((s) => !s.error && !s.shadowed).map((s) => s.name),
    ),
  ]
  const gone = form.skills.filter((r) => !r.startsWith('!') && !r.includes('*') && !found.includes(r))
  return [...found.map((name) => ({ name, gone: false })), ...gone.map((name) => ({ name, gone: true }))]
})
const allSkills = computed(() => form.skills.includes('*'))

function toggleSkill(name: string, gone: boolean): void {
  form.skills = gone ? form.skills.filter((r) => r !== name) : flip(form.skills, name)
}

function firstLine(text?: string): string {
  return (
    text
      ?.split('\n')
      .find((l) => l.trim())
      ?.trim() ?? ''
  )
}

/** Load the form from the open assistant (a blank one when adding). */
function reset(): void {
  saveError.value = ''
  editingMounts.value.clear()
  pendingDelete.value = false
  Object.assign(form, current.value ? fromAssistant(current.value) : blank())
  pristine.value = JSON.stringify(form)
}

function fromAssistant(assistant: Assistant): Form {
  return {
    id: assistant.id,
    name: assistant.name,
    mounts: assistant.mounts.map((m) => ({ ...m })),
    instructions: assistant.instructions ?? '',
    model: assistant.model ? { ...assistant.model } : null,
    mcp: Object.fromEntries(Object.entries(assistant.mcp).map(([k, v]) => [k, [...v]])),
    skills: [...assistant.skills],
    allow_ask: [...assistant.allow_ask],
    cat: assistant.cat ? { ...assistant.cat, items: [...(assistant.cat.items ?? [])] } : null,
  }
}

function discard(): void {
  Object.assign(form, JSON.parse(pristine.value) as Form)
  editingMounts.value.clear()
  saveError.value = ''
}

/** Re-read the assistants and the decks they may mount. */
async function load(): Promise<void> {
  loadError.value = ''
  try {
    const [, choices] = await Promise.all([
      refreshAssistants(),
      rumu.request<AssistantOptions>('assistant.options'),
      refreshSettings(),
    ])
    options.value = choices
  } catch (err) {
    loadError.value = (err as Error).message
  }
}

async function save(): Promise<void> {
  if (!canSave.value) return
  saveError.value = ''
  pendingDelete.value = false
  const name = form.name.trim()
  saving.value = true
  try {
    const res = await rumu.request<{ deck: Assistant }>('assistant.upsert', {
      id: form.id || undefined,
      name,
      mounts: form.mounts,
      instructions: form.instructions.trim() || undefined,
      model: form.model ?? undefined,
      mcp: form.mcp,
      skills: form.skills,
      allow_ask: form.allow_ask.filter((asker) => asker !== form.id),
      cat: form.cat ?? undefined,
    })
    await load()
    if (isNew.value) replacePage(res.deck.id)
    else reset()
  } catch (err) {
    saveError.value = `${t('assistants.saveError')}: ${(err as Error).message}`
  } finally {
    saving.value = false
  }
}

async function remove(): Promise<void> {
  const id = editing.value
  if (!id) return
  saveError.value = ''
  deleting.value = true
  try {
    await rumu.request('assistant.delete', { id })
    await load()
    if (state.assistant === id) viewAssistant(defaultAssistant())
    replacePage(null)
  } catch (err) {
    saveError.value = `${t('assistants.deleteError')}: ${(err as Error).message}`
  } finally {
    deleting.value = false
  }
}

async function makeDefault(): Promise<void> {
  saveError.value = ''
  try {
    await rumu.request('assistant.set_default', { id: editing.value })
    await refreshAssistants()
  } catch (err) {
    saveError.value = `${t('assistants.saveError')}: ${(err as Error).message}`
  }
}

function toggle(list: string[], value: string): void {
  const i = list.indexOf(value)
  if (i >= 0) list.splice(i, 1)
  else list.push(value)
}

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape' && pendingDelete.value) {
    e.stopPropagation()
    pendingDelete.value = false
  }
}

// The form follows the open assistant, once the list names it.
watch([isNew, () => current.value?.id], reset, { immediate: true })
// A page booted from its address mounts before the socket is up.
watch(connected, (up) => up && void load(), { immediate: true })

onMounted(() => window.addEventListener('keydown', onKey, true))

onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true))
</script>

<template>
  <form class="flex min-h-0 min-w-0 flex-1 flex-col" @submit.prevent="save">
    <div class="min-h-0 flex-1 overflow-y-auto pb-8">
      <!-- the assistant's band with its avatar on the edge, then its name
           (edited in place) and, for one that exists, its id -->
      <header class="hero" :style="{ '--tone': tone }">
        <div class="band" />
        <div class="relative mx-auto w-full max-w-3xl px-4 sm:px-6">
          <AssistantAvatar :id="form.id || undefined" :cat="cat" :size="80" :near="near" class="on-band" />
          <div class="flex items-center gap-2 pt-3">
            <input
              v-model="form.name"
              type="text"
              class="name min-w-0 flex-1"
              :autofocus="isNew"
              maxlength="80"
              autocomplete="off"
              spellcheck="false"
              :placeholder="t('assistants.namePlaceholder')"
              :aria-label="t('assistants.name')"
              @pointerenter="near = true"
              @pointerleave="near = false"
              @focus="near = true"
              @blur="near = false"
            />
            <span
              v-if="isDefault"
              class="flex-none rounded-full bg-surface-2 px-2 py-px text-micro font-semibold text-ink-3"
            >
              {{ t('assistants.defaultBadge') }}
            </span>
          </div>
          <button
            v-if="form.id"
            type="button"
            class="-ml-1.5 flex items-center gap-1.5 rounded-control px-1.5 py-0.5 font-mono text-micro text-ink-3 transition-colors duration-150 hover:bg-surface-2 hover:text-ink"
            :title="t('assistants.copyId')"
            @click="copyId"
          >
            {{ form.id }}
            <Icon :name="copied ? 'ok' : 'copy'" :size="11" />
          </button>
        </div>
      </header>

      <div class="mx-auto w-full max-w-3xl px-4 pt-6 sm:px-6">
        <p
          v-if="loadError"
          class="rounded-control border border-danger/30 bg-danger-soft px-3 py-2 text-secondary text-danger"
        >
          {{ t('assistants.loadError') }}: {{ loadError }}
        </p>

        <div v-else class="flex flex-col gap-7">
          <div class="flex flex-col gap-2" :style="{ '--tone': catTone }">
            <div class="flex items-center gap-2">
              <span class="field-label">{{ t('assistants.look') }}</span>
              <button
                type="button"
                class="-my-1 ml-auto flex items-center gap-1 rounded-control px-1.5 py-0.5 text-meta font-semibold text-ink-3 transition-colors duration-150 hover:bg-surface-2 hover:text-ink"
                @click="shuffleCat"
              >
                <Icon name="dice" :size="13" />
                {{ t('assistants.lookShuffle') }}
              </button>
            </div>
            <div class="flex flex-wrap gap-1.5">
              <button
                v-for="item in CAT_ITEMS"
                :key="item"
                type="button"
                class="chip"
                :class="{ 'tone-neutral': !cat.items?.includes(item) }"
                :aria-pressed="!!cat.items?.includes(item)"
                @click="toggleItem(item)"
              >
                <Icon :name="cat.items?.includes(item) ? 'ok' : 'plus'" :size="12" />
                {{ t(`assistants.catItems.${item}`) }}
              </button>
            </div>
            <div class="flex flex-wrap items-center gap-x-6 gap-y-2 text-meta text-ink-3">
              <div class="flex items-center gap-1.5" role="radiogroup" :aria-label="t('assistants.lookTone')">
                <button
                  v-for="tone in PROJECT_TONES"
                  :key="tone"
                  type="button"
                  role="radio"
                  class="swatch"
                  :style="{ '--swatch': toneColor(tone) }"
                  :aria-checked="(cat.tone ?? 'brand') === tone"
                  :aria-label="tone"
                  @click="editCat({ tone })"
                />
              </div>
              <label class="flex items-center gap-2">
                {{ t('assistants.lookWide') }}
                <input
                  type="range"
                  class="shape"
                  :min="-CAT_SHAPE_MAX"
                  :max="CAT_SHAPE_MAX"
                  step="1"
                  :value="cat.wide"
                  @input="editCat({ wide: +($event.target as HTMLInputElement).value })"
                />
              </label>
              <label class="flex items-center gap-2">
                {{ t('assistants.lookEars') }}
                <input
                  type="range"
                  class="shape"
                  :min="-CAT_SHAPE_MAX"
                  :max="CAT_SHAPE_MAX"
                  step="1"
                  :value="cat.ears"
                  @input="editCat({ ears: +($event.target as HTMLInputElement).value })"
                />
              </label>
            </div>
          </div>

          <label class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('assistants.instructions') }}</span>
            <textarea v-model="form.instructions" rows="8" class="instructions w-full resize-y" />
          </label>

          <div class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('assistants.model') }}</span>
            <div class="flex flex-wrap items-center gap-2">
              <div class="min-w-48 flex-1">
                <ModelField v-model="modelRef" clearable :placeholder="t('assistants.modelDefault')" />
              </div>
              <div
                v-if="form.model && modelInfo && modelInfo.efforts.length > 1"
                class="flex flex-none rounded-control bg-surface-2 p-0.5"
                role="radiogroup"
                :aria-label="t('settings.effort')"
              >
                <button
                  v-for="e in modelInfo.efforts"
                  :key="e"
                  type="button"
                  role="radio"
                  :aria-checked="shownEffort === e"
                  class="rounded-xs px-2 py-0.5 text-meta font-semibold transition-colors duration-150"
                  :class="shownEffort === e ? 'bg-surface text-ink shadow-soft' : 'text-ink-3 hover:text-ink'"
                  @click="form.model = { model: form.model.model, effort: e }"
                >
                  {{ e }}
                </button>
              </div>
            </div>
          </div>

          <div class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('assistants.mcp') }}</span>
            <div class="flex flex-wrap gap-1.5">
              <button
                v-for="server in mcpOptions"
                :key="server.name"
                type="button"
                class="chip font-mono"
                :class="
                  server.gone
                    ? 'tone-danger line-through'
                    : form.mcp[server.name]
                      ? 'tone-tool'
                      : 'tone-neutral'
                "
                :aria-pressed="!!form.mcp[server.name]"
                :title="server.gone ? t('assistants.mcpGone') : undefined"
                @click="toggleServer(server.name)"
              >
                <Icon :name="server.gone ? 'close' : form.mcp[server.name] ? 'ok' : 'plus'" :size="12" />
                {{ server.name }}
              </button>
              <button type="button" class="chip tone-neutral" @click="openPage('settings', 'mcp')">
                <Icon name="settings" :size="12" />
                {{ t('assistants.manageMcp') }}
              </button>
            </div>
            <div
              v-for="server in mcpTools"
              :key="server.name"
              class="rounded-control border border-line-soft"
            >
              <button
                type="button"
                class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-meta"
                :aria-expanded="openTools.has(server.name)"
                @click="
                  openTools.has(server.name) ? openTools.delete(server.name) : openTools.add(server.name)
                "
              >
                <Icon
                  name="chevron"
                  :size="12"
                  class="flex-none text-ink-3 transition-transform duration-150"
                  :class="{ 'rotate-90': openTools.has(server.name) }"
                />
                <span class="font-mono text-ink">{{ server.name }}</span>
                <span class="text-ink-3">{{
                  t('assistants.toolsOn', { n: server.on, total: server.tools.length })
                }}</span>
              </button>
              <div v-if="openTools.has(server.name)" class="border-t border-line-soft">
                <div class="flex gap-1.5 px-3 py-1.5">
                  <button type="button" class="chip tone-neutral" @click="form.mcp[server.name] = ['*']">
                    {{ t('assistants.allTools') }}
                  </button>
                  <button type="button" class="chip tone-neutral" @click="form.mcp[server.name] = []">
                    {{ t('assistants.noTools') }}
                  </button>
                </div>
                <ul class="flex max-h-80 flex-col overflow-y-auto">
                  <li v-for="tool in server.tools" :key="tool.name" class="border-t border-line-soft">
                    <label class="flex cursor-pointer items-baseline gap-2 px-3 py-1.5">
                      <input
                        type="checkbox"
                        class="flex-none self-center"
                        :checked="rulesEnable(form.mcp[server.name]!, tool.name)"
                        @change="toggleTool(server.name, tool.name)"
                      />
                      <span class="flex-none font-mono text-meta text-ink">{{ tool.name }}</span>
                      <span class="min-w-0 truncate text-meta text-ink-3" :title="tool.description">{{
                        firstLine(tool.description)
                      }}</span>
                    </label>
                  </li>
                </ul>
              </div>
            </div>
          </div>

          <div class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('assistants.skills') }}</span>
            <div class="flex flex-wrap gap-1.5">
              <button
                type="button"
                class="chip"
                :class="allSkills ? 'tone-tool' : 'tone-neutral'"
                :aria-pressed="allSkills"
                @click="form.skills = allSkills ? [] : ['*']"
              >
                <Icon :name="allSkills ? 'ok' : 'plus'" :size="12" />
                {{ t('assistants.allTools') }}
              </button>
              <button
                v-for="skill in skillOptions"
                :key="skill.name"
                type="button"
                class="chip font-mono"
                :class="
                  skill.gone
                    ? 'tone-danger line-through'
                    : rulesEnable(form.skills, skill.name)
                      ? 'tone-tool'
                      : 'tone-neutral'
                "
                :aria-pressed="!skill.gone && rulesEnable(form.skills, skill.name)"
                :title="skill.gone ? t('assistants.skillGone') : undefined"
                @click="toggleSkill(skill.name, skill.gone)"
              >
                <Icon
                  :name="skill.gone ? 'close' : rulesEnable(form.skills, skill.name) ? 'ok' : 'plus'"
                  :size="12"
                />
                {{ skill.name }}
              </button>
              <button type="button" class="chip tone-neutral" @click="openPage('settings', 'skills')">
                <Icon name="settings" :size="12" />
                {{ t('assistants.manageMcp') }}
              </button>
            </div>
          </div>

          <div v-if="askOptions.length" class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('assistants.allowAsk') }}</span>
            <div class="flex flex-wrap gap-1.5">
              <button
                v-for="assistant in askOptions"
                :key="assistant.id"
                type="button"
                class="chip"
                :class="form.allow_ask.includes(assistant.id) ? 'tone-agent' : 'tone-neutral'"
                :aria-pressed="form.allow_ask.includes(assistant.id)"
                @click="toggle(form.allow_ask, assistant.id)"
              >
                <Icon :name="form.allow_ask.includes(assistant.id) ? 'ok' : 'plus'" :size="12" />
                {{ assistant.name }}
              </button>
            </div>
          </div>

          <div class="flex flex-col gap-1.5">
            <div class="flex items-center gap-2">
              <span class="field-label">{{ t('assistants.mounts') }}</span>
              <button
                type="button"
                class="-mr-1.5 ml-auto flex items-center gap-1 rounded-control px-1.5 py-0.5 text-meta font-semibold text-ink-3 transition-colors duration-150 hover:bg-surface-2 hover:text-ink"
                @click="addMount"
              >
                <Icon name="plus" :size="12" />
                {{ t('assistants.addMount') }}
              </button>
            </div>

            <!-- One row per mount. A wide form lines them up as a table under
                 column heads; a narrow one stacks each row: the deck, then
                 where it mounts, then whether it is writable. -->
            <div class="mounts overflow-hidden rounded-control border border-line-soft">
              <div
                class="mount-row mount-head border-b border-line-soft bg-canvas/40 text-micro font-semibold text-ink-3"
              >
                <span class="m-deck">{{ t('assistants.deck') }}</span>
                <span class="m-at border border-transparent px-2.5">{{ t('assistants.mountAt') }}</span>
                <span class="m-write text-center">{{ t('assistants.writable') }}</span>
              </div>

              <div class="mount-row bg-surface-2/60 text-ink-3" :title="t('assistants.selfMount')">
                <button
                  v-if="form.id"
                  type="button"
                  class="m-deck -ml-1.5 flex min-w-0 items-center gap-1.5 justify-self-start rounded-control px-1.5 py-1 text-left font-card text-secondary text-ink-2 transition-colors duration-150 hover:bg-surface hover:text-memory"
                  :title="t('library.openInLibrary')"
                  @click="openDeck(form.id)"
                >
                  <Icon name="memory" :size="14" class="flex-none text-ink-3" />
                  <span class="truncate">{{ form.name }}</span>
                </button>
                <span v-else class="m-deck flex min-w-0 items-center gap-1.5 py-1 font-card text-secondary">
                  <Icon name="memory" :size="14" class="flex-none text-ink-3" />
                  <span class="truncate">{{ form.name || '—' }}</span>
                </span>
                <span class="m-at truncate border border-transparent px-2.5 font-mono text-meta"
                  ><span class="text-ink-3/70">{{ PREFIX }}</span
                  >self</span
                >
                <label class="m-write text-meta">
                  <input type="checkbox" class="opacity-40" checked disabled />
                  <span class="m-write-text">{{ t('assistants.writable') }}</span>
                </label>
                <span class="m-act flex justify-center">
                  <Icon name="pin" :size="12" />
                </span>
              </div>

              <div v-for="(mount, i) in form.mounts" :key="i" class="mount-row border-t border-line-soft">
                <span class="m-deck flex min-w-0 items-center gap-1">
                  <span v-if="editingMounts.has(mount)" class="min-w-0 flex-1">
                    <Select
                      :model-value="mount.deck"
                      :options="deckOptions"
                      :label="t('assistants.deck')"
                      :placeholder="t('assistants.pickDeck')"
                      @update:model-value="pickDeck(mount, $event)"
                    />
                  </span>
                  <button
                    v-else
                    type="button"
                    class="-ml-1.5 flex min-w-0 items-center gap-1.5 rounded-control px-1.5 py-1 text-left font-card text-secondary transition-colors duration-150 hover:bg-surface-2 hover:text-memory"
                    :class="badDecks.has(i) ? 'text-danger' : 'text-ink'"
                    :title="t('library.openInLibrary')"
                    @click="openDeck(mount.deck)"
                  >
                    <Icon name="memory" :size="14" class="flex-none text-ink-3" />
                    <span class="truncate">{{ deckName(mount.deck) }}</span>
                  </button>
                  <button
                    type="button"
                    class="icon-btn size-6 flex-none"
                    :title="t('assistants.changeDeck')"
                    :aria-label="t('assistants.changeDeck')"
                    :aria-pressed="editingMounts.has(mount)"
                    @click="toggleEditMount(mount)"
                  >
                    <Icon :name="editingMounts.has(mount) ? 'ok' : 'edit'" :size="12" />
                  </button>
                </span>
                <label
                  class="m-at flex min-w-0 items-center rounded-control border bg-surface px-2.5 font-mono text-meta transition-[border-color,box-shadow] duration-150 focus-within:border-brand focus-within:shadow-[0_0_0_3px_color-mix(in_srgb,var(--brand)_18%,transparent)]"
                  :class="badMounts.has(i) ? 'border-danger' : 'border-line'"
                >
                  <span class="flex-none text-ink-3">{{ PREFIX }}</span>
                  <input
                    :value="mountName(mount.at)"
                    type="text"
                    class="min-w-0 flex-1 rounded-none border-0 bg-transparent px-0 py-1 shadow-none focus:shadow-none"
                    autocomplete="off"
                    spellcheck="false"
                    :aria-label="t('assistants.mountAt')"
                    @input="mount.at = PREFIX + ($event.target as HTMLInputElement).value.trim()"
                  />
                </label>
                <label class="m-write text-meta text-ink-2">
                  <input v-model="mount.write" type="checkbox" />
                  <span class="m-write-text">{{ t('assistants.writable') }}</span>
                </label>
                <button
                  type="button"
                  class="m-act icon-btn size-6"
                  :title="t('assistants.unmount')"
                  :aria-label="t('assistants.unmount')"
                  @click="removeMount(i)"
                >
                  <Icon name="close" :size="12" />
                </button>
              </div>
            </div>
            <span v-if="badMounts.size" class="field-hint text-danger">{{ t('assistants.mountHint') }}</span>
            <span v-if="duplicateDeck" class="field-hint text-danger">{{ t('assistants.mountTaken') }}</span>
          </div>
        </div>
      </div>
    </div>

    <footer v-if="!loadError" class="flex-none border-t border-line-soft py-3">
      <div class="mx-auto flex w-full max-w-3xl flex-col gap-2 px-4 sm:px-6">
        <p v-if="saveError" class="text-meta text-danger">{{ saveError }}</p>
        <div v-if="pendingDelete" class="flex items-center gap-3">
          <Icon name="trash" :size="14" class="flex-none text-danger" />
          <p class="min-w-0 flex-1 text-meta font-semibold text-ink">
            {{ t('assistants.deletePrompt', { name: form.name }) }}
          </p>
          <button type="button" class="btn" @click="pendingDelete = false">
            {{ t('common.cancel') }}
          </button>
          <button type="button" class="btn btn-danger" :disabled="deleting" @click="remove">
            {{ t('assistants.deleteConfirm') }}
          </button>
        </div>
        <div v-else class="flex flex-wrap items-center gap-2">
          <button
            v-if="canDelete"
            type="button"
            class="-ml-2 flex items-center gap-1.5 rounded-control px-2 py-1.5 text-meta font-semibold text-ink-3 transition-colors duration-150 hover:bg-danger-soft hover:text-danger"
            @click="pendingDelete = true"
          >
            <Icon name="trash" :size="14" />
            {{ t('assistants.delete') }}
          </button>
          <button
            v-if="!isNew && !isDefault"
            type="button"
            class="flex items-center gap-1.5 rounded-control px-2 py-1.5 text-meta font-semibold text-ink-3 transition-colors duration-150 hover:bg-surface-2 hover:text-ink"
            @click="makeDefault"
          >
            {{ t('assistants.makeDefault') }}
          </button>
          <button v-if="dirty && !isNew" type="button" class="btn ml-auto" @click="discard">
            {{ t('assistants.discard') }}
          </button>
          <button
            type="submit"
            class="btn btn-primary"
            :class="{ 'ml-auto': !dirty || isNew }"
            :disabled="!canSave"
          >
            {{ isNew ? t('assistants.create') : t('assistants.save') }}
          </button>
        </div>
      </div>
    </footer>
  </form>
</template>

<style scoped>
/* The mounts, sized by their own box: narrow, each row stacks the deck (with
 * its action), the mount point, then the write toggle with its label; wide,
 * they line up as columns under the head row, which names the toggle. */
.mounts {
  container-type: inline-size;
}

.mount-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 1.5rem;
  grid-template-areas:
    'deck act'
    'at at'
    'write write';
  align-items: center;
  gap: 0.375rem 0.75rem;
  padding: 0.5rem 0.75rem;
}

.m-deck {
  grid-area: deck;
}
.m-at {
  grid-area: at;
}
.m-write {
  grid-area: write;
  display: flex;
  align-items: center;
  gap: 0.5rem;
}
.m-act {
  grid-area: act;
}

.mount-head {
  display: none;
}

@container (width >= 34rem) {
  .mount-row {
    grid-template-columns: minmax(0, 2fr) minmax(0, 3fr) 6.5rem 1.5rem;
    grid-template-areas: 'deck at write act';
    padding-block: 0.375rem;
  }

  .mount-head {
    display: grid;
    padding-block: 0.25rem;
  }

  .m-write {
    justify-content: center;
  }

  /* the head row names the column */
  .mount-row:not(.mount-head) .m-write-text {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
}
/* The hero: the band of the assistant's tone, its avatar half on it, the
 * name written under it like on a name card. */
.hero {
  --band: 5rem;
  --k: 1;
}

.band {
  height: var(--band);
  border-bottom: 1px solid color-mix(in oklab, var(--tone) calc(var(--k) * 30%), var(--line-soft));
  background: var(--grain), color-mix(in oklab, var(--tone) calc(var(--k) * 20%), var(--surface));
}

.on-band {
  margin-top: -40px;
}

.name {
  margin-left: -0.5rem;
  border-color: transparent;
  background: transparent;
  padding: 0.125rem 0.5rem;
  font-family: var(--font-card);
  font-size: 1.625rem;
  font-weight: 600;
  line-height: 1.3;
  box-shadow: none;
}

.name:hover {
  border-color: var(--line);
}

.instructions {
  font-family: var(--font-card);
  line-height: 1.65;
}

:global(html[data-theme='dark']) .hero {
  --k: 0.9;
}

/* The look's color picks: a dot of each tone, ringed when chosen. */
.swatch {
  width: 16px;
  height: 16px;
  border-radius: 999px;
  background: var(--swatch);
  box-shadow:
    0 0 0 2px var(--surface),
    0 0 0 3px transparent;
  transition: box-shadow 0.15s;
}

.swatch[aria-checked='true'] {
  box-shadow:
    0 0 0 2px var(--surface),
    0 0 0 3.5px var(--swatch);
}

.shape {
  width: 88px;
  accent-color: var(--tone);
}
</style>
