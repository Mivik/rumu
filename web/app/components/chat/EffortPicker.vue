<script setup lang="ts">
// The composer's effort chip, beside the send button: the reasoning effort
// the next prompt runs at, out of those the picked model takes. Hidden when
// there is nothing to choose.
import { computed } from 'vue'
import { t } from '../../i18n'
import { currentPick, models, pick, resolvePick } from '../../lib/state/models'
import type { Effort } from '../../lib/generated/Effort'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'

const resolved = computed(() => resolvePick(currentPick()))
const efforts = computed<Effort[]>(() => resolved.value?.model?.efforts ?? [])

/** What the effort applies to: the pick, or with none the default model. */
const target = computed(() => currentPick()?.model ?? models.settings?.roles.default ?? null)

function choose(e: Effort, close: () => void): void {
  if (target.value) pick({ model: target.value, effort: e })
  close()
}
</script>

<template>
  <Popover v-if="efforts.length > 1" menu role="menu" :label="t('settings.effort')" placement="top-end">
    <template #trigger="{ open, toggle }">
      <button
        type="button"
        class="inline-flex flex-none items-center gap-1 rounded-full py-1 pr-1.5 pl-2 text-meta font-semibold text-ink-3 transition-colors hover:bg-surface-2 hover:text-ink"
        :class="{ 'bg-surface-2 text-ink': open }"
        :title="t('settings.effort')"
        aria-haspopup="menu"
        :aria-expanded="open"
        @click="toggle"
      >
        <Icon name="brain" :size="14" />
        <span>{{ resolved?.effort }}</span>
        <Icon name="down" :size="12" />
      </button>
    </template>
    <template #default="{ close }">
      <div class="panel-head mt-1 mb-1 px-2.5">{{ t('settings.effort') }}</div>
      <button
        v-for="e in efforts"
        :key="e"
        type="button"
        role="menuitemradio"
        :aria-checked="resolved?.effort === e"
        class="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-secondary transition-colors duration-150 hover:bg-surface-2"
        :class="resolved?.effort === e ? 'font-semibold text-ink' : 'text-ink-2 hover:text-ink'"
        @click="choose(e, close)"
      >
        <Icon
          name="ok"
          :size="13"
          class="flex-none text-brand"
          :class="resolved?.effort === e ? '' : 'invisible'"
        />
        {{ e }}
      </button>
    </template>
  </Popover>
</template>
