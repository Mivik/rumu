<script setup lang="ts">
// A card's history: one row per commit, and the selected version's diff.
import { computed, ref, watch } from 'vue'
import { t } from '../../i18n'
import { cardHistory, cardVersion, closeLibrary, openSession, state } from '../../lib/store'
import type { FilePatch, LibraryCommit, LibraryVersion } from '../../lib/types'
import { relTime } from '../../lib/util'
import FileDiff from '../chat/FileDiff.vue'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ deck: string; path: string }>()
defineEmits<{ back: [] }>()

const commits = ref<LibraryCommit[]>([])
const selected = ref<LibraryCommit | null>(null)
const version = ref<LibraryVersion | null>(null)
const loading = ref(false)
const versionLoading = ref(false)
const error = ref('')

function authorLabel(author: string): string {
  if (author === 'user') return t('library.you')
  if (author === 'system') return t('library.system')
  return author
}

function toSession(id: string): void {
  closeLibrary()
  void openSession(id)
}

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    commits.value = await cardHistory(props.deck, props.path)
    if (commits.value.length) await pick(commits.value[0]!)
    else {
      selected.value = null
      version.value = null
    }
  } catch (err) {
    error.value = (err as Error).message
  } finally {
    loading.value = false
  }
}

async function pick(commit: LibraryCommit): Promise<void> {
  selected.value = commit
  versionLoading.value = true
  try {
    version.value = await cardVersion(props.deck, props.path, commit.commit)
  } catch (err) {
    error.value = (err as Error).message
    version.value = null
  } finally {
    versionLoading.value = false
  }
}

watch(
  () => [props.deck, props.path],
  () => void load(),
  { immediate: true },
)

const patch = computed<FilePatch | null>(() =>
  version.value
    ? {
        kind: version.value.previous === null ? 'create' : 'update',
        hunks: version.value.hunks,
      }
    : null,
)
const args = computed<Record<string, unknown> | null>(() =>
  version.value?.previous === null ? { content: version.value.view ?? '' } : null,
)
const session = computed(() => {
  const id = selected.value?.session
  return id && state.sessions.some((s) => s.id === id) ? id : null
})
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div class="flex flex-none items-center gap-2 px-4 pt-3 pb-2 sm:px-6">
      <button
        type="button"
        class="icon-btn size-8"
        :title="t('library.back')"
        :aria-label="t('library.back')"
        @click="$emit('back')"
      >
        <Icon name="chevron" :size="15" class="rotate-180" />
      </button>
      <h3 class="text-title font-bold">{{ t('library.history') }}</h3>
      <span v-if="commits.length" class="text-meta text-ink-3 tabular-nums">{{ commits.length }}</span>
      <div class="ml-auto flex items-center gap-1">
        <slot name="actions" />
      </div>
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto px-4 pb-4 sm:px-6">
      <p
        v-if="error"
        class="rounded-control border border-danger/30 bg-danger-soft px-3 py-2 text-secondary text-danger"
      >
        {{ error }}
      </p>
      <p v-else-if="loading" class="py-8 text-center text-secondary text-ink-3">…</p>
      <p v-else-if="!commits.length" class="py-8 text-center text-secondary text-ink-3">
        {{ t('library.noHistory') }}
      </p>

      <ul v-else class="flex flex-col gap-0.5">
        <li v-for="commit in commits" :key="commit.commit">
          <button
            type="button"
            class="flex w-full min-w-0 flex-col gap-0.5 rounded-control px-2.5 py-2 text-left transition-colors duration-150"
            :class="selected?.commit === commit.commit ? 'bg-surface-2' : 'hover:bg-surface-2/60'"
            @click="pick(commit)"
          >
            <span class="flex min-w-0 items-baseline gap-2">
              <span class="min-w-0 flex-1 truncate text-meta text-ink">
                {{ commit.subject }}
              </span>
              <span class="flex-none text-micro text-ink-3 tabular-nums">
                {{ relTime(commit.at * 1000) }}
              </span>
            </span>
            <span class="flex min-w-0 items-center gap-2 text-micro text-ink-3">
              <span class="truncate">{{ authorLabel(commit.author) }}</span>
              <span class="font-mono">{{ commit.short }}</span>
              <span v-if="commit.path" class="truncate">{{ commit.path }}</span>
            </span>
          </button>
        </li>
      </ul>

      <section v-if="selected" class="mt-3 border-t border-line-soft pt-3">
        <div class="flex flex-wrap items-center gap-x-3 gap-y-1 px-1 pb-2 text-meta text-ink-3">
          <span class="font-semibold text-ink-2">{{ t('library.version') }}</span>
          <span>{{ selected.subject }}</span>
          <span class="font-mono">{{ selected.short }}</span>
          <button
            v-if="session"
            type="button"
            class="ml-auto font-semibold text-brand-strong underline decoration-brand-strong/30 underline-offset-2 hover:decoration-current"
            @click="toSession(session)"
          >
            {{ t('library.openSession') }}
          </button>
        </div>
        <div v-if="versionLoading" class="py-4 text-center text-meta text-ink-3">…</div>
        <template v-else-if="version">
          <p v-if="version.view === null" class="px-1 py-2 text-meta text-ink-3">
            {{ t('library.deletedVersion') }}
          </p>
          <FileDiff :patch="patch" :args="args" :path="`${path}.md`" />
        </template>
      </section>
    </div>
  </div>
</template>
