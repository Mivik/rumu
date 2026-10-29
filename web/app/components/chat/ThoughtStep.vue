<script setup lang="ts">
// A step's reasoning inside an opened activity run: a quiet disclosure
// ("Thought for 4s") and, opened, the text under a hairline. The live one
// (`node` null) reads "Thinking" and shows its stream while the model
// thinks, folding once the answer starts unless toggled by hand. `flat` shows
// just the text, for a run (ActivityRun) whose own line is the disclosure.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { type Node, reasoningText } from '../../lib/types'
import { activeDelta } from '../../lib/store'
import { fmtThought } from '../../lib/util'
import Icon from '../ui/Icon.vue'

const props = withDefaults(defineProps<{ node: Node | null; flat?: boolean }>(), { flat: false })

const reasoning = computed(() => {
  if (!props.node) return activeDelta.value?.reasoning ?? ''
  return props.node.body.type === 'assistant' ? reasoningText(props.node.body.reasoning) : ''
})

/** The thinking span: the kernel's timing, or the live one once the answer starts. */
const ms = computed(() => {
  if (!props.node) return activeDelta.value?.reasonMs
  return props.node.body.type === 'assistant' ? props.node.body.reason_ms : undefined
})

const thinking = computed(() => !props.node && ms.value === undefined)

/** Tri-state: null = auto (the live one open while the model thinks). */
const manual = ref<boolean | null>(null)
const open = computed(() => (manual.value !== null ? manual.value : !props.node && !activeDelta.value?.text))
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
    v-if="flat"
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
      <Icon name="chevron" :size="10" class="reason-chevron" :class="{ 'rotate-90': open }" />
    </button>
    <div v-if="open" class="reason-body reveal">{{ reasoning }}</div>
  </div>
</template>
