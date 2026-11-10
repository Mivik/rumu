<script setup lang="ts">
// The assistants page (`/assistants/<id>`, `?new` for a new one). Each
// assistant is a deck (`decks/<id>/deck.yaml`) with a pinned
// `instructions.md`; the form edits its name, default mounts, instructions
// and who may ask it. Saves go through `assistant.upsert` /
// `assistant.delete`; `assistant.set_default` picks the one sessions run as
// by default.
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { t } from '../../i18n'
import { rumu } from '../../lib/api'
import { openPage, pageUp, replacePage } from '../../lib/router'
import {
  connected,
  defaultAssistant,
  openLibraryAtDeck,
  refreshAssistants,
  state,
  viewAssistant,
} from '../../lib/store'
import type { Assistant, AssistantOptions, DeckMount } from '../../lib/types'
import RumuCat from '../brand/RumuCat.vue'
import Icon from '../ui/Icon.vue'
import Select from '../ui/Select.vue'
import type { SelectOption } from '../ui/Select.vue'
import PageSheet from './PageSheet.vue'

interface Form {
  id: string
  name: string
  mounts: DeckMount[]
  instructions: string
  allow_ask: string[]
}

function blank(): Form {
  return {
    id: '',
    name: '',
    mounts: [],
    instructions: '',
    allow_ask: [],
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
const isDefault = computed(() => !!current.value?.default)
/** The last assistant cannot go: sessions need one to run as. */
const canDelete = computed(() => !isNew.value && state.assistants.length > 1)
const dirty = computed(() => JSON.stringify(form) !== pristine.value)
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
    allow_ask: [...assistant.allow_ask],
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
      allow_ask: form.allow_ask.filter((asker) => asker !== form.id),
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
  <PageSheet
    :title="t('assistants.title')"
    :count="assistants.length"
    :detail="isNew || !!current"
    :pick="t('assistants.pick')"
    :item="editing"
    @back="pageUp"
  >
    <template #icon>
      <RumuCat :size="18" :paper="false" class="flex-none" />
    </template>

    <template #list>
      <div class="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto p-3">
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
          @click="openPage('assistants', assistant.id)"
        >
          <RumuCat :size="22" :paper="false" />
          <span class="flex min-w-0 flex-1 flex-col">
            <span class="truncate text-secondary">{{ assistant.name }}</span>
            <span class="truncate font-mono text-micro font-normal text-ink-3">{{
              assistant.id.slice(0, 8)
            }}</span>
          </span>
          <span
            v-if="assistant.default"
            class="flex-none rounded-full bg-surface-2 px-2 py-px text-micro font-semibold text-ink-3"
          >
            {{ t('assistants.defaultBadge') }}
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
          @click="openPage('assistants', null, true)"
        >
          <span class="flex size-[22px] flex-none items-center justify-center">
            <Icon name="plus" :size="14" />
          </span>
          {{ t('assistants.new') }}
        </button>
      </div>
    </template>

    <form class="flex min-h-0 min-w-0 flex-1 flex-col" @submit.prevent="save">
      <header class="flex flex-none items-center gap-3 px-4 pt-5 pb-1 sm:px-6">
        <template v-if="!isNew">
          <h2 class="min-w-0 truncate text-title font-bold">{{ form.name }}</h2>
          <span
            v-if="isDefault"
            class="flex-none rounded-full bg-surface-2 px-2 py-px text-micro font-semibold text-ink-3"
          >
            {{ t('assistants.defaultBadge') }}
          </span>
        </template>
        <h2 v-else class="text-title font-bold">{{ t('assistants.new') }}</h2>
      </header>

      <div class="min-h-0 flex-1 overflow-y-auto px-4 pt-3 pb-6 sm:px-6">
        <p
          v-if="loadError"
          class="rounded-control border border-danger/30 bg-danger-soft px-3 py-2 text-secondary text-danger"
        >
          {{ t('assistants.loadError') }}: {{ loadError }}
        </p>

        <div v-else class="flex flex-col gap-5">
          <label v-if="isNew" class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('assistants.name') }}</span>
            <input
              v-model.trim="form.name"
              type="text"
              class="w-full"
              maxlength="80"
              autocomplete="off"
              spellcheck="false"
            />
          </label>

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

          <label class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('assistants.instructions') }}</span>
            <textarea v-model="form.instructions" rows="6" class="w-full resize-y" />
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

      <footer
        v-if="!loadError"
        class="flex flex-none flex-col gap-2 border-t border-line-soft px-4 py-3 sm:px-6"
      >
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
      </footer>
    </form>
  </PageSheet>
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
</style>
