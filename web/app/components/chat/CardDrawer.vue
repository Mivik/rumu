<script setup lang="ts">
// A content-sized reader. Historical text never borrows live metadata.
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { t } from '../../i18n'
import { cardById, cardUsesOnPath, parseCardView, useIsCurrent } from '../../lib/cards'
import type { CardUse, CardView } from '../../lib/cards'
import { closeDrawer, state } from '../../lib/store'
import { renderMarkdown } from '../../lib/markdown'
import Icon from '../ui/Icon.vue'

const dialog = ref<HTMLDialogElement | null>(null)
const reader = ref<HTMLElement | null>(null)
// Keep the card on the page until the native dialog finishes fading out.
const drawer = ref(state.drawer)
const path = computed(() => drawer.value?.cardId ?? '')
const card = computed(() => cardById.value.get(path.value) ?? null)
const selectedUse = computed<CardUse | null>(() => {
  const refId = drawer.value?.useRef
  if (!refId) return null // a library open: always the live card
  const uses = cardUsesOnPath.value.get(path.value) ?? []
  return uses.find((u) => u.callId === refId || u.node.id === refId) ?? null
})
const changed = computed(
  () => !!card.value && !!selectedUse.value && !useIsCurrent(card.value, selectedUse.value),
)
const tab = ref<'snapshot' | 'current'>('current')
watch(
  () => state.drawer,
  (d) => {
    if (!d) return
    drawer.value = d
    tab.value = d.useRef ? 'snapshot' : 'current'
    reader.value?.scrollTo(0, 0)
  },
  { immediate: true },
)
const snapshot = computed(() => !card.value || (changed.value && tab.value === 'snapshot'))
const EMPTY_VIEW: CardView = { title: null, description: null, keywords: [], meta: {}, body: '' }
const view = computed<CardView>(() => {
  if (snapshot.value) {
    const use = selectedUse.value
    return use ? parseCardView(use.snapshot, use.kind === 'import') : EMPTY_VIEW
  }
  const c = card.value
  return c
    ? {
        title: c.title ?? null,
        description: c.description ?? null,
        keywords: c.keywords ?? [],
        meta: c.meta ?? {},
        body: c.body,
      }
    : EMPTY_VIEW
})
const title = computed(() => view.value.title || path.value.split('/').at(-1) || path.value)
const description = computed(() => view.value.description)
const keywords = computed(() => view.value.keywords)
const meta = computed(() => view.value.meta)
const scope = computed(() => {
  const value = meta.value.scope
  return typeof value === 'string'
    ? [value]
    : Array.isArray(value)
      ? value.filter((v) => typeof v === 'string')
      : []
})
const fields = computed(() =>
  Object.entries(meta.value).filter(([key]) => !['title', 'description', 'keywords', 'scope'].includes(key)),
)
const revision = computed(() => (snapshot.value ? selectedUse.value?.rev : card.value?.revision))
const shownHtml = computed(() => renderMarkdown(view.value.body, { breaks: false, cardId: path.value }))
const formatValue = (value: unknown): string =>
  typeof value === 'string' ? value : JSON.stringify(value, null, 2)

watch(
  () => !!state.drawer,
  async () => {
    await nextTick()
    if (state.drawer && !dialog.value?.open) dialog.value?.showModal()
    else if (!state.drawer) dialog.value?.close()
  },
  { immediate: true, flush: 'post' },
)
onBeforeUnmount(() => dialog.value?.close())

function onBackdrop(e: MouseEvent): void {
  const rect = dialog.value?.getBoundingClientRect()
  if (
    e.target === dialog.value &&
    rect &&
    (e.clientX < rect.left || e.clientX > rect.right || e.clientY < rect.top || e.clientY > rect.bottom)
  ) {
    closeDrawer()
  }
}
</script>

<template>
  <Teleport to="body">
    <dialog
      ref="dialog"
      class="card-reader"
      :aria-label="t('cards.drawer.label')"
      @cancel.prevent="closeDrawer"
      @click="onBackdrop"
    >
      <button
        class="icon-btn absolute top-3 right-3"
        autofocus
        :title="t('cards.drawer.close')"
        :aria-label="t('cards.drawer.closeAria')"
        @click="closeDrawer"
      >
        <Icon name="close" :size="16" />
      </button>
      <article ref="reader" :key="path" class="reader-content min-h-0 overflow-auto p-6 sm:px-8 sm:pb-7">
        <header class="pb-1">
          <div class="pr-6 font-card">
            <p v-if="path.includes('/')" class="break-anywhere text-meta leading-none text-ink-3">
              {{ path.slice(0, path.lastIndexOf('/') + 1) }}
            </p>
            <h1 class="text-[24px] leading-tight font-semibold sm:text-[28px]">{{ title }}</h1>
          </div>
          <p v-if="description" class="mt-3 text-[14px] leading-relaxed text-ink-2">{{ description }}</p>
          <div v-if="scope.length" class="mt-3 flex flex-wrap gap-x-2 gap-y-1 text-meta text-ink-2">
            <span>{{ t('cards.drawer.scope') }}</span>
            <span v-for="(entry, i) in scope" :key="i" class="font-mono">{{ entry }}</span>
          </div>
        </header>
        <p v-if="!card" class="mt-4 text-meta text-ink-3">
          {{ t(selectedUse ? 'cards.drawer.deletedSeen' : 'cards.drawer.deleted') }}
        </p>
        <div v-else-if="changed" class="mt-4 flex flex-wrap items-center gap-2 text-meta text-ink-3">
          <span>{{ t('cards.drawer.olderSeen') }}</span>
          <button
            v-for="version in ['snapshot', 'current'] as const"
            :key="version"
            class="rounded-control px-2 py-1"
            :class="tab === version ? 'bg-surface-2 text-ink' : ''"
            :aria-pressed="tab === version"
            @click="tab = version"
          >
            {{ t(version === 'snapshot' ? 'cards.drawer.asSeen' : 'cards.drawer.current') }}
          </button>
        </div>
        <div class="md mt-5 text-ink" v-html="shownHtml" />
        <div v-if="keywords.length" class="mt-5 flex flex-wrap gap-1.5">
          <span v-for="(keyword, i) in keywords" :key="i" class="keyword-tag">{{ keyword }}</span>
        </div>
        <details :key="`${path}:${snapshot}`" class="mt-5 text-meta text-ink-2">
          <summary class="w-fit cursor-pointer">{{ t('cards.drawer.details') }}</summary>
          <dl class="mt-4 grid grid-cols-[minmax(0,80px)_minmax(0,1fr)] gap-x-4 gap-y-3 leading-relaxed">
            <dt class="text-ink-3">{{ t('cards.drawer.path') }}</dt>
            <dd>{{ path }}</dd>
            <template v-if="revision">
              <dt class="text-ink-3">{{ t('cards.drawer.revision') }}</dt>
              <dd>{{ revision }}</dd>
            </template>
            <template v-for="[key, value] in fields" :key="key">
              <dt class="text-ink-3">{{ key }}</dt>
              <dd class="whitespace-pre-wrap">{{ formatValue(value) }}</dd>
            </template>
          </dl>
        </details>
      </article>
    </dialog>
  </Teleport>
</template>

<style scoped>
.card-reader {
  position: fixed;
  inset: 0;
  width: min(680px, calc(100vw - 32px));
  max-width: none;
  max-height: min(85dvh, calc(100dvh - 32px - env(safe-area-inset-top) - env(safe-area-inset-bottom)));
  margin: auto;
  padding: 0;
  overflow: hidden;
  border: 0;
  border-radius: var(--radius-card);
  background: var(--surface);
  color: var(--ink);
  box-shadow: var(--shadow-pop-v);
}
.card-reader {
  opacity: 0;
  translate: 0 8px;
  transition:
    opacity 150ms var(--ease-soft),
    translate 150ms var(--ease-soft),
    display 150ms allow-discrete,
    overlay 150ms allow-discrete;
}
.card-reader[open] {
  display: flex;
  flex-direction: column;
  opacity: 1;
  translate: 0 0;
  transition-duration: 300ms;
}
.card-reader::backdrop {
  background: var(--scrim);
  opacity: 0;
  transition:
    opacity 200ms ease-out,
    display 200ms allow-discrete,
    overlay 200ms allow-discrete;
}
.card-reader[open]::backdrop {
  opacity: 1;
  transition-duration: 300ms;
}
.reader-content {
  overscroll-behavior: contain;
  overflow-wrap: anywhere;
}
.reader-content :deep(.md) {
  line-height: 1.75;
}
@starting-style {
  .card-reader[open] {
    opacity: 0;
    translate: 0 16px;
  }
  .card-reader[open]::backdrop {
    opacity: 0;
  }
}
@media (prefers-reduced-motion: reduce) {
  .card-reader,
  .card-reader[open],
  .card-reader::backdrop,
  .card-reader[open]::backdrop {
    transition: none;
  }
}
</style>
