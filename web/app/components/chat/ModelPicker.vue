<script setup lang="ts">
// The composer's model chip: the model the next prompt runs on, picked from
// the ModelMenu. The effort is its own chip (EffortPicker).
import { computed } from 'vue'
import { t } from '../../i18n'
import { openPage } from '../../lib/router'
import { currentPick, models, pick, resolvePick } from '../../lib/state/models'
import type { ModelInfo } from '../../lib/generated/ModelInfo'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'
import ModelMenu from './ModelMenu.vue'

const use = computed(() => currentPick())
const resolved = computed(() => resolvePick(use.value))
const settings = computed(() => models.settings)
const empty = computed(() => !settings.value?.aliases.length && !settings.value?.providers.length)

/** The selected model's ref; with no pick, the default model's. */
const selected = computed(() => use.value?.model ?? settings.value?.roles.default ?? null)

function choose(m: ModelInfo, close: () => void): void {
  // Moving to an alias takes its effort; another keeps the shown one.
  const effort = m.target ? undefined : (resolved.value?.effort ?? undefined)
  pick(effort ? { model: m.ref, effort } : { model: m.ref })
  close()
}

function configure(close: () => void): void {
  close()
  openPage('settings', 'providers')
}
</script>

<template>
  <Popover v-if="settings" :label="t('settings.modelPicker')" placement="top-start" wide>
    <template #trigger="{ open, toggle }">
      <button
        type="button"
        class="inline-flex min-w-0 items-center gap-1.5 rounded-full py-1 pr-2 pl-2.5 text-meta font-semibold text-ink-2 transition-colors hover:bg-surface-2 hover:text-ink"
        :class="{ 'bg-surface-2 text-ink': open }"
        :title="resolved?.modelRef ?? ''"
        :aria-expanded="open"
        @click="toggle"
      >
        <Icon name="model" :size="14" class="text-ink-3" />
        <span class="truncate">{{ resolved?.label }}</span>
        <Icon name="down" :size="12" class="text-ink-3" />
      </button>
    </template>
    <template #default="{ close }">
      <div class="flex flex-col gap-2">
        <ModelMenu :selected="selected" @pick="choose($event, close)" @add="close" />
        <button
          v-if="!empty"
          type="button"
          class="flex items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-meta text-ink-3 transition-colors hover:bg-surface-2 hover:text-ink"
          @click="configure(close)"
        >
          <Icon name="settings" :size="13" />
          {{ t('settings.editModels') }}
        </button>
      </div>
    </template>
  </Popover>
</template>
