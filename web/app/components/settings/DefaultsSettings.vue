<script setup lang="ts">
// Where session titles come from (the first or the latest prompt, or a
// model's summary, its model then picked below), and the model that writes
// compaction summaries. Changes are a draft until Apply saves them.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { editSettings, models } from '../../lib/state/models'
import type { Roles } from '../../lib/generated/Roles'
import type { Titles } from '../../lib/generated/Titles'
import Select from '../ui/Select.vue'
import ModelField from './ModelField.vue'
import NoProviders from './NoProviders.vue'
import SettingGroup from './SettingGroup.vue'
import SettingRow from './SettingRow.vue'
import ApplyBar from './ApplyBar.vue'

const TITLES: Titles[] = ['first', 'last', 'model']

interface Form {
  titles: Titles
  /** Model refs; empty = unset. */
  title: string
  compact: string
}

const settings = computed(() => models.settings)
const titleOptions = computed(() => TITLES.map((x) => ({ value: x, label: t(`settings.titles_${x}`) })))

const saved = computed<Form>(() => ({
  titles: settings.value?.titles ?? 'first',
  title: settings.value?.roles.title ?? '',
  compact: settings.value?.roles.compact ?? '',
}))
/** The settings being edited: the draft, else the saved ones. */
const draft = ref<Form | null>(null)
const form = computed(() => draft.value ?? saved.value)
const changed = computed(() =>
  (Object.keys(form.value) as (keyof Form)[]).some((k) => form.value[k] !== saved.value[k]),
)
const saving = ref(false)

function set(patch: Partial<Form>): void {
  draft.value = { ...form.value, ...patch }
}

async function apply(): Promise<void> {
  const f = form.value
  saving.value = true
  let ok = true
  if (f.titles !== saved.value.titles) ok = await editSettings('settings.set_titles', { titles: f.titles })
  if (ok && (f.title !== saved.value.title || f.compact !== saved.value.compact)) {
    const roles: Roles = { ...settings.value?.roles }
    for (const role of ['title', 'compact'] as const) {
      if (f[role]) roles[role] = f[role]
      else delete roles[role]
    }
    ok = await editSettings('settings.set_roles', roles)
  }
  if (ok) draft.value = null
  saving.value = false
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div class="@container flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-4 pt-4 pb-6 sm:px-6">
      <NoProviders v-if="settings && !settings.providers.length" />
      <template v-else>
        <SettingGroup :title="t('settings.roleTitle')">
          <SettingRow :label="t('settings.titleSource')" wide>
            <Select
              :model-value="form.titles"
              :options="titleOptions"
              :label="t('settings.titleSource')"
              @update:model-value="set({ titles: $event })"
            />
          </SettingRow>
          <SettingRow v-if="form.titles === 'model'" :label="t('settings.titleModel')" wide>
            <ModelField
              :model-value="form.title"
              :placeholder="t('settings.pickModel')"
              @update:model-value="set({ title: $event })"
            />
          </SettingRow>
        </SettingGroup>
        <SettingGroup :title="t('settings.roleCompact')">
          <SettingRow :label="t('settings.compactModel')" wide>
            <ModelField
              :model-value="form.compact"
              :placeholder="t('settings.sameModel')"
              clearable
              @update:model-value="set({ compact: $event })"
            />
          </SettingRow>
        </SettingGroup>
      </template>
    </div>
    <ApplyBar
      v-if="settings?.providers.length"
      :changed="changed"
      :saving="saving"
      @discard="draft = null"
      @apply="apply"
    />
  </div>
</template>
