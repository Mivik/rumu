<script setup lang="ts">
import { t } from '../i18n'
import { dismissToast, state } from '../lib/store'
import type { ToastKind } from '../lib/types'
import { applyUpdate, update } from '../lib/update'
import Icon from './ui/Icon.vue'

// Literal lookup (Tailwind only emits complete class names).
const kindDot: Record<ToastKind, string> = {
  error: 'bg-danger',
  info: 'bg-pine',
}
</script>

<template>
  <TransitionGroup
    tag="div"
    class="fixed right-4 bottom-4 left-4 z-80 flex max-w-none flex-col gap-2 sm:left-auto sm:max-w-[380px]"
    enter-from-class="translate-y-3 scale-95 opacity-0"
    enter-active-class="transition duration-300 ease-spring"
    leave-to-class="translate-x-6 opacity-0"
    leave-active-class="transition duration-200 ease-in"
    move-class="transition-transform duration-300 ease-soft"
  >
    <div
      v-if="update.ready && !update.dismissed"
      key="update"
      class="flex items-center gap-2.5 rounded-card border border-line-soft bg-surface py-2.5 pr-3 pl-4 text-meta text-ink shadow-pop"
      role="status"
    >
      <span class="size-2 flex-none rounded-full bg-pine" />
      <span class="min-w-0 flex-1">{{ t('common.update.ready') }}</span>
      <button
        class="flex-none rounded-xs border border-pine/40 px-2 py-0.5 text-micro font-semibold text-pine transition-colors hover:bg-pine/10"
        @click="applyUpdate"
      >
        {{ t('common.update.reload') }}
      </button>
      <button
        class="flex-none text-ink-3 transition-colors hover:text-ink"
        :aria-label="t('common.dismiss')"
        @click="update.dismissed = true"
      >
        <Icon name="close" :size="12" />
      </button>
    </div>
    <div
      v-for="toast in state.toasts"
      :key="toast.id"
      class="flex items-start gap-2.5 rounded-card border border-line-soft bg-surface px-4 py-3 text-meta text-ink shadow-pop"
      role="alert"
    >
      <span class="mt-1.5 size-2 flex-none rounded-full" :class="kindDot[toast.kind]" />
      <span class="min-w-0 flex-1">
        {{ toast.message }}
        <span v-if="toast.detail" class="mt-0.5 block font-mono text-micro break-words text-ink-3">{{
          toast.detail
        }}</span>
      </span>
      <button
        class="mt-0.5 flex-none text-ink-3 transition-colors hover:text-ink"
        :aria-label="t('common.dismiss')"
        @click="dismissToast(toast.id)"
      >
        <Icon name="close" :size="12" />
      </button>
    </div>
  </TransitionGroup>
</template>
