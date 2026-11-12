<script setup lang="ts">
// The assistants page: the assistants as a grid of cards, the default first.
// One opens at `/assistants/<id>` (`?new` for a new one) in the sheet's whole
// width, with the header as the trail back.
import { computed, watch } from 'vue'
import { t } from '../../i18n'
import { SWAP } from '../../lib/motion'
import { openPage, pageUp } from '../../lib/router'
import { connected, refreshAssistants, state } from '../../lib/store'
import RumuCat from '../brand/RumuCat.vue'
import Icon from '../ui/Icon.vue'
import AssistantCard from './AssistantCard.vue'
import AssistantForm from './AssistantForm.vue'
import PageSheet from './PageSheet.vue'

const route = computed(() => state.page!)
const assistants = computed(() => [...state.assistants].sort((a, b) => Number(b.default) - Number(a.default)))

/** The open item's crumb, or none on the grid (or for a stale id). */
const crumb = computed(() => {
  const r = route.value
  if (r.fresh) return t('assistants.new')
  return state.assistants.find((a) => a.id === r.item)?.name ?? null
})

/** Booted at an item before the roster loads: neither the item nor the grid yet. */
const loading = computed(() => !!route.value.item && !state.assistants.length)

// A page booted from its address mounts before the socket is up.
watch(connected, (up) => up && void refreshAssistants(), { immediate: true })
</script>

<template>
  <PageSheet :title="t('assistants.title')" :count="crumb ? undefined : assistants.length">
    <template #icon>
      <RumuCat :size="18" :paper="false" class="flex-none" />
    </template>
    <template #title>
      <template v-if="crumb">
        <button type="button" class="flex-none text-ink-3 transition-colors hover:text-ink" @click="pageUp">
          {{ t('assistants.title') }}
        </button>
        <Icon name="chevron" :size="14" class="-mx-1 flex-none text-ink-3" />
        <span class="min-w-0 truncate">{{ crumb }}</span>
      </template>
      <span v-else class="truncate">{{ t('assistants.title') }}</span>
    </template>

    <Transition v-bind="SWAP" mode="out-in">
      <div v-if="crumb" :key="`${route.item}/${route.fresh}`" class="flex min-h-0 flex-1 flex-col">
        <AssistantForm />
      </div>

      <div v-else-if="!loading" class="min-h-0 flex-1 overflow-y-auto px-4 pt-5 pb-8 sm:px-6">
        <div class="stagger grid grid-cols-[repeat(auto-fill,minmax(15rem,1fr))] gap-4">
          <AssistantCard
            v-for="(a, i) in assistants"
            :key="a.id"
            :assistant="a"
            :style="{ '--i': i }"
            @open="openPage('assistants', a.id)"
          />
          <button
            type="button"
            :style="{ '--i': assistants.length }"
            class="flex min-h-46 items-center justify-center gap-2 rounded-card border border-dashed border-line text-secondary text-ink-3 transition-colors duration-200 hover:border-ink-3 hover:bg-surface-2/40 hover:text-ink"
            @click="openPage('assistants', null, true)"
          >
            <Icon name="plus" :size="15" />
            {{ t('assistants.new') }}
          </button>
        </div>
      </div>
    </Transition>
  </PageSheet>
</template>
