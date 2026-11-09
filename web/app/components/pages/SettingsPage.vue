<script setup lang="ts">
// The settings page (`settings.yaml`, at `/settings[/<section>]`): its
// sections at the root, one section below it. The header is the trail —
// Settings › section › the section's inner page — each crumb but the last
// going back to its page. Each section saves its own edits.
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

const SECTIONS = [
  { key: 'profiles', icon: 'model' },
  { key: 'providers', icon: 'link' },
] as const
type Section = (typeof SECTIONS)[number]['key']

const section = computed<Section | null>(() => SECTIONS.find((s) => s.key === state.page?.item)?.key ?? null)

/** Settings, then the section's own trail (its first crumb is the section
 *  itself), or the section alone while it is on its own page. */
const trail = computed<{ label: string; go?: () => void }[]>(() => {
  const s = section.value
  if (!s) return [{ label: t('settings.title') }]
  const root = { label: t('settings.title'), go: () => openPage('settings') }
  return layout.settingsTrail.length ? [root, ...layout.settingsTrail] : [root, { label: t(`settings.${s}`) }]
})

// A page booted from its address mounts before the socket is up.
watch(connected, (up) => up && void refreshSettings(), { immediate: true })
</script>

<template>
  <PageSheet :title="t('settings.title')" icon="settings" tone="text-ink-3">
    <template #title>
      <template v-for="(c, i) in trail" :key="i">
        <Icon v-if="i" name="chevron" :size="14" class="-mx-1 flex-none text-ink-3" />
        <button
          v-if="c.go && i < trail.length - 1"
          type="button"
          class="flex-none text-ink-3 transition-colors hover:text-ink"
          :disabled="layout.settingsBusy"
          @click="c.go"
        >
          {{ c.label }}
        </button>
        <span v-else class="min-w-0 truncate">{{ c.label }}</span>
      </template>
    </template>

    <nav v-if="!section" class="flex w-full max-w-2xl flex-col gap-1 p-3 sm:p-4">
      <button
        v-for="s in SECTIONS"
        :key="s.key"
        type="button"
        class="flex items-center gap-3 rounded-control px-3 py-3 text-left text-message text-ink-2 transition-colors duration-150 hover:bg-surface-2/60 hover:text-ink"
        @click="openPage('settings', s.key)"
      >
        <Icon :name="s.icon" :size="17" class="flex-none text-ink-3" />
        <span class="flex-1">{{ t(`settings.${s.key}`) }}</span>
        <Icon name="chevron" :size="14" class="flex-none text-ink-3" />
      </button>
    </nav>
    <ProfilesSettings v-else-if="section === 'profiles'" />
    <ProvidersSettings v-else />
  </PageSheet>
</template>
