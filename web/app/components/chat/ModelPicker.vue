<script setup lang="ts">
// The composer's model chip: the profile or model the next prompt runs on.
// Profiles, pinned models, then each provider's models
// (folded, but for the picked model's provider). The effort is its own chip
// (EffortPicker).
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { openSettings } from '../../lib/layout'
import { currentPick, editSettings, modelByRef, models, pick, resolvePick } from '../../lib/state/models'
import type { ModelInfo } from '../../lib/generated/ModelInfo'
import type { ModelUse } from '../../lib/generated/ModelUse'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'
import NoProviders from '../settings/NoProviders.vue'

const query = ref('')

const use = computed(() => currentPick())
const resolved = computed(() => resolvePick(use.value))
const settings = computed(() => models.settings)

const q = computed(() => query.value.trim().toLowerCase())
const matches = (s: string): boolean => !q.value || s.toLowerCase().includes(q.value)

/** Nothing to pick from yet: only the way to add a provider. */
const empty = computed(() => !settings.value?.profiles.length && !settings.value?.providers.length)

const profiles = computed(() =>
  (settings.value?.profiles ?? []).filter((p) => matches(p.name) || matches(p.model)),
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
 *  iff it holds the picked model. A search opens every match. */
const folds = ref<Record<string, boolean>>({})
function isOpen(g: { key: string; models: ModelInfo[] }): boolean {
  if (q.value || g.key === 'pinned') return true
  return folds.value[g.key] ?? g.models.some((m) => m.ref === selected.value.model)
}
function toggleGroup(g: { key: string; models: ModelInfo[] }): void {
  folds.value = { ...folds.value, [g.key]: !isOpen(g) }
}

/** Selected row: a profile name or a model ref. With no pick, the default
 *  profile. */
const selected = computed(() => {
  const u = use.value
  if (!u) return { profile: settings.value?.roles.default ?? null, model: null }
  return 'profile' in u.target
    ? { profile: u.target.profile, model: null }
    : { profile: null, model: u.target.model }
})

function choose(target: ModelUse['target'], close: () => void): void {
  // Moving to a profile takes its effort; a model keeps the shown one.
  const effort = 'model' in target ? (resolved.value?.effort ?? undefined) : undefined
  pick(effort ? { target, effort } : { target })
  close()
}

async function togglePin(m: ModelInfo): Promise<void> {
  await editSettings('settings.pin', { model: m.ref, pinned: !m.pinned })
}

function configure(close: () => void): void {
  close()
  openSettings()
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
          <template v-if="profiles.length">
            <div class="panel-head">{{ t('settings.profiles') }}</div>
            <button
              v-for="p in profiles"
              :key="p.name"
              type="button"
              class="flex items-center gap-2 rounded-control px-2.5 py-1.5 text-left transition-colors duration-150 hover:bg-surface-2"
              :aria-pressed="selected.profile === p.name"
              @click="choose({ profile: p.name }, close)"
            >
              <Icon
                name="ok"
                :size="13"
                class="flex-none text-brand"
                :class="selected.profile === p.name ? '' : 'invisible'"
              />
              <span
                class="flex-none text-secondary"
                :class="selected.profile === p.name ? 'font-semibold text-ink' : 'text-ink-2'"
                >{{ p.name }}</span
              >
              <span class="min-w-0 truncate text-micro text-ink-3" :title="p.model">{{
                modelByRef(p.model)?.name ?? p.model
              }}</span>
              <span
                v-if="settings.roles.default === p.name"
                class="ml-auto flex-none rounded-xs border border-line px-1 text-micro text-ink-3"
                >{{ t('settings.roleDefault') }}</span
              >
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
                :aria-pressed="selected.model === m.ref"
                @click="choose({ model: m.ref }, close)"
              >
                <Icon
                  name="ok"
                  :size="13"
                  class="flex-none text-brand"
                  :class="selected.model === m.ref ? '' : 'invisible'"
                />
                <span
                  class="min-w-0 truncate text-secondary"
                  :class="selected.model === m.ref ? 'font-semibold text-ink' : 'text-ink-2'"
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

          <NoProviders v-if="empty" compact @add="close" />
        </div>

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
