<script setup lang="ts">
// The assistants page: the assistants as a grid of cards, the default first.
// One opens at `/assistants/<id>` (`?new` for a new one): at the desktop
// breakpoint as a dialog over the grid, narrower in the sheet's whole width
// with the header as the trail back.
import { computed, watch } from 'vue'
import { t } from '../../i18n'
import { layout, leaveItem } from '../../lib/layout'
import { SWAP } from '../../lib/motion'
import { openPage } from '../../lib/router'
import { connected, refreshAssistants, state } from '../../lib/store'
import RumuCat from '../brand/RumuCat.vue'
import Icon from '../ui/Icon.vue'
import Modal from '../ui/Modal.vue'
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
/** The open item is a dialog over the grid rather than the sheet's body. */
const dialog = computed(() => layout.wide && !!crumb.value)
/** The item takes the sheet's body. */
const inSheet = computed(() => !layout.wide && !!crumb.value)

/** Booted at an item before the roster loads: neither the item nor the grid yet. */
const loading = computed(() => !!route.value.item && !state.assistants.length)

// A page booted from its address mounts before the socket is up.
watch(connected, (up) => up && void refreshAssistants(), { immediate: true })
</script>

<template>
  <PageSheet :title="t('assistants.title')" :count="inSheet ? undefined : assistants.length">
    <template #icon>
      <RumuCat :size="18" :paper="false" class="flex-none" />
    </template>
    <template #title>
      <template v-if="inSheet">
        <button
          type="button"
          class="flex-none text-ink-3 transition-colors hover:text-ink"
          @click="leaveItem"
        >
          {{ t('assistants.title') }}
        </button>
        <Icon name="chevron" :size="14" class="-mx-1 flex-none text-ink-3" />
        <span class="min-w-0 truncate">{{ crumb }}</span>
      </template>
      <span v-else class="truncate">{{ t('assistants.title') }}</span>
    </template>

    <Transition v-bind="SWAP" mode="out-in">
      <div v-if="inSheet" :key="`${route.item}/${route.fresh}`" class="flex min-h-0 flex-1 flex-col">
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

    <Modal
      :open="dialog"
      :label="crumb ?? ''"
      :z="55"
      wrapper-class="p-8"
      @close="leaveItem"
      panel-class="relative max-h-full w-[min(46rem,100%)] rounded-sheet"
    >
      <!-- the form's band and name head the dialog: no header above them -->
      <button
        type="button"
        class="icon-btn absolute top-3 right-3 z-10 size-8 rounded-full bg-surface/80 shadow-soft backdrop-blur-sm"
        :title="t('common.close')"
        :aria-label="t('common.close')"
        @click="leaveItem"
      >
        <Icon name="close" :size="16" />
      </button>
      <Transition v-bind="SWAP" mode="out-in">
        <div :key="`${route.item}/${route.fresh}`" class="flex min-h-0 flex-1 flex-col">
          <AssistantForm />
        </div>
      </Transition>
    </Modal>
  </PageSheet>
</template>
