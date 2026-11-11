<script setup lang="ts">
// The default model, and where session titles come from: the first or the
// latest prompt, or a model's summary (its model then picked below). Every
// change saves at once.
import { computed } from 'vue'
import { t } from '../../i18n'
import { editSettings, models } from '../../lib/state/models'
import type { Roles } from '../../lib/generated/Roles'
import type { Titles } from '../../lib/generated/Titles'
import ModelField from './ModelField.vue'
import NoProviders from './NoProviders.vue'

const TITLES: Titles[] = ['first', 'last', 'model']

const settings = computed(() => models.settings)

function setRole(role: keyof Roles, value: string): Promise<boolean> {
  const roles: Roles = { ...settings.value?.roles }
  if (value) roles[role] = value
  else delete roles[role]
  return editSettings('settings.set_roles', roles)
}

function setTitles(titles: Titles): void {
  if (titles !== settings.value?.titles) void editSettings('settings.set_titles', { titles })
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto px-4 pt-5 pb-6 sm:px-6">
    <NoProviders v-if="settings && !settings.providers.length" />
    <section v-else class="grid gap-x-4 gap-y-3 sm:grid-cols-2">
      <label class="flex flex-col gap-1.5">
        <span class="field-label">{{ t('settings.roleDefault') }}</span>
        <ModelField
          :model-value="settings?.roles.default ?? ''"
          :placeholder="t('settings.pickModel')"
          @update:model-value="setRole('default', $event)"
        />
      </label>
      <div class="flex flex-col gap-1.5">
        <span class="field-label">{{ t('settings.roleTitle') }}</span>
        <div
          class="flex rounded-control bg-surface-2 p-0.5"
          role="radiogroup"
          :aria-label="t('settings.roleTitle')"
        >
          <button
            v-for="x in TITLES"
            :key="x"
            type="button"
            role="radio"
            :aria-checked="settings?.titles === x"
            class="flex-1 rounded-xs px-2 py-1 text-meta font-semibold transition-colors duration-150"
            :class="settings?.titles === x ? 'bg-surface text-ink shadow-soft' : 'text-ink-3 hover:text-ink'"
            @click="setTitles(x)"
          >
            {{ t(`settings.titles_${x}`) }}
          </button>
        </div>
        <ModelField
          v-if="settings?.titles === 'model'"
          :model-value="settings.roles.title ?? ''"
          :placeholder="t('settings.pickModel')"
          @update:model-value="setRole('title', $event)"
        />
      </div>
    </section>
  </div>
</template>
