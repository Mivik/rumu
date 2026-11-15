<script setup lang="ts">
// The models to pick from, as the composer's chip lists them: a search,
// the `lead` slot (unless searching), the aliases (unless left out), pinned
// ones, then each provider's models (folded, but for the selected model's
// provider). Pins toggle in place.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { editSettings, modelByRef, models } from '../../lib/state/models'
import type { ModelInfo } from '../../lib/generated/ModelInfo'
import Icon from '../ui/Icon.vue'
import NoProviders from '../settings/NoProviders.vue'

const props = withDefaults(defineProps<{ selected: string | null; aliases?: boolean }>(), { aliases: true })
const emit = defineEmits<{ pick: [model: ModelInfo]; add: [] }>()

const query = ref('')
const settings = computed(() => models.settings)

const q = computed(() => query.value.trim().toLowerCase())
const matches = (s: string): boolean => !q.value || s.toLowerCase().includes(q.value)

const aliases = computed(() =>
  props.aliases
    ? (settings.value?.aliases ?? []).filter((m) => matches(m.name) || matches(m.target?.model ?? ''))
    : [],
)

/** Nothing to pick from yet: only the way to add a provider. */
const empty = computed(
  () => !settings.value?.providers.length && !(props.aliases && settings.value?.aliases.length),
)

/** Pinned models first, then each provider's unpinned ones. */
const groups = computed(() => {
  const providers = settings.value?.providers ?? []
  const list: { key: string; name: string; models: ModelInfo[] }[] = []
  const pinned = providers
    .flatMap((p) => p.models)
    .filter((m) => m.pinned && (matches(m.name) || matches(m.ref)))
  if (pinned.length) list.push({ key: 'pinned', name: t('settings.pinned'), models: pinned })
  for (const p of providers) {
    const models = p.models.filter((m) => !m.pinned && (matches(m.name) || matches(m.ref)))
    if (models.length) list.push({ key: p.id, name: p.name, models })
  }
  return list
})

/** Folds the user flipped, by provider id; unflipped, a provider is open
 *  iff it holds the selected model. A search opens every match. */
const folds = ref<Record<string, boolean>>({})
function isOpen(g: { key: string; models: ModelInfo[] }): boolean {
  if (q.value || g.key === 'pinned') return true
  return folds.value[g.key] ?? g.models.some((m) => m.ref === props.selected)
}
function toggleGroup(g: { key: string; models: ModelInfo[] }): void {
  folds.value = { ...folds.value, [g.key]: !isOpen(g) }
}

async function togglePin(m: ModelInfo): Promise<void> {
  await editSettings('settings.pin', { model: m.ref, pinned: !m.pinned })
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <div v-if="!empty" class="relative">
      <Icon
        name="search"
        :size="13"
        class="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-ink-3"
      />
      <input
        v-model="query"
        autofocus
        type="search"
        class="w-full py-1.5 pl-8 text-secondary"
        :placeholder="t('settings.searchModels')"
        autocomplete="off"
        spellcheck="false"
      />
    </div>

    <div class="flex flex-col gap-0.5">
      <slot v-if="!q && !empty" name="lead" />
      <template v-if="aliases.length">
        <div class="panel-head">{{ t('settings.aliases') }}</div>
        <button
          v-for="m in aliases"
          :key="m.ref"
          type="button"
          class="flex items-center gap-2 rounded-control px-2.5 py-1.5 text-left transition-colors duration-150 hover:bg-surface-2"
          :aria-pressed="selected === m.ref"
          @click="emit('pick', m)"
        >
          <Icon
            name="ok"
            :size="13"
            class="flex-none text-brand"
            :class="selected === m.ref ? '' : 'invisible'"
          />
          <span
            class="flex-none text-secondary"
            :class="selected === m.ref ? 'font-semibold text-ink' : 'text-ink-2'"
            >{{ m.name }}</span
          >
          <span class="min-w-0 truncate text-micro text-ink-3" :title="m.target?.model">{{
            modelByRef(m.target?.model ?? '')?.name ?? m.target?.model
          }}</span>
        </button>
      </template>

      <template v-for="group in groups" :key="group.key">
        <div v-if="group.key === 'pinned'" class="panel-head mt-1">{{ group.name }}</div>
        <button
          v-else
          type="button"
          class="panel-head mt-1 mb-0 items-center rounded-control py-1 pl-1.5 text-left transition-colors hover:bg-surface-2"
          :aria-expanded="isOpen(group)"
          :disabled="!!q"
          @click="toggleGroup(group)"
        >
          <Icon
            name="chevron"
            :size="12"
            class="flex-none text-ink-3 transition-transform duration-150"
            :class="{ 'rotate-90': isOpen(group) }"
          />
          <span class="min-w-0 truncate">{{ group.name }}</span>
          <small>{{ group.models.length }}</small>
        </button>
        <div
          v-for="m in isOpen(group) ? group.models : []"
          :key="m.ref"
          class="group flex items-center gap-1 rounded-control transition-colors duration-150 hover:bg-surface-2"
        >
          <button
            type="button"
            class="flex min-w-0 flex-1 items-center gap-2 rounded-control px-2.5 py-1.5 text-left"
            :aria-pressed="selected === m.ref"
            @click="emit('pick', m)"
          >
            <Icon
              name="ok"
              :size="13"
              class="flex-none text-brand"
              :class="selected === m.ref ? '' : 'invisible'"
            />
            <span
              class="min-w-0 truncate text-secondary"
              :class="selected === m.ref ? 'font-semibold text-ink' : 'text-ink-2'"
              >{{ m.name }}</span
            >
          </button>
          <button
            type="button"
            class="icon-btn mr-1.5 size-5"
            :class="m.pinned ? '' : 'opacity-0 group-hover:opacity-100 focus-visible:opacity-100'"
            :title="m.pinned ? t('settings.unpin') : t('settings.pin')"
            :aria-label="m.pinned ? t('settings.unpin') : t('settings.pin')"
            :aria-pressed="m.pinned"
            @click.stop="togglePin(m)"
          >
            <Icon name="pin" :size="12" :class="m.pinned ? 'text-ink' : ''" />
          </button>
        </div>
      </template>

      <NoProviders v-if="empty" compact @add="emit('add')" />
    </div>
  </div>
</template>
