<script setup lang="ts">
// The face of a card, shared by the chat's card reader and the library so a
// card reads the same everywhere: serif heading, subtitle, description, body, keywords,
// then the frontmatter details. Callers fill the slots with their extras.
import { t } from '../i18n'

defineProps<{
  /** Folder (or deck/folder) shown above the title; empty for none. */
  dir: string
  title: string
  description?: string | null
  keywords: string[]
  /** Rendered markdown of the body. */
  html: string
}>()
</script>

<template>
  <div class="card-face">
    <header class="pb-1">
      <div class="pr-6 font-card">
        <p v-if="dir" class="break-anywhere text-meta leading-none text-ink-3">{{ dir }}/</p>
        <h1 class="text-[24px] leading-tight font-semibold sm:text-[28px]">{{ title }}</h1>
      </div>
      <slot name="subtitle" />
      <p v-if="description" class="mt-3 text-[14px] leading-relaxed text-ink-2">{{ description }}</p>
      <slot name="meta" />
    </header>
    <slot name="notice" />
    <div class="md mt-5 text-ink" v-html="html" />
    <div v-if="keywords.length" class="mt-5 flex flex-wrap gap-1.5">
      <span v-for="(keyword, i) in keywords" :key="i" class="keyword-tag">{{ keyword }}</span>
    </div>
    <details class="mt-5 text-meta text-ink-2">
      <summary class="w-fit cursor-pointer">{{ t('cards.drawer.details') }}</summary>
      <dl class="mt-4 grid grid-cols-[minmax(0,80px)_minmax(0,1fr)] gap-x-4 gap-y-3 leading-relaxed">
        <slot name="details" />
      </dl>
    </details>
  </div>
</template>

<style scoped>
.card-face {
  overflow-wrap: anywhere;
}
.card-face :deep(.md) {
  line-height: 1.75;
}
</style>
