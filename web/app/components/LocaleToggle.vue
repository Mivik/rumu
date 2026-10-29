<script setup lang="ts">
// The language switch, pinned to the foot of the sidebar. Two quiet labels:
// the current one inked, the other reachable on hover.
import { LOCALE_LABELS, SUPPORTED_LOCALES, locale, setLocale } from '../i18n'
</script>

<template>
  <div class="flex items-center gap-1.5 border-t border-line-soft px-3 pt-2 pb-1">
    <button
      v-for="l in SUPPORTED_LOCALES"
      :key="l"
      type="button"
      class="rounded-xs px-1 py-0.5 text-micro transition-colors"
      :class="locale === l ? 'font-semibold text-ink' : 'text-ink-3 hover:text-ink-2'"
      :aria-pressed="locale === l"
      @click="setLocale(l)"
    >
      {{ LOCALE_LABELS[l] }}
    </button>
  </div>
</template>
