<script setup lang="ts">
// One card window: a native modal dialog over whatever was showing (the chat,
// the library, or another card window). It resolves the live card, offers the
// "as seen" snapshot, and owns the dialog lifecycle. Body links push a new
// window instead of replacing this one, so a followed link leaves this card
// open below it.
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { t } from '../../i18n'
import {
  cardKey,
  cardUsesOnPath,
  fetchCard,
  mountedId,
  mountedRef,
  parseCardView,
  useIsCurrent,
} from '../../lib/cards'
import type { CardUse, CardView } from '../../lib/cards'
import { beginDrawerClose, closeDrawer, state } from '../../lib/store'
import type { CardWindow } from '../../lib/store'
import type { Card } from '../../lib/types'
import { renderMarkdown } from '../../lib/markdown'
import CardFace from '../CardFace.vue'
import CardEditor from '../library/CardEditor.vue'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ entry: CardWindow }>()

const dialog = ref<HTMLDialogElement | null>(null)
const pane = ref<InstanceType<typeof CardEditor> | null>(null)

/** The live card behind this window (`card.get`), when the branch still
 *  mounts it; re-read whenever cards may have changed. */
const card = ref<Card | null>(null)
/** The first read is in; until then the window assumes the card is there. */
const loaded = ref(false)
let readSeq = 0
watch(
  () => [props.entry.cardId, state.cardsRev, state.mainBranch] as const,
  async ([id]) => {
    const seq = ++readSeq
    let next: Card | null = null
    try {
      next = id ? await fetchCard(id) : null
    } catch {
      // A failed read keeps what is shown.
      if (seq === readSeq) loaded.value = true
      return
    }
    if (seq !== readSeq) return
    card.value = next
    loaded.value = true
  },
  { immediate: true },
)
const deck = computed(() => props.entry.deck ?? card.value?.deck ?? null)
const path = computed(() => props.entry.path ?? card.value?.path ?? null)
/** The address shown in the details: the mount path when there is one. */
const address = computed(() => props.entry.cardId ?? path.value ?? '')
/** A window whose card is gone (opened from a chip, then deleted or
 *  unmounted) shows its recorded view instead of a reader. */
const reader = computed(
  () => !!card.value || ((!loaded.value || props.entry.cardId === null) && !!deck.value && !!path.value),
)
const selectedUse = computed<CardUse | null>(() => {
  const refId = props.entry.useRef
  if (!refId) return null // a library open: always the live card
  const uses = cardUsesOnPath.value.get(card.value ? cardKey(card.value) : address.value) ?? []
  return uses.find((u) => u.callId === refId || u.node.id === refId) ?? null
})
const changed = computed(
  () => !!card.value && !!selectedUse.value && !useIsCurrent(card.value, selectedUse.value),
)
const tab = ref<'snapshot' | 'current'>('current')
watch(
  changed,
  (stale) => {
    // A stale use opens on the as-seen version; one still current does not.
    tab.value = stale ? 'snapshot' : 'current'
  },
  { immediate: true },
)

const recorded = computed<CardView | null>(() => {
  const use = selectedUse.value
  return use ? parseCardView(use.snapshot, use.kind === 'import') : null
})
const snapshot = computed(() => !!card.value && changed.value && tab.value === 'snapshot')
const EMPTY_VIEW: CardView = { description: null, keywords: [], meta: {}, body: '' }
const view = computed<CardView>(() => {
  if (snapshot.value) return recorded.value ?? EMPTY_VIEW
  const c = card.value
  if (!c) return recorded.value ?? EMPTY_VIEW
  return {
    description: c.description ?? null,
    keywords: c.keywords ?? [],
    meta: c.meta ?? {},
    body: c.body,
  }
})
const title = computed(() => address.value.split('/').at(-1) || address.value)
/** The card's directory, prefix included. */
const dir = computed(() => {
  const i = address.value.lastIndexOf('/')
  return i < 0 ? '' : address.value.slice(0, i)
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
  if (target?.length === 2) return { deck: target[0]!, path: target[1]! }
  if (deck.value && path.value) return { deck: deck.value, path: path.value }
  return mountedRef(address.value) ?? undefined
})
const shownHtml = computed(() => renderMarkdown(view.value.body, { breaks: false, base: linkBase.value }))
/** 详情 rows: the card's address, the version, then its other frontmatter. */
const detailRows = computed<[string, string][]>(() => {
  const rows: [string, string][] = [[t('cards.drawer.path'), address.value]]
  if (revision.value) rows.push([t('cards.drawer.revision'), revision.value])
  for (const [key, value] of Object.entries(view.value.meta)) {
    if (['description', 'keywords', 'scope'].includes(key)) continue
    rows.push([key, typeof value === 'string' ? value : JSON.stringify(value, null, 2)])
  }
  return rows
})

// Mounted only while its entry is in the stack: show it as a modal dialog
// (the top layer orders stacked windows) and close it on unmount.
onMounted(() => dialog.value?.showModal())
onBeforeUnmount(() => dialog.value?.close())

/** The topmost window owns the scrim; the ones below skip it so a card left
 *  open under a followed link stays legible. */
const top = computed(() => {
  const open = state.drawers.filter((w) => !w.closing)
  return open.at(-1)?.key === props.entry.key
})

/** Close the window, asking the pane first when it has unsaved work (a gone
 *  card has no pane to ask). The entry stays until the close transition has
 *  played. */
function close(): void {
  if (props.entry.closing || (pane.value && !pane.value.confirmLeave())) return
  beginDrawerClose(props.entry.key)
  dialog.value?.close()
  window.setTimeout(() => closeDrawer(props.entry.key), 200)
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

/** Keep showing the saved card after an in-window rename or move. */
function onMoved(nextDeck: string, nextPath: string): void {
  const entry = state.drawers.find((window) => window.key === props.entry.key)
  if (!entry) return
  const id = mountedId(nextDeck, nextPath)
  if (!id) return close()
  entry.cardId = id
  entry.deck = nextDeck
  entry.path = nextPath
}
</script>

<template>
  <Teleport to="body">
    <dialog
      :id="`card-window-${entry.key}`"
      ref="dialog"
      class="card-reader"
      :class="{ top }"
      :aria-label="t('cards.drawer.label')"
      @cancel.prevent="onCancel"
      @click="onBackdrop"
    >
      <CardEditor
        v-if="reader"
        ref="pane"
        :deck="deck ?? ''"
        :path="path ?? ''"
        :in-library="false"
        :override="snapshot ? view : null"
        :overlay="`#card-window-${entry.key}`"
        @close="close"
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
          @click="close"
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
  /* a reading window, not a page: past 720px a tall card scrolls inside */
  max-height: min(85dvh, 720px, calc(100dvh - 32px - env(safe-area-inset-top) - env(safe-area-inset-bottom)));
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
  background-color: var(--scrim);
  opacity: 0;
  transition:
    opacity 200ms ease-out,
    background-color 200ms ease-out,
    display 200ms allow-discrete,
    overlay 200ms allow-discrete;
}
.card-reader:not(.top)::backdrop {
  background-color: transparent;
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
