<script setup lang="ts">
// A thin dialog shell around the shared card pane: it resolves the live card,
// offers the "as seen" snapshot, and owns the native <dialog> lifecycle.
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { t } from '../../i18n'
import {
  cardById,
  cardKey,
  cardLabel,
  cardUsesOnPath,
  mountedRef,
  parseCardView,
  useIsCurrent,
} from '../../lib/cards'
import type { CardUse, CardView } from '../../lib/cards'
import { closeDrawer, state } from '../../lib/store'
import { renderMarkdown } from '../../lib/markdown'
import CardFace from '../CardFace.vue'
import CardEditor from '../library/CardEditor.vue'
import Icon from '../ui/Icon.vue'

const dialog = ref<HTMLDialogElement | null>(null)
const pane = ref<InstanceType<typeof CardEditor> | null>(null)
// Keep the card on the page until the native dialog finishes fading out.
const drawer = ref(state.drawer)
const path = computed(() => drawer.value?.cardId ?? '')
const card = computed(() => cardById.value.get(path.value) ?? null)
const selectedUse = computed<CardUse | null>(() => {
  const refId = drawer.value?.useRef
  if (!refId) return null // a library open: always the live card
  const uses = cardUsesOnPath.value.get(card.value ? cardKey(card.value) : path.value) ?? []
  return uses.find((u) => u.callId === refId || u.node.id === refId) ?? null
})
const changed = computed(
  () => !!card.value && !!selectedUse.value && !useIsCurrent(card.value, selectedUse.value),
)
const tab = ref<'snapshot' | 'current'>('current')
/** Remounts the pane on each open: fresh load, read mode and scroll. */
const openKey = ref(0)
watch(
  () => state.drawer,
  (d) => {
    if (!d) return
    drawer.value = d
    openKey.value++
    // A stale use opens on the as-seen version; one still current does not.
    tab.value = d.useRef && changed.value ? 'snapshot' : 'current'
  },
  { immediate: true },
)

const snapshot = computed(() => !card.value || (changed.value && tab.value === 'snapshot'))
const EMPTY_VIEW: CardView = { description: null, keywords: [], meta: {}, body: '' }
const view = computed<CardView>(() => {
  if (snapshot.value) {
    const use = selectedUse.value
    return use ? parseCardView(use.snapshot, use.kind === 'import') : EMPTY_VIEW
  }
  const c = card.value
  return c
    ? {
        description: c.description ?? null,
        keywords: c.keywords ?? [],
        meta: c.meta ?? {},
        body: c.body,
      }
    : EMPTY_VIEW
})
const title = computed(() => path.value.split('/').at(-1) || path.value)
/** The card's directory, without its deck's mount prefix. */
const dir = computed(() => {
  const i = path.value.lastIndexOf('/')
  return i < 0 ? '' : cardLabel(path.value.slice(0, i))
})
const scope = computed(() => {
  const value = view.value.meta.scope
  return typeof value === 'string'
    ? [value]
    : Array.isArray(value)
      ? value.filter((v) => typeof v === 'string')
      : []
})
const revision = computed(() => (snapshot.value ? selectedUse.value?.rev : card.value?.revision))
/** Relative links resolve beside the shown body: a link card shows its target's. */
const linkBase = computed(() => {
  const target = card.value?.link?.split(':')
  return target?.length === 2 ? { deck: target[0]!, path: target[1]! } : (mountedRef(path.value) ?? undefined)
})
const shownHtml = computed(() => renderMarkdown(view.value.body, { breaks: false, base: linkBase.value }))
/** 详情 rows: the card's address, the version, then its other frontmatter. */
const detailRows = computed<[string, string][]>(() => {
  const rows: [string, string][] = [[t('cards.drawer.path'), path.value]]
  if (revision.value) rows.push([t('cards.drawer.revision'), revision.value])
  for (const [key, value] of Object.entries(view.value.meta)) {
    if (['description', 'keywords', 'scope'].includes(key)) continue
    rows.push([key, typeof value === 'string' ? value : JSON.stringify(value, null, 2)])
  }
  return rows
})

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

/** Close the dialog, asking the pane first when it has unsaved work. */
function close(): void {
  if (pane.value?.confirmLeave()) closeDrawer()
}

function onCancel(e: Event): void {
  // The pane's own Escape (leaving edit / history) is preventDefault-ed on
  // keydown, which suppresses this event; what arrives here is a real close.
  e.preventDefault()
  close()
}

function onBackdrop(e: MouseEvent): void {
  const rect = dialog.value?.getBoundingClientRect()
  if (
    e.target === dialog.value &&
    rect &&
    (e.clientX < rect.left || e.clientX > rect.right || e.clientY < rect.top || e.clientY > rect.bottom)
  ) {
    close()
  }
}

/** Keep showing the saved card after an in-drawer rename or move. */
function onMoved(deck: string, cardPath: string): void {
  const live = state.cards.find((c) => c.deck === deck && c.path === cardPath)
  if (live) {
    if (state.drawer) state.drawer.cardId = live.id
  } else {
    closeDrawer()
  }
}
</script>

<template>
  <Teleport to="body">
    <dialog
      id="card-drawer"
      ref="dialog"
      class="card-reader"
      :aria-label="t('cards.drawer.label')"
      @cancel.prevent="onCancel"
      @click="onBackdrop"
    >
      <CardEditor
        v-if="card"
        :key="openKey"
        ref="pane"
        :deck="card.deck"
        :path="card.path"
        :in-library="false"
        :override="snapshot ? view : null"
        overlay="#card-drawer"
        @close="closeDrawer"
        @moved="onMoved"
      >
        <template #trailing>
          <button
            type="button"
            class="icon-btn size-8"
            :title="t('cards.drawer.close')"
            :aria-label="t('cards.drawer.closeAria')"
            @click="close"
          >
            <Icon name="close" :size="16" />
          </button>
        </template>
        <template #meta>
          <div v-if="scope.length" class="mt-3 flex flex-wrap gap-x-2 gap-y-1 text-meta text-ink-2">
            <span>{{ t('cards.drawer.scope') }}</span>
            <span v-for="(entry, i) in scope" :key="i" class="font-mono">{{ entry }}</span>
          </div>
        </template>
        <template #notice>
          <div v-if="changed" class="mt-4 flex flex-wrap items-center gap-2 text-meta text-ink-3">
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
        </template>
        <template #details>
          <template v-for="[label, value] in detailRows" :key="label">
            <dt class="text-ink-3">{{ label }}</dt>
            <dd class="whitespace-pre-wrap">{{ value }}</dd>
          </template>
        </template>
      </CardEditor>

      <template v-else>
        <button
          class="icon-btn absolute top-3 right-3"
          :title="t('cards.drawer.close')"
          :aria-label="t('cards.drawer.closeAria')"
          @click="closeDrawer"
        >
          <Icon name="close" :size="16" />
        </button>
        <article class="reader-content scroll-gutter min-h-0 overflow-auto p-6 sm:px-8 sm:pb-7">
          <CardFace
            :dir="dir"
            :title="title"
            :description="view.description"
            :keywords="view.keywords"
            :html="shownHtml"
          >
            <template #meta>
              <div v-if="scope.length" class="mt-3 flex flex-wrap gap-x-2 gap-y-1 text-meta text-ink-2">
                <span>{{ t('cards.drawer.scope') }}</span>
                <span v-for="(entry, i) in scope" :key="i" class="font-mono">{{ entry }}</span>
              </div>
            </template>
            <template #notice>
              <p class="mt-4 text-meta text-ink-3">
                {{ t(selectedUse ? 'cards.drawer.deletedSeen' : 'cards.drawer.deleted') }}
              </p>
            </template>
            <template #details>
              <template v-for="[label, value] in detailRows" :key="label">
                <dt class="text-ink-3">{{ label }}</dt>
                <dd class="whitespace-pre-wrap">{{ value }}</dd>
              </template>
            </template>
          </CardFace>
        </article>
      </template>
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
