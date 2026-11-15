<script setup lang="ts">
// The built-in provider's models: aliases, each a name standing for another
// provider's model at an effort. Sessions and roles name the alias, so
// pointing it at a newer model moves all of them. Every change saves at once.
import { computed, reactive } from 'vue'
import { t } from '../../i18n'
import { ALIAS_PROVIDER, editSettings, models, nearestEffort } from '../../lib/state/models'
import type { Effort } from '../../lib/generated/Effort'
import type { ModelInfo } from '../../lib/generated/ModelInfo'
import Icon from '../ui/Icon.vue'
import NoProviders from './NoProviders.vue'
import ModelField from './ModelField.vue'

const aliases = computed(() => models.settings?.aliases ?? [])

/** Names being edited, by the saved name. */
const names = reactive<Record<string, string>>({})
const draft = reactive({ name: '', model: '' })

function shownEffort(m: ModelInfo): Effort | null {
  return m.efforts.length ? nearestEffort(m.target?.effort ?? 'high', m.efforts) : null
}

function save(m: ModelInfo, patch: { model?: string; effort?: Effort; rename?: string }): Promise<boolean> {
  return editSettings('settings.set_alias', {
    name: patch.rename ?? m.name,
    rename: patch.rename ? m.name : undefined,
    model: patch.model ?? m.target?.model,
    effort: patch.effort ?? m.target?.effort,
  })
}

async function rename(m: ModelInfo): Promise<void> {
  const next = (names[m.name] ?? m.name).trim()
  delete names[m.name]
  if (next && next !== m.name) await save(m, { rename: next })
}

function remove(m: ModelInfo): void {
  void editSettings('settings.remove_alias', { name: m.name })
}

const canAdd = computed(
  () => !!draft.name.trim() && !!draft.model && !aliases.value.some((m) => m.name === draft.name.trim()),
)

async function add(): Promise<void> {
  if (!canAdd.value) return
  const name = draft.name.trim()
  if (!(await editSettings('settings.set_alias', { name, model: draft.model }))) return
  // The first model there is to run on becomes the default.
  const s = models.settings
  if (s && !s.roles.default)
    await editSettings('settings.set_roles', { ...s.roles, default: `${ALIAS_PROVIDER}/${name}` })
  draft.name = ''
  draft.model = ''
}
</script>

<template>
  <NoProviders v-if="models.settings && !models.settings.providers.length" />
  <section v-else class="flex flex-col gap-2">
    <div
      v-for="m in aliases"
      :key="m.ref"
      class="flex flex-wrap items-center gap-2 rounded-card border border-line-soft bg-surface px-3 py-2.5 md:flex-nowrap"
    >
      <input
        :value="names[m.name] ?? m.name"
        type="text"
        class="w-28 flex-none font-semibold"
        :aria-label="t('settings.aliasName')"
        autocomplete="off"
        spellcheck="false"
        @input="names[m.name] = ($event.target as HTMLInputElement).value"
        @blur="rename(m)"
        @keydown.enter.prevent="($event.target as HTMLInputElement).blur()"
      />
      <div class="min-w-40 flex-1 md:min-w-0">
        <ModelField
          :model-value="m.target?.model ?? ''"
          :aliases="false"
          @update:model-value="save(m, { model: $event })"
        />
      </div>
      <div
        v-if="m.efforts.length > 1"
        class="flex flex-none rounded-control bg-surface-2 p-0.5"
        role="radiogroup"
        :aria-label="t('settings.effort')"
      >
        <button
          v-for="e in m.efforts"
          :key="e"
          type="button"
          role="radio"
          :aria-checked="shownEffort(m) === e"
          class="rounded-xs px-2 py-0.5 text-meta font-semibold transition-colors duration-150"
          :class="shownEffort(m) === e ? 'bg-surface text-ink shadow-soft' : 'text-ink-3 hover:text-ink'"
          @click="save(m, { effort: e })"
        >
          {{ e }}
        </button>
      </div>
      <button
        type="button"
        class="icon-btn size-7 flex-none"
        :title="t('settings.removeAlias')"
        :aria-label="t('settings.removeAlias')"
        @click="remove(m)"
      >
        <Icon name="trash" :size="13" />
      </button>
    </div>

    <form class="flex flex-wrap items-center gap-2 pt-1" @submit.prevent="add">
      <input
        v-model="draft.name"
        type="text"
        class="w-28 flex-none"
        :placeholder="t('settings.aliasName')"
        autocomplete="off"
        spellcheck="false"
      />
      <div class="min-w-48 flex-1">
        <ModelField v-model="draft.model" :aliases="false" :placeholder="t('settings.pickModel')" />
      </div>
      <button type="submit" class="btn flex-none" :disabled="!canAdd">
        <Icon name="plus" :size="13" />
        {{ t('settings.addAlias') }}
      </button>
    </form>
  </section>
</template>
