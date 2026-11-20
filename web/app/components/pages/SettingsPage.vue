<script setup lang="ts">
// The settings page (`settings.yaml`, at `/settings[/<section>]`): the
// sections listed down the side, the open one beside them. A narrow sheet
// shows one pane at a time, the list at the root; a wide one always has a
// section open, the first when the address names none. The header is the
// trail — Settings › section › the section's inner page — each crumb but the
// last going back to its page. Each section saves its own edits. Beside the
// sessions sidebar (lg) it opens as a dialog over the desk instead (app.vue).
import { computed, onBeforeUnmount, onMounted, ref, watch, watchEffect } from 'vue'
import { t } from '../../i18n'
import { layout } from '../../lib/layout'
import { closePage, openPage, pageUp, replacePage } from '../../lib/router'
import { refreshSettings } from '../../lib/state/models'
import { connected, state } from '../../lib/store'
import DefaultsSettings from '../settings/DefaultsSettings.vue'
import McpSettings from '../settings/McpSettings.vue'
import ProvidersSettings from '../settings/ProvidersSettings.vue'
import SkillsSettings from '../settings/SkillsSettings.vue'
import UiSettings from '../settings/UiSettings.vue'
import Icon from '../ui/Icon.vue'
import PageSheet from './PageSheet.vue'

defineProps<{ modal?: boolean }>()

const SECTIONS = [
  { key: 'defaults', icon: 'model' },
  { key: 'providers', icon: 'link' },
  { key: 'mcp', icon: 'mcp' },
  { key: 'skills', icon: 'note' },
  { key: 'ui', icon: 'tune' },
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

/** Another section opens; the open one goes back to its own page (the
 *  first crumb of its trail). */
function choose(key: Section): void {
  if (key !== section.value) openPage('settings', key)
  else layout.settingsTrail[0]?.go?.()
}

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
    :modal="modal"
    @back="pageUp"
    @close="closePage"
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
          :disabled="layout.settingsBusy"
          @click="choose(s.key)"
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

    <DefaultsSettings v-if="section === 'defaults'" />
    <ProvidersSettings v-else-if="section === 'providers'" />
    <McpSettings v-else-if="section === 'mcp'" />
    <SkillsSettings v-else-if="section === 'skills'" />
    <UiSettings v-else-if="section === 'ui'" />
  </PageSheet>
</template>
