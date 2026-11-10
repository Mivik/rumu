<script setup lang="ts">
// The settings page (`settings.yaml`, at `/settings[/<section>]`): the
// sections listed down the side, the open one beside them. A narrow sheet
// shows one pane at a time, the list at the root; a wide one always has a
// section open, the first when the address names none. The header is the
// trail — Settings › section › the section's inner page — each crumb but the
// last going back to its page. Each section saves its own edits.
import { computed, onBeforeUnmount, onMounted, ref, watch, watchEffect } from 'vue'
import { t } from '../../i18n'
import { layout } from '../../lib/layout'
import { openPage, pageUp, replacePage } from '../../lib/router'
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
  // wide, the list beside the section is the way back
  const root = { label: t('settings.title'), go: wide.value ? undefined : pageUp }
  return layout.settingsTrail.length ? [root, ...layout.settingsTrail] : [root, { label: t(`settings.${s}`) }]
})

// The sheet is wide when it shows the list beside the item (PageSheet's
// `@2xl` container query, 42rem).
const sheet = ref<{ $el: HTMLElement } | null>(null)
const wide = ref(false)
let observer: ResizeObserver | undefined
onMounted(() => {
  observer = new ResizeObserver(([entry]) => {
    const rem = parseFloat(getComputedStyle(document.documentElement).fontSize)
    wide.value = (entry?.contentRect.width ?? 0) >= 42 * rem
  })
  if (sheet.value) observer.observe(sheet.value.$el)
})
onBeforeUnmount(() => observer?.disconnect())
watchEffect(() => {
  if (wide.value && !section.value) replacePage(SECTIONS[0].key)
})

// A page booted from its address mounts before the socket is up.
watch(connected, (up) => up && void refreshSettings(), { immediate: true })
</script>

<template>
  <PageSheet
    ref="sheet"
    :title="t('settings.title')"
    icon="settings"
    tone="text-ink-3"
    :detail="!!section"
    :item="section ?? ''"
    @back="pageUp"
  >
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
        <span v-else :class="i < trail.length - 1 ? 'flex-none text-ink-3' : 'min-w-0 truncate'">{{
          c.label
        }}</span>
      </template>
    </template>

    <template #list>
      <nav class="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto p-3">
        <button
          v-for="s in SECTIONS"
          :key="s.key"
          type="button"
          class="flex items-center gap-2.5 rounded-control px-2.5 py-2.5 text-left transition-colors duration-150"
          :class="
            s.key === section
              ? 'bg-surface font-semibold text-ink shadow-soft'
              : 'text-ink-2 hover:bg-surface/70 hover:text-ink'
          "
          :aria-current="s.key === section ? 'page' : undefined"
          :disabled="layout.settingsBusy && s.key !== section"
          @click="s.key !== section && openPage('settings', s.key)"
        >
          <Icon
            :name="s.icon"
            :size="16"
            class="flex-none"
            :class="s.key === section ? 'text-ink' : 'text-ink-3'"
          />
          <span class="flex-1">{{ t(`settings.${s.key}`) }}</span>
          <span class="flex flex-none text-ink-3 @2xl:hidden"><Icon name="chevron" :size="14" /></span>
        </button>
      </nav>
    </template>

    <ProfilesSettings v-if="section === 'profiles'" />
    <ProvidersSettings v-else-if="section === 'providers'" />
  </PageSheet>
</template>
