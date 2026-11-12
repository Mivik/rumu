<script setup lang="ts">
// One JSON value as a foldable tree. Objects and arrays open a level at a
// time; strings keep their line breaks, so a node's text reads as text rather
// than one escaped line, and a very long one shows its head until asked.
// `bare` lists a root object's fields with no fold of its own (a call's args).
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import Icon from '../ui/Icon.vue'

const props = withDefaults(
  defineProps<{ value: unknown; name?: string | number; depth?: number; bare?: boolean }>(),
  { name: undefined, depth: 0, bare: false },
)

/** Levels open on first show: a node's body, content and payload. */
const OPEN_DEPTH = 4
/** Characters of a string shown before "show all". */
const LONG = 1200

const open = ref(props.depth < OPEN_DEPTH)
const full = ref(false)

const isArray = computed(() => Array.isArray(props.value))
const entries = computed<[string | number, unknown][]>(() => {
  const v = props.value
  if (Array.isArray(v)) return v.map((x, i) => [i, x])
  if (v !== null && typeof v === 'object') return Object.entries(v)
  return []
})
/** A folded object or array: its bracket and size, `{3}` / `[12]`. */
const shape = computed(() => {
  const n = entries.value.length
  return isArray.value ? `[${n}]` : `{${n}}`
})
const isBranch = computed(() => props.value !== null && typeof props.value === 'object')

const str = computed(() => (typeof props.value === 'string' ? props.value : ''))
const clipped = computed(() => !full.value && str.value.length > LONG)
const shown = computed(() => (clipped.value ? `${str.value.slice(0, LONG)}…` : str.value))

/** Literal utility strings (Tailwind only emits complete class names). */
const tone = computed(() => {
  const v = props.value
  if (typeof v === 'string') return 'text-pine'
  if (typeof v === 'number') return 'text-tool'
  return 'text-memory'
})
</script>

<template>
  <!-- the root sets the type; nested levels inherit it -->
  <div :class="depth ? '' : bare ? 'font-mono text-meta' : 'font-mono text-micro'">
    <template v-if="isBranch">
      <button
        v-if="!bare"
        type="button"
        class="flex items-baseline gap-1 rounded-xs text-left hover:bg-surface-2 disabled:hover:bg-transparent"
        :disabled="!entries.length"
        :aria-expanded="entries.length ? open : undefined"
        @click="open = !open"
      >
        <Icon
          name="chevron"
          :size="11"
          class="text-ink-3 transition-transform"
          :class="{ 'rotate-90': open && entries.length, invisible: !entries.length }"
        />
        <span v-if="name !== undefined" class="text-ink-2">{{ name }}:</span>
        <span class="text-ink-3">{{ shape }}</span>
      </button>
      <div v-if="bare">
        <JsonTree v-for="[k, v] in entries" :key="k" :value="v" :name="k" :depth="depth + 1" />
      </div>
      <div v-else-if="open && entries.length" class="ml-[5px] border-l border-line-soft pl-2.5">
        <JsonTree v-for="[k, v] in entries" :key="k" :value="v" :name="k" :depth="depth + 1" />
      </div>
    </template>
    <div v-else class="pl-[15px]">
      <span v-if="name !== undefined" class="text-ink-2">{{ name }}: </span>
      <span v-if="typeof value === 'string' && !value.includes('\n')" :class="tone" class="break-words"
        >"{{ shown }}"</span
      >
      <span v-else-if="typeof value !== 'string'" :class="tone">{{
        value === undefined ? 'undefined' : JSON.stringify(value)
      }}</span>
      <div
        v-if="typeof value === 'string' && value.includes('\n')"
        class="my-0.5 rounded-xs bg-surface px-2 py-1 break-words whitespace-pre-wrap text-ink"
        v-text="shown"
      />
      <button
        v-if="clipped"
        type="button"
        class="ml-1 text-ink-3 underline hover:text-ink"
        @click="full = true"
      >
        {{ t('chat.trace.more', { n: str.length }) }}
      </button>
    </div>
  </div>
</template>
