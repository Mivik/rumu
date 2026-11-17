<script setup lang="ts">
// The preview of reverting the file changes since a user message: each file
// the `write`/`edit` calls since then touched, with what reverting does to
// it — restore or delete (checked, can be unchecked), or left alone because
// it changed again since or no full diff was recorded — and the `bash`
// commands of the span, whose effects are not covered. Reverting re-plans on
// the server, so a file changed meanwhile is still left alone.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { t } from '../../i18n'
import type { NodeId } from '../../lib/types'
import type { RevertAction } from '../../lib/generated/RevertAction'
import type { RevertFile } from '../../lib/generated/RevertFile'
import type { RevertPlan } from '../../lib/generated/RevertPlan'
import { revertFiles, revertPlan } from '../../lib/store'
import { toast } from '../../lib/state/ui'
import Icon from '../ui/Icon.vue'
import Modal from '../ui/Modal.vue'

const props = defineProps<{ node: NodeId }>()
const emit = defineEmits<{ close: [] }>()

const plan = ref<RevertPlan | null>(null)
const loading = ref(true)
const busy = ref(false)
const picked = ref(new Set<string>())
const showCommands = ref(false)

const keyOf = (f: RevertFile): string => `${f.ws}:${f.path}`
const revertible = (a: RevertAction): boolean => a === 'restore' || a === 'delete'

const files = computed(() => plan.value?.files ?? [])
const commands = computed(() => plan.value?.commands ?? [])
const chosen = computed(() => files.value.filter((f) => revertible(f.action) && picked.value.has(keyOf(f))))

onMounted(async () => {
  plan.value = await revertPlan(props.node)
  loading.value = false
  picked.value = new Set(files.value.filter((f) => revertible(f.action)).map(keyOf))
})

function toggle(f: RevertFile): void {
  const next = new Set(picked.value)
  if (!next.delete(keyOf(f))) next.add(keyOf(f))
  picked.value = next
}

const WORD: Record<RevertAction, 'restore' | 'delete' | 'changed' | 'unsupported'> = {
  restore: 'restore',
  delete: 'delete',
  changed: 'changed',
  unsupported: 'unsupported',
}

function hint(f: RevertFile): string {
  if (f.error) return f.error
  if (f.action === 'changed') return t('chat.revert.changedHint')
  if (f.action === 'unsupported') return t('chat.revert.unsupportedHint')
  return f.label
}

async function submit(): Promise<void> {
  if (busy.value || !chosen.value.length) return
  busy.value = true
  const done = await revertFiles(
    props.node,
    chosen.value.map((f) => ({ ws: f.ws, path: f.path })),
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
      <header class="flex items-start gap-2">
        <div class="flex min-w-0 flex-col gap-0.5">
          <h3 class="text-title font-bold">{{ t('chat.revert.title') }}</h3>
          <p class="text-meta text-ink-3">{{ t('chat.revert.hint') }}</p>
        </div>
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
            <span class="min-w-0 flex-1 truncate font-mono" :class="f.action === 'delete' && 'line-through'">
              {{ f.label }}
            </span>
            <span v-if="revertible(f.action)" class="flex flex-none gap-1 tabular-nums">
              <span v-if="f.added" class="font-medium text-pine">+{{ f.added }}</span>
              <span v-if="f.removed" class="font-medium text-danger">−{{ f.removed }}</span>
            </span>
            <span class="w-16 flex-none text-right text-micro text-ink-3">
              {{ t(`chat.revert.${WORD[f.action]}`) }}
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
          <li v-for="(c, i) in commands" :key="i" class="truncate font-mono text-micro text-ink-2" :title="c">
            $ {{ c }}
          </li>
        </ul>
      </div>

      <footer class="flex items-center justify-end gap-2 pt-1">
        <button type="button" class="btn" @click="emit('close')">{{ t('common.cancel') }}</button>
        <button type="button" class="btn btn-primary" :disabled="busy || !chosen.length" @click="submit">
          <Icon name="undo" :size="13" />
          {{ t('chat.revert.submit', { count: chosen.length }) }}
        </button>
      </footer>
    </div>
  </Modal>
</template>
