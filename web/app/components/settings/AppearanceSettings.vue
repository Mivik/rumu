<script setup lang="ts">
// The appearance section: the theme, and how much of a turn's work shows —
// a preset over the settings under it, then a preview of a made-up turn.
// Changes are a draft until Apply saves them (lib/state/appearance): the
// page wears a drafted theme, the preview a drafted work view; Discard or
// leaving the section drops it.
import { computed, onBeforeUnmount, ref, watchEffect } from 'vue'
import { t, type MessageKey } from '../../i18n'
import type { Fold } from '../../lib/generated/Fold'
import type { Theme } from '../../lib/generated/Theme'
import type { Appearance } from '../../lib/generated/Appearance'
import type { WorkView } from '../../lib/generated/WorkView'
import {
  appearance,
  presetOf,
  sameWork,
  setAppearance,
  themeDraft,
  WORK_PRESETS,
  type WorkPreset,
} from '../../lib/state/appearance'
import ApplyBar from './ApplyBar.vue'
import Select from '../ui/Select.vue'
import Switch from '../ui/Switch.vue'
import SettingGroup from './SettingGroup.vue'
import SettingRow from './SettingRow.vue'
import WorkPreview from './WorkPreview.vue'

type FoldKey = 'steps' | 'thinking' | 'calls' | 'edits'

const THEMES: Theme[] = ['system', 'light', 'dark']
const PRESETS = Object.keys(WORK_PRESETS) as WorkPreset[]
/** The folds and the values each offers (a diff has no `auto`). */
const FOLDS: [FoldKey, Fold[]][] = [
  ['steps', ['folded', 'auto', 'open']],
  ['thinking', ['folded', 'auto', 'open']],
  ['calls', ['folded', 'auto', 'open']],
  ['edits', ['folded', 'open']],
]

/** The settings being edited: the draft, else the saved ones. */
const draft = ref<Appearance | null>(null)
const shown = computed(() => draft.value ?? appearance.value)
const work = computed(() => shown.value.work)
const workPreset = computed(() => presetOf(work.value))
/** Do files stand outside folded steps, for a limit to apply to? */
const outsideList = computed(() => work.value.always_show_edits && work.value.steps !== 'open')
const changed = computed(
  () => shown.value.theme !== appearance.value.theme || !sameWork(work.value, appearance.value.work),
)
const saving = ref(false)
watchEffect(() => (themeDraft.value = draft.value?.theme ?? null))
onBeforeUnmount(() => (themeDraft.value = null))
const themeOptions = computed(() => THEMES.map((x) => ({ value: x, label: t(`settings.theme_${x}`) })))
const foldOptions = (key: FoldKey, values: Fold[]) =>
  values.map((x) => ({ value: x, label: t(`settings.work_${key}_${x}` as MessageKey) }))

function setTheme(theme: Theme): void {
  draft.value = { ...shown.value, theme }
}

function setWork(patch: Partial<WorkView>): void {
  draft.value = { ...shown.value, work: { ...work.value, ...patch } }
}

async function apply(): Promise<void> {
  saving.value = true
  if (await setAppearance(shown.value)) draft.value = null
  saving.value = false
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div class="@container flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-4 pt-4 pb-6 sm:px-6">
      <SettingGroup>
        <SettingRow :label="t('settings.theme')">
          <Select
            :model-value="shown.theme"
            :options="themeOptions"
            :label="t('settings.theme')"
            @update:model-value="setTheme"
          />
        </SettingRow>
      </SettingGroup>

      <SettingGroup :title="t('settings.work')">
        <SettingRow :label="t('settings.workPreset')" wide>
          <template #label>
            <span
              v-if="!workPreset"
              class="rounded-xs bg-surface-2 px-1.5 text-micro font-semibold text-ink-3"
            >
              {{ t('settings.workPresetCustom') }}
            </span>
          </template>
          <div
            class="flex w-full rounded-control bg-surface-2 p-0.5"
            role="radiogroup"
            :aria-label="t('settings.workPreset')"
          >
            <button
              v-for="x in PRESETS"
              :key="x"
              type="button"
              role="radio"
              :aria-checked="workPreset === x"
              class="flex-1 rounded-xs px-2 py-1 text-meta font-semibold whitespace-nowrap transition-colors duration-150"
              :class="workPreset === x ? 'bg-surface text-ink shadow-soft' : 'text-ink-3 hover:text-ink'"
              @click="workPreset !== x && setWork(WORK_PRESETS[x])"
            >
              {{ t(`settings.workPreset_${x}`) }}
            </button>
          </div>
        </SettingRow>
        <template v-for="[key, values] in FOLDS" :key="key">
          <SettingRow :label="t(`settings.work_${key}`)">
            <Select
              :model-value="work[key]"
              :options="foldOptions(key, values)"
              :label="t(`settings.work_${key}`)"
              @update:model-value="setWork({ [key]: $event })"
            />
          </SettingRow>
          <!-- with the steps always open, nothing folds them away -->
          <SettingRow
            v-if="key === 'steps'"
            :label="t('settings.work_always_show_edits')"
            :disabled="work.steps === 'open'"
          >
            <Switch
              :model-value="work.always_show_edits"
              :label="t('settings.work_always_show_edits')"
              :disabled="work.steps === 'open'"
              @update:model-value="setWork({ always_show_edits: $event })"
            />
          </SettingRow>
          <!-- only the files outside the fold are limited -->
          <SettingRow v-if="key === 'edits'" :label="t('settings.work_limit_edits')" :disabled="!outsideList">
            <Switch
              :model-value="work.limit_edits"
              :label="t('settings.work_limit_edits')"
              :disabled="!outsideList"
              @update:model-value="setWork({ limit_edits: $event })"
            />
          </SettingRow>
        </template>
        <WorkPreview :work="work" />
      </SettingGroup>
    </div>
    <ApplyBar :changed="changed" :saving="saving" @discard="draft = null" @apply="apply" />
  </div>
</template>
