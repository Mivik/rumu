<script setup lang="ts">
// A field-shaped model select over the composer's ModelMenu, by ref: custom
// models too unless left out. A clearable one offers its placeholder as the
// first choice, which empties it.
import { computed } from 'vue'
import { t } from '../../i18n'
import { modelByRef } from '../../lib/state/models'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'
import ModelMenu from '../chat/ModelMenu.vue'

withDefaults(defineProps<{ placeholder?: string; custom?: boolean; clearable?: boolean }>(), {
  custom: true,
})
const model = defineModel<string>({ required: true })
const current = computed(() => (model.value ? (modelByRef(model.value)?.name ?? model.value) : null))

function choose(ref: string, close: () => void): void {
  model.value = ref
  close()
}
</script>

<template>
  <Popover :label="t('settings.model')" placement="bottom-start" wide block>
    <template #trigger="{ open, toggle }">
      <button
        type="button"
        class="select-trigger"
        :class="{ 'is-open': open }"
        :title="model"
        :aria-label="t('settings.model')"
        :aria-expanded="open"
        @click="toggle"
      >
        <span class="min-w-0 flex-1 truncate text-left" :class="current ? 'text-ink' : 'text-ink-3'">
          {{ current ?? placeholder }}
        </span>
        <Icon
          name="down"
          :size="14"
          class="flex-none text-ink-3 transition-transform duration-200"
          :class="open ? 'rotate-180' : ''"
        />
      </button>
    </template>
    <template #default="{ close }">
      <button
        v-if="clearable"
        type="button"
        class="mx-1.5 mt-1.5 flex items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-ink-2 transition-colors duration-150 hover:bg-surface-2"
        :aria-pressed="!model"
        @click="choose('', close)"
      >
        {{ placeholder }}
      </button>
      <ModelMenu :selected="model || null" :custom="custom" @pick="choose($event.ref, close)" @add="close" />
    </template>
  </Popover>
</template>
