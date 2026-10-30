<script setup lang="ts">
// A user message: right-aligned coral-tinted bubble (max ~70% of the column,
// width fits content, no avatar); its coral ring on the thread is ChatView's.
// Time only on hover; edit/copy/fork appear left of the bubble on hover.
// Editing swaps the bubble for a textarea and resends the prompt on a branch
// forked from just before it, so the original turn stays where it was.
import { computed, nextTick, ref } from 'vue'
import { t } from '../../i18n'
import type { Node } from '../../lib/types'
import { editPrompt, forkAt, isBusy } from '../../lib/store'
import { renderMarkdown } from '../../lib/markdown'
import { contentText, copyText } from '../../lib/util'
import { toast } from '../../lib/state/ui'
import Icon from '../ui/Icon.vue'
import BranchSwitcher from './BranchSwitcher.vue'
import ContextPills from './ContextPills.vue'

const props = withDefaults(defineProps<{ node: Node; context?: Node[] }>(), { context: () => [] })

const text = computed(() => contentText((props.node.body as Extract<Node['body'], { type: 'user' }>).content))

const editing = ref(false)
const saving = ref(false)
const draft = ref('')
const editor = ref<HTMLTextAreaElement | null>(null)
const bubble = ref<HTMLElement | null>(null)
/** The message's text height when editing began: the editor never opens
 *  shorter than the message it replaces. */
const editMinH = ref(0)

async function startEdit(): Promise<void> {
  if (isBusy.value) return
  const b = bubble.value
  if (b) {
    const cs = getComputedStyle(b)
    editMinH.value = b.clientHeight - parseFloat(cs.paddingTop) - parseFloat(cs.paddingBottom)
  }
  draft.value = text.value
  editing.value = true
  await nextTick()
  const el = editor.value
  if (!el) return
  el.focus()
  el.setSelectionRange(el.value.length, el.value.length)
}

function cancelEdit(): void {
  if (saving.value) return
  editing.value = false
  draft.value = ''
}

async function saveEdit(): Promise<void> {
  const next = draft.value.trim()
  if (!next || saving.value) return
  if (next === text.value.trim()) {
    editing.value = false
    return
  }
  saving.value = true
  try {
    if (await editPrompt(props.node, next)) editing.value = false
  } finally {
    saving.value = false
  }
}

function onEditorKeydown(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    e.preventDefault()
    cancelEdit()
  } else if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    void saveEdit()
  }
}

async function onCopy(): Promise<void> {
  toast('info', (await copyText(text.value)) ? t('chat.user.copied') : t('common.copyFailed'))
}

function onFork(): void {
  void forkAt(props.node.id)
}
</script>

<template>
  <div class="group/urow relative flex min-w-0 flex-col gap-1.5">
    <div :data-path-node="node.id" class="flex min-w-0 justify-end">
      <div
        v-if="editing"
        class="w-[85%] min-w-0 rounded-sheet rounded-br-xs bg-brand-soft px-4 py-2 text-ink shadow-soft sm:w-[70%]"
      >
        <textarea
          ref="editor"
          v-model="draft"
          rows="2"
          :style="{ minHeight: `${editMinH}px` }"
          :aria-label="t('chat.user.edit')"
          class="max-h-[50vh] min-h-[52px] w-full resize-none [field-sizing:content] rounded-none border-0 bg-transparent p-0 text-message leading-normal text-ink focus:shadow-none"
          @keydown="onEditorKeydown"
        />
        <div class="mt-1 flex items-center justify-end gap-3">
          <button
            class="text-micro font-semibold text-ink-2 transition-colors hover:text-ink disabled:opacity-50"
            :disabled="saving"
            @click="cancelEdit"
          >
            {{ t('chat.user.cancel') }}
          </button>
          <button
            class="text-micro font-semibold text-brand-strong transition-opacity hover:underline disabled:opacity-50"
            :disabled="saving || !draft.trim()"
            @click="saveEdit"
          >
            {{ t('chat.user.resend') }}
          </button>
        </div>
      </div>
      <div
        v-else
        ref="bubble"
        class="md w-fit max-w-[85%] rounded-sheet rounded-br-xs bg-brand-soft px-4 py-2 text-ink sm:max-w-[70%]"
        :title="new Date(node.at).toLocaleString()"
        v-html="renderMarkdown(text)"
      />
    </div>

    <!-- actions sit at the message's bottom-right, revealed on hover -->
    <div
      v-if="!editing"
      class="mt-0.5 hidden h-5 items-center justify-end gap-0.5 opacity-0 transition duration-200 ease-soft group-hover/urow:opacity-100 group-focus-within/urow:opacity-100 sm:flex"
    >
      <BranchSwitcher :parent-id="node.parent" />
      <button
        class="icon-btn size-5"
        :title="t('chat.user.editTitle')"
        :aria-label="t('chat.user.editTitle')"
        :disabled="isBusy"
        @click="startEdit"
      >
        <Icon name="edit" :size="12" />
      </button>
      <button
        class="icon-btn size-5"
        :title="t('chat.user.copyTitle')"
        :aria-label="t('chat.user.copyTitle')"
        @click="onCopy"
      >
        <Icon name="copy" :size="12" />
      </button>
      <button
        class="icon-btn size-5"
        :title="t('chat.user.forkTitle')"
        :aria-label="t('chat.user.forkTitle')"
        :disabled="isBusy"
        @click="onFork"
      >
        <Icon name="fork" :size="12" />
      </button>
    </div>

    <ContextPills v-if="props.context.length" :nodes="props.context" />
  </div>
</template>
