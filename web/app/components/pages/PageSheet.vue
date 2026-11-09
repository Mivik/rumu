<script setup lang="ts">
// A management page on the chat's sheet: a header naming it, and below it the
// page's list beside the item open on it. The sheet is a size container: when
// it is narrow (a phone, a slim window) one pane shows at a time, and with an
// item open the header's menu button turns into a back arrow to the list. A
// page without a list is one pane at every width.
import { useSlots } from 'vue'
import { t } from '../../i18n'
import { layout, toggleSidebar } from '../../lib/layout'
import RumuCat from '../brand/RumuCat.vue'
import Icon from '../ui/Icon.vue'

defineProps<{
  title: string
  count?: number | string
  icon?: string
  /** The icon's tone (a text color class). */
  tone?: string
  /** An item is open: its pane shows, and the list hides when narrow. */
  detail?: boolean
  /** The wide sheet's word in the item pane while none is open. */
  pick?: string
}>()

const emit = defineEmits<{ back: [] }>()
const split = !!useSlots().list
</script>

<template>
  <section
    class="sheet @container m-2 flex min-h-0 min-w-0 flex-col overflow-hidden rounded-sheet bg-surface sm:m-3 lg:mx-0 lg:my-4"
    :aria-label="title"
  >
    <header class="flex min-h-14 flex-none items-center gap-2 border-b border-line-soft px-4 sm:px-6">
      <button
        v-if="split && detail"
        type="button"
        class="icon-btn -ml-1.5 size-8 flex-none @2xl:hidden"
        :title="t('common.back')"
        :aria-label="t('common.back')"
        @click="emit('back')"
      >
        <Icon name="back" :size="16" />
      </button>
      <!-- the sessions sidebar is a drawer below lg -->
      <button
        type="button"
        class="icon-btn -ml-1.5 size-8 flex-none lg:hidden"
        :class="{ '@max-2xl:hidden': split && detail }"
        :title="t('chat.header.sessions')"
        :aria-label="t('chat.header.openSessions')"
        :aria-expanded="layout.sidebarOpen"
        @click="toggleSidebar"
      >
        <Icon name="menu" :size="16" />
      </button>
      <h1 class="flex min-w-0 items-center gap-2 text-title font-bold">
        <slot name="icon">
          <Icon v-if="icon" :name="icon" :size="16" class="flex-none" :class="tone" />
        </slot>
        <slot name="title">
          <span class="truncate">{{ title }}</span>
        </slot>
        <span
          v-if="count !== undefined"
          class="flex-none rounded-full bg-surface-2 px-2 py-px text-micro font-semibold text-ink-3 tabular-nums"
          >{{ count }}</span
        >
      </h1>
      <div class="ml-auto flex min-w-0 items-center gap-2">
        <slot name="actions" />
      </div>
    </header>

    <div v-if="split" class="grid min-h-0 flex-1 grid-cols-1 @2xl:grid-cols-[18rem_minmax(0,1fr)]">
      <div
        class="flex min-h-0 flex-col bg-canvas/40 @2xl:border-r @2xl:border-line-soft"
        :class="{ '@max-2xl:hidden': detail }"
      >
        <slot name="list" />
      </div>
      <div class="flex min-h-0 min-w-0 flex-col" :class="{ '@max-2xl:hidden': !detail }">
        <slot v-if="detail" />
        <div v-else class="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
          <RumuCat :size="56" :paper="false" class="opacity-90" />
          <p class="max-w-56 text-secondary text-ink-3">{{ pick }}</p>
        </div>
      </div>
    </div>
    <div v-else class="flex min-h-0 min-w-0 flex-1 flex-col">
      <slot />
    </div>
  </section>
</template>
