<script setup lang="ts">
// The one pill: rounded, tinted, optional leading/trailing icon. Used for
// context pills ("imported …", "workspace …") and the sub-agent affordance. The default slot allows rich content (e.g. clickable
// card names inside an "imported A, B" pill).
// The tint recipe lives in main.css (.chip + .tone-*).
import Icon from './Icon.vue'

export type ChipTone = 'neutral' | 'brand' | 'memory' | 'tool' | 'danger' | 'agent'

withDefaults(
  defineProps<{
    icon?: string
    iconAfter?: string
    tone?: ChipTone
    label?: string
    title?: string
    mono?: boolean
    clickable?: boolean
  }>(),
  { tone: 'neutral', label: '', clickable: false },
)

defineEmits<{ click: [e: MouseEvent] }>()
</script>

<template>
  <component
    :is="clickable ? 'button' : 'span'"
    class="chip"
    :class="[`tone-${tone}`, { 'font-mono text-micro': mono }]"
    :title="title"
    :type="clickable ? 'button' : undefined"
    @click="clickable && $emit('click', $event)"
  >
    <Icon v-if="icon" :name="icon" :size="12" />
    <span class="min-w-0 overflow-hidden text-ellipsis">
      <slot>{{ label }}</slot>
    </span>
    <Icon v-if="iconAfter" :name="iconAfter" :size="11" />
  </component>
</template>
