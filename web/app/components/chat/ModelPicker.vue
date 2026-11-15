<script setup lang="ts">
// The composer's model chip: the model the next prompt runs on and its
// reasoning effort, picked from the ModelMenu. Its first row follows the
// assistant's model; the effort bar at the foot sets the effort on the shown
// model, which pins that model.
import { computed } from 'vue'
import { t } from '../../i18n'
import { openPage } from '../../lib/router'
import { assistantModel, currentPick, modelByRef, models, pick, resolvePick } from '../../lib/state/models'
import type { Effort } from '../../lib/generated/Effort'
import type { ModelInfo } from '../../lib/generated/ModelInfo'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'
import ModelMenu from './ModelMenu.vue'

const use = computed(() => currentPick())
const resolved = computed(() => resolvePick(use.value))
const settings = computed(() => models.settings)
const empty = computed(() => !settings.value?.aliases.length && !settings.value?.providers.length)
const own = computed(() => assistantModel())
const ownLabel = computed(() => {
  const m = own.value
  if (!m) return ''
  const r = resolvePick(m)
  return [modelByRef(m.model)?.name ?? m.model, r?.effort].filter(Boolean).join(' · ')
})
const efforts = computed<Effort[]>(() => resolved.value?.model?.efforts ?? [])

function choose(m: ModelInfo, close: () => void): void {
  // Moving to an alias takes its effort; another keeps the shown one.
  const effort = m.target ? undefined : (resolved.value?.effort ?? undefined)
  pick(effort ? { model: m.ref, effort } : { model: m.ref })
  close()
}

function follow(close: () => void): void {
  pick(null)
  close()
}

function setEffort(e: Effort): void {
  const ref = resolved.value?.modelRef
  if (ref) pick({ model: ref, effort: e })
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
        <Icon name="model" :size="14" class="flex-none text-ink-3" />
        <span class="truncate">{{ resolved?.label }}</span>
        <span v-if="efforts.length > 1" class="flex-none text-ink-3">· {{ resolved?.effort }}</span>
        <Icon name="down" :size="12" class="flex-none text-ink-3" />
      </button>
    </template>
    <template #default="{ close }">
      <div class="flex flex-col gap-2">
        <ModelMenu :selected="use?.model ?? null" @pick="choose($event, close)" @add="close">
          <template v-if="own" #lead>
            <button
              type="button"
              class="flex items-center gap-2 rounded-control px-2.5 py-1.5 text-left transition-colors duration-150 hover:bg-surface-2"
              :aria-pressed="!use"
              @click="follow(close)"
            >
              <Icon name="ok" :size="13" class="flex-none text-brand" :class="use ? 'invisible' : ''" />
              <span class="flex-none text-secondary" :class="use ? 'text-ink-2' : 'font-semibold text-ink'">{{
                t('settings.assistantModel')
              }}</span>
              <span class="min-w-0 truncate text-micro text-ink-3">{{ ownLabel }}</span>
            </button>
            <div class="mx-1 my-1 border-t border-line-soft" />
          </template>
        </ModelMenu>
        <button
          v-if="!empty"
          type="button"
          class="flex items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-meta text-ink-3 transition-colors hover:bg-surface-2 hover:text-ink"
          @click="configure(close)"
        >
          <Icon name="settings" :size="13" />
          {{ t('settings.editModels') }}
        </button>
        <div
          v-if="efforts.length > 1"
          class="sticky -bottom-3 -mx-3 -mb-3 flex items-center gap-2.5 border-t border-line-soft bg-surface px-3 pt-2 pb-3"
        >
          <span class="flex flex-none items-center gap-1 text-meta font-semibold text-ink-3">
            <Icon name="brain" :size="13" />
            {{ t('settings.effort') }}
          </span>
          <div
            class="flex flex-1 rounded-control bg-surface-2 p-0.5"
            role="radiogroup"
            :aria-label="t('settings.effort')"
          >
            <button
              v-for="e in efforts"
              :key="e"
              type="button"
              role="radio"
              :aria-checked="resolved?.effort === e"
              class="flex-1 rounded-xs px-1.5 py-0.5 text-meta font-semibold transition-colors duration-150"
              :class="
                resolved?.effort === e ? 'bg-surface text-ink shadow-soft' : 'text-ink-3 hover:text-ink'
              "
              @click="setEffort(e)"
            >
              {{ e }}
            </button>
          </div>
        </div>
      </div>
    </template>
  </Popover>
</template>
