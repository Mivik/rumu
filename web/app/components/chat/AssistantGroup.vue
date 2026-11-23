<script setup lang="ts">
// One assistant group = the consecutive assistant steps of one turn. Plain
// left-aligned text (no bubble) at the column's full width; the cat mascot
// heads the turn on a line of its own. Between pieces of answer text, the
// steps' reasoning and tool calls form one ActivityRun: open while it is the
// turn's frontier, folded to a summary line once the model moves on, its
// edits standing outside the fold. While the turn runs, its frontier
// always carries the live dot: the trailing activity run (its running calls
// included), the streaming text's caret, or — while it waits on the model —
// a "working…" line of its own.
// One muted meta line at the bottom:
// usage and the files the turn changed stay visible (hover usage for the
// full request/turn breakdown);
// copy/regenerate/fork and the more menu reveal on hover once the turn has finished.
// Only the `live` group (the last one, while a turn runs) shows the stream.
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { t } from '../../i18n'
import type { Node } from '../../lib/types'
import type { TimelineGroup } from '../../lib/store'
import { forkSession, regenerateFrom, state, useBranchView } from '../../lib/store'
import { catOf } from '../../lib/assistantFace'
import type { CatState } from '../../lib/cat'
import { fileEditsOf } from '../../lib/toolrows'
import { contentText, copyText } from '../../lib/util'
import { toast } from '../../lib/state/ui'
import { vEnter } from '../../lib/motion'
import Icon from '../ui/Icon.vue'
import RumuCat from '../brand/RumuCat.vue'
import ActivityRun from './ActivityRun.vue'
import ArtifactCard from './ArtifactCard.vue'
import McpAppView from './McpAppView.vue'
import BranchSwitcher from './BranchSwitcher.vue'
import ContextPills from './ContextPills.vue'
import MessageMenu from './MessageMenu.vue'
import ReplyText from './ReplyText.vue'
import StreamText from './StreamText.vue'
import UsageHud from './UsageHud.vue'

const props = withDefaults(defineProps<{ group: TimelineGroup; live?: boolean; prompt?: Node | null }>(), {
  live: false,
  prompt: null,
})

type AssistantBody = Extract<Node['body'], { type: 'assistant' }>

const view = useBranchView()
const activeDelta = view.delta

function bodyOf(n: Node): AssistantBody {
  return n.body as AssistantBody
}

/** Before the first token there is no body yet: the cat plus "working…". */
const hasBody = computed(() => props.group.items.length > 0 || props.group.assistants.length > 0)

/** The session's assistant's own cat. */
const cat = computed(() => (state.session ? catOf(state.assistants, state.session.assistant) : undefined))

/** Live: waiting on the model (nothing streamed yet), thinking (reasoning
 * streams) or working (answer text streams). */
const catState = computed<CatState>(() => {
  if (!props.live) return 'idle'
  const d = activeDelta.value
  return d?.text ? 'working' : d?.reasoning ? 'thinking' : 'waiting'
})

/** The live turn has calls still waiting on their result (the trailing
 * run's running step). */
const running = computed(() => {
  const tail = props.group.items.at(-1)
  return props.live && tail?.kind === 'activity' && tail.steps.at(-1)?.kind === 'running'
})

/** The files the turn changed and the lines it added / removed, for the
 * meta line (a file edited twice counts once). */
const changed = computed(() => {
  const files = new Set<string>()
  let added = 0
  let removed = 0
  for (const item of props.group.items) {
    if (item.kind !== 'activity') continue
    for (const s of item.steps) {
      if (s.kind !== 'tools') continue
      for (const v of s.rows.flatMap(fileEditsOf)) {
        // cards are memory, not the workspace files the turn changed
        if (v.toolCardId) continue
        // the same file however the calls wrote its path
        const file = v.patch?.file
        files.add(file ? `${file.ws}:${file.path}` : v.callFull)
        added += v.diff?.added ?? 0
        removed += v.diff?.removed ?? 0
      }
    }
  }
  return files.size ? { count: files.size, added, removed } : null
})

function textOf(n: Node): string {
  return contentText(bodyOf(n).content)
}

/** Every piece of answer text in the group, for copying. */
const answerText = computed(() =>
  props.group.items
    .map((item) => {
      if (item.kind !== 'text') return ''
      return item.node ? textOf(item.node) : ''
    })
    .filter(Boolean)
    .join('\n\n'),
)

const forkTarget = computed(() => props.group.safeNode)
function onFork(): void {
  const n = forkTarget.value
  if (n) void forkSession(n.id)
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

/** A transient model failure being retried on this branch (`turn.retrying`),
 * shown where the thinking indicator is: "retrying 2/5 · 3s", counting down
 * to the replay. Null unless this is the live group. */
const retrying = computed(() =>
  props.live && view.branch.value ? (state.retrying[view.branch.value] ?? null) : null,
)
/** This live group's branch is compacting its context (`compact.started`). */
const compacting = computed(() =>
  props.live && view.branch.value ? Boolean(state.compacting[view.branch.value]) : false,
)
const retryLeft = ref(0)
let retryTimer: ReturnType<typeof setInterval> | undefined
watch(
  retrying,
  (r) => {
    if (retryTimer) {
      clearInterval(retryTimer)
      retryTimer = undefined
    }
    if (!r) {
      retryLeft.value = 0
      return
    }
    const tick = (): void => {
      retryLeft.value = Math.max(0, Math.ceil((r.until - Date.now()) / 1000))
    }
    tick()
    retryTimer = setInterval(tick, 250)
  },
  { immediate: true },
)
onBeforeUnmount(() => retryTimer && clearInterval(retryTimer))

/** The live turn waits on the model: no call runs and the step has streamed
 * nothing yet — so the dot needs a line of its own ("working…"): beside the
 * mascot before anything shows, else at the end. */
const idle = computed(() => {
  if (!props.live || retrying.value || compacting.value || running.value) return false
  const d = activeDelta.value
  return !d?.reasoning && !d?.text
})

/** The items to show: while it waits on the model, a trailing run of only
 * recall and catalog has nothing to say on its line yet — it shows once the
 * step's reasoning joins it. */
const items = computed(() => {
  const tail = props.group.items.at(-1)
  const silent =
    tail?.kind === 'activity' && tail.steps.every((s) => s.kind === 'catalog' || s.kind === 'recall')
  return idle.value && silent ? props.group.items.slice(0, -1) : props.group.items
})

/** Nothing shows below the mascot yet: "working…" goes beside it. */
const bare = computed(() => !items.value.length && !props.group.assistants.length)
</script>

<template>
  <div class="group/agroup flex min-w-0 flex-col gap-2">
    <!-- the turn's head: the mascot on its own line -->
    <div class="tl-head">
      <RumuCat :state="catState" :cat="cat" :size="52" :paper="false" class="-ml-[6px]" />
      <span
        v-if="retrying"
        v-enter="'enter-fade'"
        class="text-meta text-ink-3 tabular-nums"
        :title="retrying.message"
      >
        {{
          t('chat.assistant.retrying', {
            attempt: retrying.attempt,
            max: retrying.max,
            seconds: retryLeft,
          })
        }}
      </span>
      <span v-else-if="compacting" v-enter="'enter-fade'" class="working-line">
        <span class="dot-live size-1.5 rounded-full bg-brand" />{{ t('chat.assistant.compacting') }}
      </span>
      <span v-else-if="idle && bare" v-enter="'enter-fade'" class="working-line">
        <span class="dot-live size-1.5 rounded-full bg-brand" />{{ t('chat.assistant.working') }}
      </span>
    </div>

    <!-- keyed from the window start a turn opened at, if any: the rest of
         the turn loads above it -->
    <template v-for="(item, i) in items" :key="i - (group.seamAt ?? 0)">
      <ContextPills v-if="item.kind === 'context'" v-enter="'enter-fade'" :nodes="item.nodes" />
      <ActivityRun
        v-else-if="item.kind === 'activity'"
        v-enter="'enter-fade'"
        :steps="item.steps"
        :live="live && !idle && i === items.length - 1"
        :current="live && i === items.length - 1"
      />
      <ArtifactCard v-else-if="item.kind === 'artifact'" v-enter="'enter-fade'" :node="item.node" />
      <McpAppView v-else-if="item.kind === 'app'" v-enter="'enter-fade'" :node="item.node" />
      <div
        v-else-if="item.kind === 'reset'"
        :data-path-node="item.node.id"
        class="flex items-center gap-1.5 text-meta text-ink-3"
      >
        <Icon name="brain" :size="12" />
        <span>{{ t('chat.assistant.reasoningReset') }}</span>
      </div>
      <StreamText v-else-if="item.streaming" v-enter="'enter-fade'" />
      <ReplyText v-else-if="item.node" :node="item.node" />
    </template>

    <span v-if="idle && !bare" v-enter="'enter-fade'" class="working-line">
      <span class="dot-live size-1.5 rounded-full bg-brand" />{{ t('chat.assistant.working') }}
    </span>

    <!-- meta line: usage stays visible; actions reveal on hover once finished -->
    <div v-if="hasBody" class="-mt-0.5 flex min-h-6 items-center gap-2">
      <!-- usage only for a finished turn; the live one is still changing -->
      <UsageHud v-if="!live" :group="group" />
      <span
        v-if="!live && changed"
        class="flex min-w-0 gap-1 text-micro whitespace-nowrap text-ink-3 tabular-nums"
      >
        <span class="truncate">{{ t('chat.assistant.changed', { count: changed.count }) }}</span>
        <span class="flex-none font-medium text-pine">+{{ changed.added }}</span>
        <span v-if="changed.removed" class="flex-none font-medium text-danger">−{{ changed.removed }}</span>
      </span>
      <span class="flex-1" />
      <span
        v-if="!live && !view.sub"
        class="flex items-center gap-0.5 opacity-100 transition duration-200 ease-soft sm:opacity-0 sm:group-hover/agroup:opacity-100 sm:group-focus-within/agroup:opacity-100 sm:has-aria-expanded:opacity-100"
      >
        <BranchSwitcher :parent-id="prompt?.id ?? null" />
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
        <MessageMenu
          v-if="group.assistants.length"
          :from="group.assistants[0]!.id"
          :to="group.assistants.at(-1)!.id"
        />
      </span>
    </div>
  </div>
</template>
