<script setup lang="ts">
// The settings dialog (`settings.yaml`): sections on the left, the open
// section on the right. Each section saves its own edits.
import { computed, watch } from 'vue'
import { t } from '../i18n'
import { refreshSettings } from '../lib/state/models'
import { closeSettings, layout } from '../lib/layout'
import ProfilesSettings from './settings/ProfilesSettings.vue'
import ProvidersSettings from './settings/ProvidersSettings.vue'
import Icon from './ui/Icon.vue'
import Modal from './ui/Modal.vue'

const SECTIONS = [
  { key: 'profiles', icon: 'model' },
  { key: 'providers', icon: 'link' },
] as const

/** The heading: the section, or the trail to the page below it. */
const trail = computed(() =>
  layout.settingsTrail.length
    ? layout.settingsTrail
    : [{ label: t(`settings.${layout.settingsSection}`), go: undefined }],
)

/** A section; the open one again goes back to its own page. */
function openSection(key: (typeof SECTIONS)[number]['key']): void {
  if (key === layout.settingsSection) layout.settingsTrail[0]?.go?.()
  else layout.settingsSection = key
}

watch(
  () => layout.settingsOpen,
  (open) => {
    if (open) void refreshSettings()
  },
)
</script>

<template>
  <Modal
    :open="layout.settingsOpen"
    :label="t('settings.title')"
    :z="55"
    wrapper-class="p-3 sm:p-6"
    panel-class="h-[min(90dvh,860px)] w-full max-w-[1120px] rounded-sheet"
    @close="closeSettings"
  >
    <div class="flex min-h-0 flex-1 flex-col sm:flex-row">
      <nav
        class="flex flex-none flex-col border-b border-line-soft bg-canvas sm:w-56 sm:border-r sm:border-b-0"
      >
        <header class="px-5 pt-5 pb-3">
          <h2 class="text-title font-bold">{{ t('settings.title') }}</h2>
        </header>
        <div class="flex flex-col gap-0.5 px-3 pb-3">
          <button
            v-for="s in SECTIONS"
            :key="s.key"
            type="button"
            class="flex items-center gap-2.5 rounded-control px-2.5 py-2 text-left text-secondary transition-colors duration-150"
            :class="
              s.key === layout.settingsSection
                ? 'bg-surface font-semibold text-ink'
                : 'text-ink-2 hover:bg-surface/70 hover:text-ink'
            "
            :aria-current="s.key === layout.settingsSection ? 'page' : undefined"
            @click="openSection(s.key)"
          >
            <Icon :name="s.icon" :size="15" class="text-ink-3" />
            {{ t(`settings.${s.key}`) }}
          </button>
        </div>
      </nav>

      <div class="flex min-h-0 min-w-0 flex-1 flex-col">
        <header class="flex flex-none items-center gap-3 px-6 pt-5 pb-1">
          <h3 class="flex min-w-0 items-center gap-1.5 text-title font-bold">
            <template v-for="(c, i) in trail" :key="i">
              <Icon v-if="i" name="chevron" :size="14" class="flex-none text-ink-3" />
              <button
                v-if="c.go"
                type="button"
                class="flex-none text-ink-3 transition-colors hover:text-ink"
                @click="c.go"
              >
                {{ c.label }}
              </button>
              <span v-else class="min-w-0 truncate">{{ c.label }}</span>
            </template>
          </h3>
          <button
            type="button"
            class="icon-btn ml-auto size-8 flex-none"
            :title="t('common.close')"
            :aria-label="t('common.close')"
            @click="closeSettings"
          >
            <Icon name="close" :size="14" />
          </button>
        </header>
        <ProfilesSettings v-if="layout.settingsSection === 'profiles'" />
        <ProvidersSettings v-else />
      </div>
    </div>
  </Modal>
</template>
