<script setup lang="ts">
// One assistant group = the consecutive assistant steps of one turn. Plain
// left-aligned text (no bubble) at the column's full width; the cat mascot
// heads the turn on a line of its own. Between pieces of answer text, the
// steps' reasoning and tool calls fold into one ActivityRun summary line, live
// as "thinking" while the model reasons. One muted meta line at the bottom:
// usage stays visible (hover for the full request/turn breakdown);
// copy/regenerate/fork reveal on hover once the turn has finished.
// Only the `live` group (the last one, while a turn runs) shows the stream.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import type { Node } from '../../lib/types'
import type { TimelineGroup } from '../../lib/store'
import { activeDelta, forkFrom, regenerateFrom } from '../../lib/store'
import type { CatState } from '../../lib/cat'
import { renderMarkdown } from '../../lib/markdown'
import { contentText, copyText } from '../../lib/util'
import { toast } from '../../lib/state/ui'
import { vEnter } from '../../lib/motion'
import Icon from '../ui/Icon.vue'
import RumuCat from '../brand/RumuCat.vue'
import ActivityRun from './ActivityRun.vue'
import ContextPills from './ContextPills.vue'
import RunningTools from './RunningTools.vue'
import UsageHud from './UsageHud.vue'

const props = withDefaults(defineProps<{ group: TimelineGroup; live?: boolean; prompt?: Node | null }>(), {
  live: false,
  prompt: null,
})

type AssistantBody = Extract<Node['body'], { type: 'assistant' }>

function bodyOf(n: Node): AssistantBody {
  return n.body as AssistantBody
}

const streamText = computed(() => (props.live ? (activeDelta.value?.text ?? '') : ''))

/** Before the first token there is no body yet: the cat plus "working…". */
const hasBody = computed(() => props.group.items.length > 0 || props.group.assistants.length > 0)

/** Live: waiting for the turn's first token (reasoning or text) vs. tokens
 * already streaming. The cat reads the difference — ears up and a slow look
 * about while waiting, a quicker side-to-side scan while generating. */
const catState = computed<CatState>(() => {
  if (!props.live) return 'idle'
  const d = activeDelta.value
  return d && (d.text || d.reasoning) ? 'working' : 'waiting'
})

function textOf(n: Node): string {
  return contentText(bodyOf(n).content)
}

/** Every piece of answer text in the group, for copying. */
const answerText = computed(() =>
  props.group.items
    .map((item) => {
      if (item.kind !== 'text') return ''
      if (item.node) return textOf(item.node)
      return item.streaming ? streamText.value : ''
    })
    .filter(Boolean)
    .join('\n\n'),
)

const forkTarget = computed(() => props.group.safeNode)
function onFork(): void {
  const n = forkTarget.value
  if (n) void forkFrom(n.id)
}

const regenerating = ref(false)
async function onRegenerate(): Promise<void> {
  const n = props.prompt
  if (!n || props.live || regenerating.value) return
  regenerating.value = true
  try {
    await regenerateFrom(n.id)
  } finally {
    regenerating.value = false
  }
}

async function onCopy(): Promise<void> {
  toast('info', (await copyText(answerText.value)) ? t('chat.assistant.copied') : t('common.copyFailed'))
}
</script>

<template>
  <div class="group/agroup flex min-w-0 flex-col gap-2">
    <!-- the turn's head: the mascot on its own line -->
    <div class="tl-head">
      <RumuCat :state="catState" :size="52" :paper="false" class="-ml-[6px]" />
      <span v-if="!hasBody" v-enter="'enter-fade'" class="text-meta text-ink-3">{{
        t('chat.assistant.working')
      }}</span>
    </div>

    <template v-for="(item, i) in group.items" :key="i">
      <ContextPills v-if="item.kind === 'context'" v-enter="'enter-fade'" :nodes="item.nodes" />
      <ActivityRun v-else-if="item.kind === 'activity'" v-enter="'enter-fade'" :steps="item.steps" />
      <div
        v-else-if="item.kind === 'reset'"
        :data-path-node="item.node.id"
        class="flex items-center gap-1.5 text-meta text-ink-3"
      >
        <Icon name="brain" :size="12" />
        <span>{{ t('chat.assistant.reasoningReset') }}</span>
      </div>
      <div
        v-else-if="item.streaming"
        v-enter="'enter-fade'"
        class="md streaming-caret"
        v-html="renderMarkdown(streamText)"
      />
      <div
        v-else-if="item.node"
        :data-path-node="item.node.id"
        class="md"
        v-html="renderMarkdown(textOf(item.node))"
      />
    </template>

    <!-- live output of the calls still running -->
    <RunningTools v-if="live" v-enter="'enter-fade'" :group="group" />

    <!-- meta line: usage stays visible; actions reveal on hover once finished -->
    <div v-if="hasBody" class="-mt-0.5 flex min-h-6 items-center gap-2">
      <!-- usage only for a finished turn; the live one is still changing -->
      <UsageHud v-if="!live" :group="group" />
      <span class="flex-1" />
      <span
        v-if="!live"
        class="flex items-center gap-0.5 opacity-100 transition duration-200 ease-soft sm:opacity-0 sm:group-hover/agroup:opacity-100 sm:group-focus-within/agroup:opacity-100"
      >
        <button
          class="icon-btn size-5"
          :title="t('chat.assistant.copy')"
          :aria-label="t('chat.assistant.copy')"
          :disabled="!answerText"
          @click="onCopy"
        >
          <Icon name="copy" :size="12" />
        </button>
        <button
          class="icon-btn size-5"
          :title="prompt ? t('chat.assistant.regenerate') : t('chat.assistant.noPrompt')"
          :aria-label="t('chat.assistant.regenerate')"
          :disabled="!prompt || regenerating"
          @click="onRegenerate"
        >
          <Icon name="regenerate" :size="12" />
        </button>
        <button
          class="icon-btn size-5"
          :title="forkTarget ? t('chat.assistant.fork') : t('chat.assistant.noSafePoint')"
          :aria-label="t('chat.assistant.fork')"
          :disabled="!forkTarget"
          @click="onFork"
        >
          <Icon name="fork" :size="12" />
        </button>
      </span>
    </div>
  </div>
</template>
