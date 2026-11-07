<script setup lang="ts">
// Assistant configuration: the roster's settings dialog. Each assistant is a
// deck (`decks/<name>/deck.yaml`) with a pinned `instructions.md`; this form
// edits its name, workspaces, default mounts, instructions and who may
// ask it. Saves go through `assistant.upsert` / `assistant.delete`;
// `assistant.set_default` picks the one sessions run as by default.
import { computed, onBeforeUnmount, reactive, ref, watch } from 'vue'
import { t } from '../i18n'
import { rumu } from '../lib/api'
import { closeAssistantSettings, layout } from '../lib/layout'
import { defaultAssistant, openLibraryAtDeck, refreshAssistants, state, viewAssistant } from '../lib/store'
import type { Assistant, AssistantOptions, DeckMount } from '../lib/types'
import RumuCat from './brand/RumuCat.vue'
import Icon from './ui/Icon.vue'
import Modal from './ui/Modal.vue'
import Select from './ui/Select.vue'
import type { SelectOption } from './ui/Select.vue'

const NEW = '__new__'

interface Form {
  id: string
  name: string
  mounts: DeckMount[]
  /** Attached to its new sessions; the first is the default. */
  workspaces: string[]
  instructions: string
  allow_ask: string[]
}

function blank(): Form {
  return {
    id: '',
    name: '',
    mounts: [],
    workspaces: [],
    instructions: '',
    allow_ask: [],
  }
}

const options = ref<AssistantOptions>({ decks: [] })
const form = reactive<Form>(blank())
const pristine = ref(JSON.stringify(blank()))
const editing = ref<string>(NEW)
const pendingDelete = ref(false)
const saving = ref(false)
const deleting = ref(false)
const loadError = ref('')
const saveError = ref('')

/** The default assistant first: it is the one everything falls back to. */
const assistants = computed(() => [...state.assistants].sort((a, b) => Number(b.default) - Number(a.default)))
const isNew = computed(() => editing.value === NEW)
const isDefault = computed(() => state.assistants.some((a) => a.id === editing.value && a.default))
/** The last assistant cannot go: sessions need one to run as. */
const canDelete = computed(() => !isNew.value && state.assistants.length > 1)
const dirty = computed(() => JSON.stringify(form) !== pristine.value)
const nameInvalid = computed(() => isNew.value && !!form.name && !/^[a-z][a-z0-9-]*$/.test(form.name))
const canSave = computed(
  () =>
    !saving.value &&
    dirty.value &&
    !!form.name.trim() &&
    !nameInvalid.value &&
    !badMounts.value.size &&
    !badDecks.value.size,
)

/** Decks a mount row may name; the assistant's own deck is the fixed `/cards/self`. */
const deckOptions = computed<SelectOption[]>(() =>
  options.value.decks
    .filter((d) => d.id !== form.id)
    .map((d) => ({ value: d.id, label: d.name, icon: 'memory' })),
)

/** Mount rows showing the deck dropdown instead of the deck link. */
const editingMounts = ref(new Set<DeckMount>())

/** Jump to a deck's row in the card library, which opens over this dialog. */
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

/** Registered workspaces the assistant does not attach yet. */
const otherWorkspaces = computed(() =>
  state.workspaces.map((w) => w.name).filter((n) => !form.workspaces.includes(n)),
)

/** Move a workspace to the front: the first is the default. */
function makeDefaultWorkspace(name: string): void {
  form.workspaces = [name, ...form.workspaces.filter((n) => n !== name)]
}

const askOptions = computed(() => assistants.value.filter((a) => a.id !== form.id))

function choose(id: string): void {
  saveError.value = ''
  editingMounts.value.clear()
  pendingDelete.value = false
  const assistant = assistants.value.find((a) => a.id === id)
  editing.value = assistant ? id : NEW
  Object.assign(form, assistant ? fromAssistant(assistant) : blank())
  pristine.value = JSON.stringify(form)
}

function fromAssistant(assistant: Assistant): Form {
  return {
    id: assistant.id,
    name: assistant.name,
    mounts: assistant.mounts.map((m) => ({ ...m })),
    workspaces: [...assistant.workspaces],
    instructions: assistant.instructions ?? '',
    allow_ask: [...assistant.allow_ask],
  }
}

function discard(): void {
  Object.assign(form, JSON.parse(pristine.value) as Form)
  editingMounts.value.clear()
  saveError.value = ''
}

async function load(select?: string): Promise<void> {
  loadError.value = ''
  try {
    const [list, choices] = await Promise.all([
      refreshAssistants(),
      rumu.request<AssistantOptions>('assistant.options'),
    ])
    options.value = choices
    const wanted = select ?? editing.value
    const next = list.some((a) => a.id === wanted)
      ? wanted
      : list.some((a) => a.id === state.assistant)
        ? state.assistant
        : (list[0]?.id ?? NEW)
    choose(next)
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
      workspaces: form.workspaces,
      instructions: form.instructions.trim() || undefined,
      allow_ask: form.allow_ask.filter((asker) => asker !== form.id),
    })
    await load(res.deck.id)
  } catch (err) {
    saveError.value = `${t('assistants.saveError')}: ${(err as Error).message}`
  } finally {
    saving.value = false
  }
}

async function remove(): Promise<void> {
  const id = editing.value
  if (!id || id === NEW) return
  saveError.value = ''
  deleting.value = true
  try {
    await rumu.request('assistant.delete', { id })
    await load()
    if (state.assistant === id) viewAssistant(defaultAssistant())
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
  // The library may sit over this dialog; its Escape is its own.
  if (e.key === 'Escape' && pendingDelete.value && !state.library.isOpen) {
    e.stopPropagation()
    pendingDelete.value = false
  }
}

watch(
  () => layout.assistantSettingsOpen,
  (open) => {
    if (open) {
      window.addEventListener('keydown', onKey, true)
      void load(layout.assistantSettingsFor ?? undefined)
    } else {
      window.removeEventListener('keydown', onKey, true)
      pendingDelete.value = false
      editingMounts.value.clear()
      loadError.value = ''
      saveError.value = ''
    }
  },
)

onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true))
</script>

<template>
  <Modal
    :open="layout.assistantSettingsOpen"
    :label="t('assistants.title')"
    :z="55"
    wrapper-class="p-3 sm:p-6"
    panel-class="h-[min(84dvh,680px)] w-full max-w-[840px] rounded-sheet"
    @close="closeAssistantSettings"
  >
    <div class="flex min-h-0 flex-1 flex-col sm:flex-row">
      <nav
        class="flex flex-none flex-col border-b border-line-soft bg-canvas/40 sm:w-60 sm:border-r sm:border-b-0"
      >
        <header class="flex items-center gap-2 px-5 pt-5 pb-3">
          <h2 class="text-title font-bold">{{ t('assistants.title') }}</h2>
          <span class="text-meta font-semibold text-ink-3 tabular-nums">{{ assistants.length }}</span>
        </header>
        <div class="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-3 pb-3">
          <button
            v-for="assistant in assistants"
            :key="assistant.id"
            type="button"
            class="group flex items-center gap-2.5 rounded-control px-2.5 py-2 text-left transition-colors duration-150"
            :class="
              assistant.id === editing
                ? 'bg-surface font-semibold text-ink shadow-soft'
                : 'text-ink-2 hover:bg-surface/70 hover:text-ink'
            "
            @click="choose(assistant.id)"
          >
            <RumuCat :size="22" :paper="false" />
            <span class="flex min-w-0 flex-1 flex-col">
              <span class="truncate text-secondary">{{ assistant.name }}</span>
              <span class="truncate font-mono text-micro font-normal text-ink-3">{{
                assistant.id.slice(0, 8)
              }}</span>
            </span>
          </button>
          <button
            type="button"
            class="mt-1 flex items-center gap-2.5 rounded-control border border-dashed px-2.5 py-2 text-left text-secondary transition-colors duration-150"
            :class="
              isNew
                ? 'border-brand/50 bg-brand-soft/50 font-semibold text-ink'
                : 'border-line text-ink-3 hover:border-ink-3 hover:text-ink'
            "
            @click="choose(NEW)"
          >
            <span class="flex size-[22px] flex-none items-center justify-center">
              <Icon name="plus" :size="14" />
            </span>
            {{ t('assistants.new') }}
          </button>
        </div>
      </nav>

      <form class="flex min-h-0 min-w-0 flex-1 flex-col" @submit.prevent="save">
        <header class="flex flex-none items-center gap-3 px-6 pt-5 pb-1">
          <template v-if="!isNew">
            <h3 class="min-w-0 truncate text-title font-bold">{{ form.name }}</h3>
            <span
              v-if="isDefault"
              class="flex-none rounded-full bg-surface-2 px-2 py-px text-micro font-semibold text-ink-3"
            >
              {{ t('assistants.defaultBadge') }}
            </span>
          </template>
          <h3 v-else class="text-title font-bold">{{ t('assistants.new') }}</h3>
          <button
            type="button"
            class="icon-btn ml-auto size-8 flex-none"
            :title="t('common.close')"
            :aria-label="t('common.close')"
            @click="closeAssistantSettings"
          >
            <Icon name="close" :size="14" />
          </button>
        </header>

        <div class="min-h-0 flex-1 overflow-y-auto px-6 pt-3 pb-6">
          <p
            v-if="loadError"
            class="rounded-control border border-danger/30 bg-danger-soft px-3 py-2 text-secondary text-danger"
          >
            {{ t('assistants.loadError') }}: {{ loadError }}
          </p>

          <div v-else class="flex flex-col gap-5">
            <label v-if="isNew" class="flex flex-col gap-1.5">
              <span class="field-label">{{ t('assistants.name') }}</span>
              <input v-model="form.name" type="text" class="w-full" autocomplete="off" spellcheck="false" />
              <span v-if="nameInvalid" class="field-hint text-danger">{{ t('assistants.nameHint') }}</span>
            </label>

            <div class="flex flex-col gap-1.5">
              <span class="field-label">{{ t('assistants.workspaces') }}</span>
              <div class="flex flex-wrap gap-1.5">
                <span
                  v-for="(name, i) in form.workspaces"
                  :key="name"
                  class="chip tone-agent gap-0.5 pr-1 pl-1"
                >
                  <span v-if="i === 0" class="px-1 text-micro opacity-70">{{ t('workspaces.default') }}</span>
                  <button
                    v-else
                    type="button"
                    class="icon-btn size-5 opacity-50 hover:opacity-100"
                    :title="t('workspaces.makeDefault')"
                    :aria-label="t('workspaces.makeDefault')"
                    @click="makeDefaultWorkspace(name)"
                  >
                    <Icon name="pin" :size="11" />
                  </button>
                  <span class="font-card">{{ name }}</span>
                  <button
                    type="button"
                    class="icon-btn size-5"
                    :title="t('assistants.removeWorkspace')"
                    :aria-label="t('assistants.removeWorkspace')"
                    @click="toggle(form.workspaces, name)"
                  >
                    <Icon name="close" :size="11" />
                  </button>
                </span>
                <button
                  v-for="name in otherWorkspaces"
                  :key="name"
                  type="button"
                  class="chip tone-neutral border-dashed"
                  :title="t('assistants.addWorkspace')"
                  @click="toggle(form.workspaces, name)"
                >
                  <Icon name="plus" :size="12" />
                  <span class="font-card">{{ name }}</span>
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
              <div class="overflow-hidden rounded-control border border-line-soft">
                <div
                  class="mount-row border-b border-line-soft bg-canvas/40 py-1 text-micro font-semibold text-ink-3"
                >
                  <span>{{ t('assistants.deck') }}</span>
                  <span class="border border-transparent px-2.5">{{ t('assistants.mountAt') }}</span>
                  <span class="text-center">{{ t('assistants.writable') }}</span>
                  <span />
                </div>
                <div class="mount-row bg-surface-2/60 py-1.5 text-ink-3" :title="t('assistants.selfMount')">
                  <button
                    v-if="form.id"
                    type="button"
                    class="flex min-w-0 items-center gap-1.5 rounded-control px-1.5 py-1 -ml-1.5 text-left font-card text-secondary text-ink-2 transition-colors duration-150 hover:bg-surface hover:text-memory"
                    :title="t('library.openInLibrary')"
                    @click="openDeck(form.id)"
                  >
                    <Icon name="memory" :size="14" class="flex-none text-ink-3" />
                    <span class="truncate">{{ form.name }}</span>
                  </button>
                  <span v-else class="flex min-w-0 items-center gap-1.5 font-card text-secondary">
                    <Icon name="memory" :size="14" class="flex-none text-ink-3" />
                    <span class="truncate">{{ form.name || '—' }}</span>
                  </span>
                  <span class="truncate border border-transparent px-2.5 font-mono text-meta"
                    ><span class="text-ink-3/70">/cards/</span>self</span
                  >
                  <input type="checkbox" class="justify-self-center opacity-40" checked disabled />
                  <span class="flex justify-center">
                    <Icon name="pin" :size="12" />
                  </span>
                </div>
                <div
                  v-for="(mount, i) in form.mounts"
                  :key="i"
                  class="mount-row border-t border-line-soft py-1.5"
                >
                  <span class="flex min-w-0 items-center gap-1">
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
                      class="flex min-w-0 items-center gap-1.5 rounded-control px-1.5 py-1 -ml-1.5 text-left font-card text-secondary transition-colors duration-150 hover:bg-surface-2 hover:text-memory flex-1"
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
                    class="flex min-w-0 items-center rounded-control border bg-surface px-2.5 font-mono text-meta transition-[border-color,box-shadow] duration-150 focus-within:border-brand focus-within:shadow-[0_0_0_3px_color-mix(in_srgb,var(--brand)_18%,transparent)]"
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
                  <input
                    v-model="mount.write"
                    type="checkbox"
                    class="justify-self-center"
                    :aria-label="t('assistants.writable')"
                  />
                  <button
                    type="button"
                    class="icon-btn size-6"
                    :aria-label="t('assistants.unmount')"
                    @click="removeMount(i)"
                  >
                    <Icon name="close" :size="12" />
                  </button>
                </div>
              </div>
              <span v-if="badMounts.size" class="field-hint text-danger">{{
                t('assistants.mountHint')
              }}</span>
              <span v-if="duplicateDeck" class="field-hint text-danger">{{
                t('assistants.mountTaken')
              }}</span>
            </div>

            <label class="flex flex-col gap-1.5">
              <span class="field-label">{{ t('assistants.instructions') }}</span>
              <textarea v-model="form.instructions" rows="4" class="w-full resize-y" />
            </label>

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
          </div>
        </div>

        <footer v-if="!loadError" class="flex flex-none flex-col gap-2 border-t border-line-soft px-6 py-3">
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
          <div v-else class="flex items-center gap-2">
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
        </footer>
      </form>
    </div>
  </Modal>
</template>

<style scoped>
/* deck | mount point | write | action, aligned across the header and every row */
.mount-row {
  display: grid;
  grid-template-columns: minmax(0, 2fr) minmax(0, 3fr) 6.5rem 1.5rem;
  align-items: center;
  column-gap: 0.75rem;
  padding-inline: 0.75rem;
}
</style>
