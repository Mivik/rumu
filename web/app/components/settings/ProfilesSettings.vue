<script setup lang="ts">
// Model profiles and the roles bound to them. A profile is a named model +
// effort: sessions and roles name the profile, so pointing it at a newer
// model moves all of them. Every change saves at once.
import { computed, reactive } from 'vue'
import { t } from '../../i18n'
import { editSettings, modelByRef, models, nearestEffort } from '../../lib/state/models'
import type { Effort } from '../../lib/generated/Effort'
import type { ProfileInfo } from '../../lib/generated/ProfileInfo'
import type { Roles } from '../../lib/generated/Roles'
import Icon from '../ui/Icon.vue'
import NoProviders from './NoProviders.vue'
import Select from '../ui/Select.vue'
import type { SelectOption } from '../ui/Select.vue'

const settings = computed(() => models.settings)
const profiles = computed(() => settings.value?.profiles ?? [])

const modelOptions = computed<SelectOption[]>(() =>
  (settings.value?.providers ?? []).flatMap((p) =>
    p.models.map((m) => ({ value: m.ref, label: m.name, hint: p.name })),
  ),
)
const profileOptions = computed<SelectOption[]>(() =>
  profiles.value.map((p) => ({ value: p.name, label: p.name, hint: p.model, hintMono: true })),
)
const titleOptions = computed<SelectOption[]>(() => [
  { value: '', label: t('settings.roleTitleOff') },
  ...profileOptions.value,
])

/** Names being edited, by the profile's saved name. */
const names = reactive<Record<string, string>>({})
const draft = reactive({ name: '', model: '' })

function efforts(p: { model: string }): Effort[] {
  return modelByRef(p.model)?.efforts ?? []
}

function shownEffort(p: ProfileInfo): Effort | null {
  const e = efforts(p)
  return e.length ? nearestEffort(p.effort ?? 'high', e) : null
}

function save(p: ProfileInfo, patch: Partial<ProfileInfo> & { rename?: string }): Promise<boolean> {
  const name = patch.rename ?? p.name
  return editSettings('settings.set_profile', {
    name,
    rename: patch.rename ? p.name : undefined,
    model: patch.model ?? p.model,
    effort: patch.effort ?? p.effort,
  })
}

async function rename(p: ProfileInfo): Promise<void> {
  const next = (names[p.name] ?? p.name).trim()
  delete names[p.name]
  if (next && next !== p.name) await save(p, { rename: next })
}

function remove(p: ProfileInfo): void {
  void editSettings('settings.remove_profile', { name: p.name })
}

async function add(): Promise<void> {
  const name = draft.name.trim()
  if (!name || !draft.model) return
  const first = !profiles.value.length
  if (!(await editSettings('settings.set_profile', { name, model: draft.model }))) return
  if (first) await setRole('default', name)
  draft.name = ''
  draft.model = ''
}

function setRole(role: keyof Roles, value: string): Promise<boolean> {
  const roles: Roles = { ...settings.value?.roles }
  if (value) roles[role] = value
  else delete roles[role]
  return editSettings('settings.set_roles', roles)
}

const canAdd = computed(
  () => !!draft.name.trim() && !!draft.model && !profiles.value.some((p) => p.name === draft.name.trim()),
)
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto px-6 pt-3 pb-6">
    <NoProviders v-if="settings && !modelOptions.length" />

    <template v-else>
      <section v-if="profiles.length" class="grid gap-x-4 gap-y-3 sm:grid-cols-2">
        <label class="flex flex-col gap-1.5">
          <span class="field-label">{{ t('settings.roleDefault') }}</span>
          <Select
            :model-value="settings?.roles.default ?? ''"
            :options="profileOptions"
            :label="t('settings.roleDefault')"
            :placeholder="t('settings.pickProfile')"
            @update:model-value="setRole('default', $event)"
          />
        </label>
        <label class="flex flex-col gap-1.5">
          <span class="field-label">{{ t('settings.roleTitle') }}</span>
          <Select
            :model-value="settings?.roles.title ?? ''"
            :options="titleOptions"
            :label="t('settings.roleTitle')"
            @update:model-value="setRole('title', $event)"
          />
        </label>
      </section>

      <section class="flex flex-col gap-2">
        <div
          v-for="p in profiles"
          :key="p.name"
          class="flex flex-wrap items-center gap-2 rounded-card border border-line-soft bg-surface px-3 py-2.5 md:flex-nowrap"
        >
          <input
            :value="names[p.name] ?? p.name"
            type="text"
            class="w-28 flex-none font-semibold"
            :aria-label="t('settings.profileName')"
            autocomplete="off"
            spellcheck="false"
            @input="names[p.name] = ($event.target as HTMLInputElement).value"
            @blur="rename(p)"
            @keydown.enter.prevent="($event.target as HTMLInputElement).blur()"
          />
          <div class="min-w-40 flex-1 md:min-w-0">
            <Select
              :model-value="p.model"
              :options="modelOptions"
              :label="t('settings.model')"
              @update:model-value="save(p, { model: $event })"
            />
          </div>
          <div
            v-if="efforts(p).length > 1"
            class="flex flex-none rounded-control bg-surface-2 p-0.5"
            role="radiogroup"
            :aria-label="t('settings.effort')"
          >
            <button
              v-for="e in efforts(p)"
              :key="e"
              type="button"
              role="radio"
              :aria-checked="shownEffort(p) === e"
              class="rounded-xs px-2 py-0.5 text-meta font-semibold transition-colors duration-150"
              :class="shownEffort(p) === e ? 'bg-surface text-ink shadow-soft' : 'text-ink-3 hover:text-ink'"
              @click="save(p, { effort: e })"
            >
              {{ e }}
            </button>
          </div>
          <button
            type="button"
            class="icon-btn size-7 flex-none"
            :title="t('settings.removeProfile')"
            :aria-label="t('settings.removeProfile')"
            @click="remove(p)"
          >
            <Icon name="trash" :size="13" />
          </button>
        </div>

        <form v-if="modelOptions.length" class="flex flex-wrap items-center gap-2 pt-1" @submit.prevent="add">
          <input
            v-model="draft.name"
            type="text"
            class="w-28 flex-none"
            :placeholder="t('settings.profileName')"
            autocomplete="off"
            spellcheck="false"
          />
          <div class="min-w-48 flex-1">
            <Select
              v-model="draft.model"
              :options="modelOptions"
              :label="t('settings.model')"
              :placeholder="t('settings.pickModel')"
            />
          </div>
          <button type="submit" class="btn flex-none" :disabled="!canAdd">
            <Icon name="plus" :size="13" />
            {{ t('settings.addProfile') }}
          </button>
        </form>
      </section>
    </template>
  </div>
</template>
