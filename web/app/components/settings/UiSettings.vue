<script setup lang="ts">
// How the app looks and behaves: the theme (the system's or a chosen one),
// and how the conversation folds its work — an edit's diff, and the steps a
// turn took. All of it belongs to this browser: the server never sees it,
// so a phone and a desk each read the way their owner likes.
import { t } from '../../i18n'
import { appearance, type ThemeMode } from '../../lib/state/appearance'

const THEMES: ThemeMode[] = ['system', 'light', 'dark']
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto px-4 pt-5 pb-6 sm:px-6">
    <section class="grid gap-x-4 gap-y-3 sm:grid-cols-2">
      <div class="flex flex-col gap-1.5">
        <span class="field-label">{{ t('settings.uiTheme') }}</span>
        <div
          class="flex rounded-control bg-surface-2 p-0.5"
          role="radiogroup"
          :aria-label="t('settings.uiTheme')"
        >
          <button
            v-for="x in THEMES"
            :key="x"
            type="button"
            role="radio"
            :aria-checked="appearance.theme === x"
            class="flex-1 rounded-xs px-2 py-1 text-meta font-semibold transition-colors duration-150"
            :class="appearance.theme === x ? 'bg-surface text-ink shadow-soft' : 'text-ink-3 hover:text-ink'"
            @click="appearance.theme = x"
          >
            {{ t(`settings.uiTheme_${x}`) }}
          </button>
        </div>
      </div>
    </section>

    <section class="flex flex-col gap-3">
      <label class="flex items-start gap-2.5">
        <input v-model="appearance.foldEdits" type="checkbox" class="mt-0.5" />
        <span class="flex flex-col gap-0.5">
          <span class="text-meta text-ink">{{ t('settings.uiFoldEdits') }}</span>
          <span class="field-hint">{{ t('settings.uiFoldEditsHint') }}</span>
        </span>
      </label>
      <label class="flex items-start gap-2.5">
        <input v-model="appearance.foldWork" type="checkbox" class="mt-0.5" />
        <span class="flex flex-col gap-0.5">
          <span class="text-meta text-ink">{{ t('settings.uiFoldWork') }}</span>
          <span class="field-hint">{{ t('settings.uiFoldWorkHint') }}</span>
        </span>
      </label>
    </section>
  </div>
</template>
