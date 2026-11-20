<script setup lang="ts">
// A text document as a chip: its name and its size, the way a file reads in
// a list. The chip in the composer's tray carries the upload's state around
// it; the one in a message opens the stored blob.
import { computed } from 'vue'
import { t } from '../../i18n'
import { fmtBytes } from '../../lib/util'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ name: string; size?: number; href?: string }>()

const bytes = computed(() => fmtBytes(props.size ?? 0))
const label = computed(() =>
  props.href
    ? t('chat.document.open', { name: props.name })
    : t('chat.document.chip', { name: props.name, size: bytes.value }),
)
</script>

<template>
  <component
    :is="href ? 'a' : 'div'"
    :href="href"
    :target="href ? '_blank' : undefined"
    :rel="href ? 'noopener' : undefined"
    class="flex min-w-0 max-w-[16rem] items-center gap-2 rounded-control border border-line bg-surface-2 px-2.5 py-1.5 text-left"
    :class="href && 'transition-colors hover:border-brand/40'"
    :title="label"
    :aria-label="label"
  >
    <Icon name="document" :size="14" class="text-ink-3" />
    <span class="min-w-0 flex-1 truncate text-meta">{{ name }}</span>
    <span class="flex-none font-mono text-micro text-ink-3">{{ bytes }}</span>
  </component>
</template>
