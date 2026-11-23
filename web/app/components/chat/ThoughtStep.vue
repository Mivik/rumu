<script setup lang="ts">
// A step's reasoning inside an opened activity run: a quiet disclosure
// ("Thought for 4s") and, opened, the text under a hairline. The live one
// (`node` null) reads "Thinking" and shows its stream while the model
// thinks, folding once the answer starts unless toggled by hand (or the
// appearance settings keep every thought open, or folded). `flat` shows
// just the text, for a run (ActivityRun) whose own line is the disclosure.
// The live stream's box follows its end as the reasoning grows.
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { t } from '../../i18n'
import { type Node, reasoningText } from '../../lib/types'
import { useBranchView } from '../../lib/store'
import { fmtThought } from '../../lib/util'
import { growFollowing } from '../../lib/motion'
import { useWorkView } from '../../lib/state/appearance'
import Icon from '../ui/Icon.vue'

const props = withDefaults(defineProps<{ node: Node | null; flat?: boolean }>(), { flat: false })
const activeDelta = useBranchView().delta
const work = useWorkView()

const reasoning = computed(() => {
  if (!props.node) return activeDelta.value?.reasoning ?? ''
  return props.node.body.type === 'assistant' ? reasoningText(props.node.body.reasoning) : ''
})

/** The live stream's box: its text grows in place, appended to one text
 * node at most every 50 ms. Setting the whole thought on each delta laid it
 * out again from the start, a cost growing with its length. */
const liveEl = ref<HTMLElement | null>(null)
let liveText: Text | null = null
let liveTimer: ReturnType<typeof setTimeout> | null = null

function drawLive(): void {
  liveTimer = null
  const el = liveEl.value
  if (!el) return
  const text = reasoning.value
  growFollowing(el, () => {
    if (liveText?.parentNode === el && text.startsWith(liveText.data)) {
      liveText.appendData(text.slice(liveText.data.length))
    } else {
      liveText = document.createTextNode(text)
      el.replaceChildren(liveText)
    }
  })
}

watch(reasoning, () => {
  if (!props.node && liveTimer === null) liveTimer = setTimeout(drawLive, 50)
})
watch(liveEl, drawLive, { flush: 'post' })
onBeforeUnmount(() => {
  if (liveTimer !== null) clearTimeout(liveTimer)
})

/** The thinking span: the kernel's timing, or the live one once the answer starts. */
const ms = computed(() => {
  if (!props.node) return activeDelta.value?.reasonMs
  return props.node.body.type === 'assistant' ? props.node.body.reason_ms : undefined
})

const thinking = computed(() => !props.node && ms.value === undefined)

/** Tri-state: null = as the appearance settings say — `auto` opens the
 *  live one while the model thinks. */
const manual = ref<boolean | null>(null)
const open = computed(() => {
  if (manual.value !== null) return manual.value
  const mode = work.value.thinking
  return mode === 'open' || (mode === 'auto' && !props.node && !activeDelta.value?.text)
})
function toggle(): void {
  manual.value = !open.value
}

const label = computed(() => {
  if (thinking.value) return t('tools.thought.thinking')
  return ms.value != null
    ? t('tools.thought.thoughtFor', { duration: fmtThought(ms.value) })
    : t('tools.thought.thought')
})
</script>

<template>
  <!-- flat: inside a run whose line already names the thinking -->
  <div
    v-if="flat && !node"
    ref="liveEl"
    class="max-h-[300px] overflow-y-auto text-meta leading-[1.625] whitespace-pre-wrap text-ink-2"
  />
  <div
    v-else-if="flat"
    class="max-h-[300px] overflow-y-auto text-meta leading-[1.625] whitespace-pre-wrap text-ink-2"
    v-text="reasoning"
  />
  <div v-else class="reason">
    <button
      type="button"
      class="reason-toggle"
      :class="thinking ? 'text-brand-strong' : open ? 'text-ink-2' : 'text-ink-3 hover:text-ink-2'"
      :aria-expanded="open"
      @click="toggle"
    >
      <span v-if="thinking" class="dot-live size-1.5 rounded-full bg-brand" />
      <Icon v-else name="brain" :size="12" />
      {{ label }}
      <Icon name="chevron" :size="12" class="reason-chevron" :class="{ 'rotate-90': open }" />
    </button>
    <template v-if="open">
      <div v-if="!node" ref="liveEl" class="reason-body reveal" />
      <div v-else class="reason-body reveal">{{ reasoning }}</div>
    </template>
  </div>
</template>
