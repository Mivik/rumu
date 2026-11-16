<script setup lang="ts">
// A user message: right-aligned coral-tinted bubble (max ~70% of the column,
// width fits content, no avatar); its coral ring on the thread is ConversationView's.
// Edit/copy/regenerate/fork sit under the message's bottom-right: always shown
// on touch, revealed on hover with a fine pointer. Editing loads the message
// into the composer (`editTarget`), outlined here while it is there;
// regenerate asks the same question again
// (same op as the assistant side's regenerate — the reply is replaced); fork
// copies the conversation up to here into a new session.
//
// Images lead the message as thumbnails, the way the web composer sends them
// (`withLeadingAnchors`); the text below is what was typed. An anchor placed
// elsewhere by another client stays visible as text.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import type { Node } from '../../lib/types'
import { forkSession, isBusy, regenerateFrom, useBranchView } from '../../lib/store'
import { renderMarkdown } from '../../lib/markdown'
import { renderedContent, typedText } from '../../lib/attachments'
import { copyText } from '../../lib/util'
import { editTarget, toast } from '../../lib/state/ui'
import Icon from '../ui/Icon.vue'
import BranchSwitcher from './BranchSwitcher.vue'
import ContextPills from './ContextPills.vue'
import ImageThumb from './ImageThumb.vue'

const props = withDefaults(defineProps<{ node: Node; context?: Node[] }>(), { context: () => [] })
const view = useBranchView()

const content = computed(() => (props.node.body as Extract<Node['body'], { type: 'user' }>).content)
const images = computed(() => content.value.attachments ?? [])
const text = computed(() => typedText(content.value.text, images.value.length))
const editing = computed(() => editTarget.value?.id === props.node.id)

async function onCopy(): Promise<void> {
  // The clipboard gets the text-only form: images as their placeholders.
  toast(
    'info',
    (await copyText(renderedContent(content.value))) ? t('chat.user.copied') : t('common.copyFailed'),
  )
}

/** Re-asking the prompt is the assistant side's regenerate: it replaces the
 *  reply that followed (guarded while the request is in flight). */
const regenerating = ref(false)
async function onRegenerate(): Promise<void> {
  if (isBusy.value || regenerating.value) return
  regenerating.value = true
  try {
    await regenerateFrom(props.node.id)
  } finally {
    regenerating.value = false
  }
}

function onFork(): void {
  void forkSession(props.node.id)
}
</script>

<template>
  <div class="group/urow relative flex min-w-0 flex-col gap-1.5">
    <div :data-path-node="node.id" class="flex min-w-0 justify-end">
      <div
        class="flex max-w-[85%] min-w-0 flex-col items-end gap-1.5 rounded-sheet outline-offset-4 transition-[outline-color] sm:max-w-[70%]"
        :class="editing ? 'outline-2 outline-brand/50' : 'outline-transparent'"
      >
        <div v-if="images.length" class="flex flex-wrap justify-end gap-1.5">
          <ImageThumb
            v-for="(a, i) in images"
            :key="`${a.hash}:${i}`"
            :attachment="a"
            :max-h="120"
            :max-w="200"
          />
        </div>
        <div
          v-if="text"
          class="md w-fit max-w-full rounded-sheet rounded-br-xs bg-brand-soft px-4 py-2 text-ink"
          :title="new Date(node.at).toLocaleString()"
          v-html="renderMarkdown(text)"
        />
      </div>
    </div>

    <!-- actions sit at the message's bottom-right: always on touch, revealed on hover with a fine pointer -->
    <div
      v-if="!view.sub"
      class="mt-0.5 flex h-5 items-center justify-end gap-0.5 opacity-100 transition duration-200 ease-soft pointer-fine:opacity-0 pointer-fine:group-hover/urow:opacity-100 pointer-fine:group-focus-within/urow:opacity-100"
    >
      <BranchSwitcher :parent-id="node.parent" />
      <button
        class="icon-btn size-5"
        :title="t('chat.user.editTitle')"
        :aria-label="t('chat.user.editTitle')"
        :aria-pressed="editing"
        :disabled="isBusy && !editing"
        @click="editTarget = editing ? null : node"
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
        :title="t('chat.user.regenerate')"
        :aria-label="t('chat.user.regenerate')"
        :disabled="isBusy || regenerating"
        @click="onRegenerate"
      >
        <Icon name="regenerate" :size="12" />
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
