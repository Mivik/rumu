<script setup lang="ts">
// The models to pick from, as the composer's chip lists them: a search,
// the `lead` slot (unless searching), the aliases (unless left out), pinned
// ones, then each provider's models (folded, but for the selected model's
// provider). Pins toggle in place.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { editSettings, models } from '../../lib/state/models'
import type { ModelInfo } from '../../lib/generated/ModelInfo'
import { searchMatches } from '../../lib/util'
import Icon from '../ui/Icon.vue'
import NoProviders from '../settings/NoProviders.vue'

const props = withDefaults(defineProps<{ selected: string | null; aliases?: boolean }>(), { aliases: true })
const emit = defineEmits<{ pick: [model: ModelInfo]; add: [] }>()

const query = ref('')
const settings = computed(() => models.settings)

const q = computed(() => query.value.trim())
/** A model the search finds by its name, id, ref or provider's name. */
const matches = (m: ModelInfo, provider?: string): boolean =>
  searchMatches(q.value, m.name, m.ref, m.id, provider)

const aliases = computed(() =>
  props.aliases
    ? (settings.value?.aliases ?? []).filter((m) => searchMatches(q.value, m.name, m.target?.model))
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
  const pinned = providers.flatMap((p) => p.models.filter((m) => m.pinned && matches(m, p.name)))
  if (pinned.length) list.push({ key: 'pinned', name: t('settings.pinned'), models: pinned })
  for (const p of providers) {
    const models = p.models.filter((m) => !m.pinned && matches(m, p.name))
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

/** The id beside a model's name, so like-named models tell apart: the
 *  full ref among the pinned (no provider heads them), none when the name
 *  is the id. */
function idOf(m: ModelInfo, pinned: boolean): string {
  const id = pinned ? m.ref : m.id
  return id === m.name ? '' : id
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
          <span class="flex min-w-0 flex-wrap items-baseline gap-x-2">
            <span
              class="min-w-0 text-secondary"
              :class="selected === m.ref ? 'font-semibold text-ink' : 'text-ink-2'"
              >{{ m.name }}</span
            >
            <span class="min-w-0 text-micro wrap-anywhere text-ink-3">{{ m.target?.model }}</span>
          </span>
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
            <span class="flex min-w-0 flex-wrap items-baseline gap-x-2">
              <span
                class="min-w-0 text-secondary"
                :class="selected === m.ref ? 'font-semibold text-ink' : 'text-ink-2'"
                >{{ m.name }}</span
              >
              <span
                v-if="idOf(m, group.key === 'pinned')"
                class="min-w-0 text-micro wrap-anywhere text-ink-3"
                >{{ idOf(m, group.key === 'pinned') }}</span
              >
            </span>
          </button>
          <button
            type="button"
            class="icon-btn relative mr-1.5 size-5 before:absolute before:-inset-1.5"
            :class="
              m.pinned
                ? ''
                : 'pointer-fine:opacity-0 pointer-fine:group-hover:opacity-100 pointer-fine:focus-visible:opacity-100'
            "
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
