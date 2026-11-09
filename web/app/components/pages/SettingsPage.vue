<script setup lang="ts">
// The settings page (`settings.yaml`, at `/settings[/<section>]`): the
// sections as tabs in the header, the open one below. Each section saves its
// own edits; a section's inner pages show as crumbs above it.
import { computed, watch } from 'vue'
import { t } from '../../i18n'
import { layout } from '../../lib/layout'
import { openPage } from '../../lib/router'
import { refreshSettings } from '../../lib/state/models'
import { connected, state } from '../../lib/store'
import ProfilesSettings from '../settings/ProfilesSettings.vue'
import ProvidersSettings from '../settings/ProvidersSettings.vue'
import Icon from '../ui/Icon.vue'
import PageSheet from './PageSheet.vue'

const SECTIONS = ['profiles', 'providers'] as const
type Section = (typeof SECTIONS)[number]

const section = computed<Section>(() => (state.page?.item === 'providers' ? 'providers' : 'profiles'))

/** The heading: the section, or the trail to the page below it. */
const trail = computed(() =>
  layout.settingsTrail.length
    ? layout.settingsTrail
    : [{ label: t(`settings.${section.value}`), go: undefined }],
)

/** A section; the open one again goes back to its own page. */
function openSection(key: Section): void {
  if (key === section.value) layout.settingsTrail[0]?.go?.()
  else openPage('settings', key === 'profiles' ? null : key)
}

// A page booted from its address mounts before the socket is up.
watch(connected, (up) => up && void refreshSettings(), { immediate: true })
</script>

<template>
  <PageSheet :title="t('settings.title')" icon="settings" tone="text-ink-3">
    <template #actions>
      <div class="flex flex-none rounded-control bg-surface-2 p-0.5" role="tablist">
        <button
          v-for="key in SECTIONS"
          :key="key"
          type="button"
          role="tab"
          :aria-selected="key === section"
          class="rounded-xs px-2.5 py-1 text-meta font-semibold transition-colors duration-150"
          :class="key === section ? 'bg-surface text-ink shadow-soft' : 'text-ink-3 hover:text-ink'"
          :disabled="layout.settingsBusy"
          @click="openSection(key)"
        >
          {{ t(`settings.${key}`) }}
        </button>
      </div>
    </template>

    <header class="flex flex-none items-center gap-3 px-4 pt-5 pb-1 sm:px-6">
      <h2 class="flex min-w-0 items-center gap-1.5 text-title font-bold">
        <template v-for="(c, i) in trail" :key="i">
          <Icon v-if="i" name="chevron" :size="14" class="flex-none text-ink-3" />
          <button
            v-if="c.go"
            type="button"
            class="flex-none text-ink-3 transition-colors hover:text-ink"
            :disabled="layout.settingsBusy"
            @click="c.go"
          >
            {{ c.label }}
          </button>
          <span v-else class="min-w-0 truncate">{{ c.label }}</span>
        </template>
      </h2>
    </header>
    <ProfilesSettings v-if="section === 'profiles'" />
    <ProvidersSettings v-else />
  </PageSheet>
</template>
