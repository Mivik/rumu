<script setup lang="ts">
// Browse the local disk for a folder (`fs.dirs`). Opening a folder picks it:
// a bound field follows the browsing, and `path` is the open one. The list
// fills what height the root is given. The path bar also takes a typed path:
// the list then offers the typed parent's folders matching the last segment,
// Tab completes, the arrows choose and Enter opens. Dot folders show only
// when the segment starts with a dot.
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { t } from '../i18n'
import { rumu } from '../lib/api'
import Icon from './ui/Icon.vue'

interface Listing {
  path: string
  parent: string | null
  dirs: string[]
}

const props = defineProps<{ start: string }>()
const emit = defineEmits<{ pick: [path: string] }>()

const listing = ref<Listing | null>(null)
const error = ref('')
const crumbsEl = ref<HTMLElement | null>(null)
const back = ref<string[]>([])
const forward = ref<string[]>([])

/** A Windows path (`C:\…`): split on both slashes, joined with `\`. */
const isWindows = (path: string): boolean => /^[A-Za-z]:/.test(path)
const sepOf = (path: string): string => (isWindows(path) ? '\\' : '/')

/** Each ancestor of the open folder, root first. */
function crumbs(path: string): { name: string; path: string }[] {
  if (isWindows(path)) {
    const [drive, ...parts] = path.split(/[\\/]/).filter(Boolean)
    const root = `${drive}\\`
    return [
      { name: root, path: root },
      ...parts.map((name, i) => ({ name, path: root + parts.slice(0, i + 1).join('\\') })),
    ]
  }
  const parts = path.split('/').filter(Boolean)
  return [
    { name: '/', path: '/' },
    ...parts.map((name, i) => ({ name, path: '/' + parts.slice(0, i + 1).join('/') })),
  ]
}

function join(base: string, name: string): string {
  const sep = sepOf(base)
  return base.endsWith(sep) ? base + name : `${base}${sep}${name}`
}

async function load(path: string): Promise<boolean> {
  try {
    listing.value = await rumu.request<Listing>('fs.dirs', { path })
    error.value = ''
    await nextTick()
    crumbsEl.value?.scrollTo({ left: crumbsEl.value.scrollWidth })
    return true
  } catch (err) {
    error.value = (err as Error).message
    return false
  }
}

/** Open a folder as a new step in the history. */
async function open(path: string): Promise<boolean> {
  const from = listing.value?.path
  if (!(await load(path))) return false
  const to = listing.value!.path
  if (from && from !== to) {
    back.value.push(from)
    forward.value = []
  }
  emit('pick', to)
  return true
}

/** Step back or forward: the top of `from` opens, and here goes onto `to`. */
async function travel(from: string[], to: string[]): Promise<void> {
  const path = from.at(-1)
  const here = listing.value?.path
  if (!path || !here || !(await load(path))) return
  from.pop()
  to.push(here)
  emit('pick', listing.value!.path)
}

defineExpose({ path: computed(() => listing.value?.path ?? '') })

const shown = computed(() => listing.value?.dirs.filter((n) => !n.startsWith('.')) ?? [])

// ---- Typed path ----

const editing = ref(false)
const draft = ref('')
const inputEl = ref<HTMLInputElement | null>(null)
/** The folders of the draft's parent, keyed by the parent as typed. */
const typed = ref<{ base: string; listing: Listing | null }>({ base: '', listing: null })
/** The highlighted suggestion, or -1 for the draft itself. */
const active = ref(-1)

/** The draft split at its last slash: the parent as typed, and the segment. */
const split = computed(() => {
  const d = draft.value
  const at = isWindows(d) ? Math.max(d.lastIndexOf('/'), d.lastIndexOf('\\')) : d.lastIndexOf('/')
  return { base: draft.value.slice(0, at + 1), segment: draft.value.slice(at + 1) }
})

const suggestions = computed(() => {
  const { base, segment } = split.value
  if (typed.value.base !== base || !typed.value.listing) return []
  const lower = segment.toLowerCase()
  return typed.value.listing.dirs.filter(
    (n) => n.toLowerCase().startsWith(lower) && (segment.startsWith('.') || !n.startsWith('.')),
  )
})

watch(
  () => split.value.base,
  async (base) => {
    if (!editing.value || base === typed.value.base) return
    typed.value = { base, listing: null }
    if (!base) return
    try {
      const found = await rumu.request<Listing>('fs.dirs', { path: base })
      // A later keystroke may have moved on to another parent.
      if (typed.value.base === base) typed.value.listing = found
    } catch {
      // A parent that doesn't exist just has nothing to offer.
    }
  },
)
watch(draft, () => (active.value = -1))

async function startEdit(): Promise<void> {
  if (!listing.value) return
  const path = listing.value.path
  const base = join(path, '')
  typed.value = { base, listing: listing.value }
  draft.value = base
  editing.value = true
  await nextTick()
  inputEl.value?.focus()
}

function stopEdit(): void {
  editing.value = false
}

async function submit(): Promise<void> {
  const name = suggestions.value[active.value]
  const path = name ? join(typed.value.listing!.path, name) : draft.value.trim()
  if (path && (await open(path))) stopEdit()
}

/** Complete the segment as far as the matches agree, then into the folder. */
function complete(): void {
  const names = suggestions.value
  if (!names.length) return
  let common = names[0]!
  for (const n of names.slice(1)) {
    let i = 0
    while (i < common.length && common[i] === n[i]) i++
    common = common.slice(0, i)
  }
  // Matching ignores case, so the agreed prefix may be shorter than the typed one.
  if (common.length < split.value.segment.length) return
  draft.value = split.value.base + common + (names.length === 1 ? sepOf(draft.value) : '')
}

function onKey(e: KeyboardEvent): void {
  const n = suggestions.value.length
  if (e.key === 'Tab' && n) {
    e.preventDefault()
    complete()
  } else if (e.key === 'ArrowDown' && n) {
    e.preventDefault()
    active.value = (active.value + 1) % n
  } else if (e.key === 'ArrowUp' && n) {
    e.preventDefault()
    active.value = active.value <= 0 ? n - 1 : active.value - 1
  } else if (e.key === 'Enter') {
    e.preventDefault()
    void submit()
  } else if (e.key === 'Escape') {
    e.preventDefault()
    e.stopPropagation()
    stopEdit()
  }
}

async function pickSuggestion(name: string): Promise<void> {
  if (await open(join(typed.value.listing!.path, name))) stopEdit()
}

onMounted(async () => {
  if (!(props.start.trim() && (await load(props.start.trim())))) await load('~')
})
</script>

<template>
  <div class="flex flex-col overflow-hidden rounded-control border border-line bg-surface">
    <div v-if="listing" class="flex items-center gap-0.5 border-b border-line-soft px-1.5 py-1">
      <button
        type="button"
        class="icon-btn size-6 flex-none"
        :disabled="!back.length"
        :title="t('workspaces.back')"
        :aria-label="t('workspaces.back')"
        @click="travel(back, forward)"
      >
        <Icon name="back" :size="13" />
      </button>
      <button
        type="button"
        class="icon-btn size-6 flex-none"
        :disabled="!forward.length"
        :title="t('workspaces.forward')"
        :aria-label="t('workspaces.forward')"
        @click="travel(forward, back)"
      >
        <Icon name="forward" :size="13" />
      </button>
      <button
        type="button"
        class="icon-btn size-6 flex-none"
        :disabled="!listing.parent"
        :title="t('workspaces.parentFolder')"
        :aria-label="t('workspaces.parentFolder')"
        @click="listing.parent && open(listing.parent)"
      >
        <Icon name="up" :size="13" />
      </button>
      <button
        type="button"
        class="icon-btn size-6 flex-none"
        :title="t('workspaces.home')"
        :aria-label="t('workspaces.home')"
        @click="open('~')"
      >
        <Icon name="home" :size="13" />
      </button>

      <input
        v-if="editing"
        ref="inputEl"
        v-model="draft"
        type="text"
        class="ml-1 h-6 min-w-0 flex-1 px-1.5 py-0 font-card text-meta"
        role="combobox"
        aria-autocomplete="list"
        :aria-expanded="suggestions.length > 0"
        :aria-label="t('workspaces.editPath')"
        autocomplete="off"
        spellcheck="false"
        @keydown="onKey"
        @blur="stopEdit"
      />
      <template v-else>
        <div
          ref="crumbsEl"
          class="ml-1 flex min-w-0 flex-1 cursor-text items-center self-stretch overflow-x-auto font-card text-meta whitespace-nowrap"
          @click.self="startEdit"
        >
          <template v-for="(c, i) in crumbs(listing.path)" :key="c.path">
            <span v-if="i > 1" class="px-0.5 text-ink-3/60">/</span>
            <button
              type="button"
              class="rounded-xs px-1 py-0.5 transition-colors duration-150 hover:bg-surface-2"
              :class="c.path === listing.path ? 'font-semibold text-ink' : 'text-ink-3 hover:text-ink'"
              @click="open(c.path)"
            >
              {{ c.name }}
            </button>
          </template>
        </div>
        <button
          type="button"
          class="icon-btn size-6 flex-none"
          :title="t('workspaces.editPath')"
          :aria-label="t('workspaces.editPath')"
          @click="startEdit"
        >
          <Icon name="edit" :size="13" />
        </button>
      </template>
    </div>
    <p v-if="error" class="px-3 py-2 text-meta text-danger">{{ error }}</p>

    <div v-if="editing" class="flex min-h-0 flex-1 flex-col overflow-y-auto p-1" role="listbox">
      <button
        v-for="(name, i) in suggestions"
        :key="name"
        type="button"
        role="option"
        :aria-selected="i === active"
        class="flex items-center gap-2 rounded-xs px-2 py-1 text-left font-card text-secondary transition-colors duration-150 hover:bg-surface-2 hover:text-ink"
        :class="i === active ? 'bg-surface-2 text-ink' : 'text-ink-2'"
        @mousedown.prevent
        @click="pickSuggestion(name)"
      >
        <Icon name="mount" :size="13" class="flex-none text-ink-3" />
        <span class="truncate">{{ name }}</span>
      </button>
    </div>
    <div v-else-if="shown.length" class="flex min-h-0 flex-1 flex-col overflow-y-auto p-1">
      <button
        v-for="name in shown"
        :key="name"
        type="button"
        class="flex items-center gap-2 rounded-xs px-2 py-1 text-left font-card text-secondary text-ink-2 transition-colors duration-150 hover:bg-surface-2 hover:text-ink"
        @click="open(join(listing!.path, name))"
      >
        <Icon name="mount" :size="13" class="flex-none text-ink-3" />
        <span class="truncate">{{ name }}</span>
      </button>
    </div>
    <p v-else-if="listing && !error" class="px-3 py-2 text-meta text-ink-3">
      {{ t('workspaces.noFolders') }}
    </p>
  </div>
</template>
