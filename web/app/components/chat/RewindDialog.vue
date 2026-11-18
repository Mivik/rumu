<script setup lang="ts">
// Rewinding to a user message, three ways: the conversation and the files
// together, the conversation only (a fork from just before the message, the
// way an edit forks; the message goes back into the composer), or the files
// only (the conversation stays and is told). The file side previews each
// file the `write`/`edit` calls since then touched, with what reverting does
// to it — restore or delete (checked, can be unchecked), or left alone because
// it changed again since or no full diff was recorded — and the `bash`
// commands of the span, whose effects are not covered. The server plans
// afresh when it reverts, so a file changed meanwhile is still left alone.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { t } from '../../i18n'
import type { Node } from '../../lib/types'
import type { RevertAction } from '../../lib/generated/RevertAction'
import type { RevertFile } from '../../lib/generated/RevertFile'
import type { RevertPlan } from '../../lib/generated/RevertPlan'
import { revertPlan, rewindTo } from '../../lib/store'
import { toast } from '../../lib/state/ui'
import Icon from '../ui/Icon.vue'
import Modal from '../ui/Modal.vue'

const props = defineProps<{ node: Node }>()
const emit = defineEmits<{ close: [] }>()

type Mode = 'both' | 'conversation' | 'files'
const MODES: Mode[] = ['both', 'conversation', 'files']
const mode = ref<Mode>('both')
const withFiles = computed(() => mode.value !== 'conversation')
const withConversation = computed(() => mode.value !== 'files')

const plan = ref<RevertPlan | null>(null)
const loading = ref(true)
const busy = ref(false)
const picked = ref(new Set<string>())
const showCommands = ref(false)

const keyOf = (f: RevertFile): string => `${f.ws}:${f.path}`
const revertible = (a: RevertAction): boolean => a === 'restore' || a === 'delete'

const files = computed(() => plan.value?.files ?? [])
const commands = computed(() => plan.value?.commands ?? [])
const chosen = computed(() =>
  withFiles.value ? files.value.filter((f) => revertible(f.action) && picked.value.has(keyOf(f))) : [],
)
const canSubmit = computed(
  () => !busy.value && !loading.value && (withConversation.value || chosen.value.length > 0),
)

onMounted(async () => {
  plan.value = await revertPlan(props.node.id)
  loading.value = false
  picked.value = new Set(files.value.filter((f) => revertible(f.action)).map(keyOf))
})

function toggle(f: RevertFile): void {
  const next = new Set(picked.value)
  if (!next.delete(keyOf(f))) next.add(keyOf(f))
  picked.value = next
}

function hint(f: RevertFile): string {
  if (f.error) return f.error
  if (f.action === 'changed') return t('chat.revert.changedHint')
  if (f.action === 'unsupported') return t('chat.revert.unsupportedHint')
  return f.label
}

const submitLabel = computed(() =>
  mode.value === 'files' ? t('chat.revert.submit', { count: chosen.value.length }) : t('chat.revert.rewind'),
)

async function submit(): Promise<void> {
  if (!canSubmit.value) return
  busy.value = true
  const done = await rewindTo(
    props.node,
    chosen.value.map((f) => ({ ws: f.ws, path: f.path })),
    withConversation.value,
  )
  busy.value = false
  if (!done) return
  const failed = done.filter((f) => f.error)
  const ok = done.length - failed.length
  if (ok) toast('info', t('chat.revert.done', { count: ok }))
  if (failed.length) {
    toast(
      'error',
      t('chat.revert.failed', { count: failed.length }),
      failed.map((f) => `${f.label}: ${f.error}`).join('\n'),
    )
  }
  emit('close')
}

// Escape closes the dialog, not the panels around it.
function onKey(e: KeyboardEvent): void {
  if (e.key !== 'Escape') return
  e.stopPropagation()
  emit('close')
}
onMounted(() => window.addEventListener('keydown', onKey, true))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true))
</script>

<template>
  <Modal
    :open="true"
    :label="t('chat.revert.title')"
    :z="60"
    wrapper-class="p-3 sm:p-6"
    panel-class="w-full max-w-[560px] max-h-[80vh] rounded-sheet"
    @close="emit('close')"
  >
    <div class="flex min-h-0 flex-col gap-3 p-5">
      <header class="flex items-center gap-2">
        <h3 class="text-title font-bold">{{ t('chat.revert.title') }}</h3>
        <button
          type="button"
          class="icon-btn ml-auto size-7 flex-none"
          :title="t('common.close')"
          :aria-label="t('common.close')"
          @click="emit('close')"
        >
          <Icon name="close" :size="13" />
        </button>
      </header>

      <div class="grid grid-cols-3 gap-1.5" role="radiogroup" :aria-label="t('chat.revert.title')">
        <button
          v-for="m in MODES"
          :key="m"
          type="button"
          role="radio"
          :aria-checked="mode === m"
          class="rounded-control border px-2 py-1.5 text-secondary transition-colors"
          :class="
            mode === m
              ? 'border-brand/60 bg-brand-soft text-ink'
              : 'border-line text-ink-2 hover:bg-surface-2'
          "
          @click="mode = m"
        >
          {{ t(`chat.revert.modes.${m}`) }}
        </button>
      </div>
      <p class="text-meta text-ink-3">{{ t(`chat.revert.hints.${mode}`) }}</p>

      <template v-if="withFiles">
        <p v-if="loading" class="text-meta text-ink-3">{{ t('chat.revert.loading') }}</p>
        <p v-else-if="!files.length" class="text-meta text-ink-3">{{ t('chat.revert.empty') }}</p>
        <ul v-else class="-mx-1 flex min-h-0 flex-col overflow-y-auto">
          <li v-for="f in files" :key="keyOf(f)">
            <label
              class="flex items-center gap-2 rounded-xs px-1 py-1 text-meta"
              :class="revertible(f.action) ? 'cursor-pointer hover:bg-surface-2' : 'text-ink-3'"
              :title="hint(f)"
            >
              <input
                type="checkbox"
                :checked="revertible(f.action) && picked.has(keyOf(f))"
                :disabled="!revertible(f.action)"
                @change="toggle(f)"
              />
              <span
                class="min-w-0 flex-1 truncate font-mono"
                :class="f.action === 'delete' && 'line-through'"
              >
                {{ f.label }}
              </span>
              <span v-if="revertible(f.action)" class="flex flex-none gap-1 tabular-nums">
                <span v-if="f.added" class="font-medium text-pine">+{{ f.added }}</span>
                <span v-if="f.removed" class="font-medium text-danger">−{{ f.removed }}</span>
              </span>
              <span class="w-16 flex-none text-right text-micro text-ink-3">
                {{ t(`chat.revert.${f.action}`) }}
              </span>
            </label>
          </li>
        </ul>

        <div v-if="commands.length" class="flex min-h-0 flex-col gap-1">
          <button
            type="button"
            class="reason-toggle self-start text-ink-3 hover:text-ink-2"
            :aria-expanded="showCommands"
            @click="showCommands = !showCommands"
          >
            <span>{{ t('chat.revert.commands', { count: commands.length }) }}</span>
            <Icon
              name="chevron"
              :size="12"
              class="reason-chevron flex-none"
              :class="{ 'rotate-90': showCommands }"
            />
          </button>
          <ul v-if="showCommands" class="max-h-40 overflow-y-auto rounded-xs bg-surface-2 px-2 py-1">
            <li
              v-for="(c, i) in commands"
              :key="i"
              class="truncate font-mono text-micro text-ink-2"
              :title="c"
            >
              $ {{ c }}
            </li>
          </ul>
        </div>
      </template>

      <footer class="flex items-center justify-end gap-2 pt-1">
        <button type="button" class="btn" @click="emit('close')">{{ t('common.cancel') }}</button>
        <button type="button" class="btn btn-primary" :disabled="!canSubmit" @click="submit">
          <Icon name="undo" :size="13" />
          {{ submitLabel }}
        </button>
      </footer>
    </div>
  </Modal>
</template>
