<script setup lang="ts">
// The side questions card, at the top right of a conversation (the main
// line's, or a sub-agent's in its panel), over it. It opens only when asked (`/btw`, or a selected passage's
// side key), folds to its title row, and closes to nothing: the passages
// asked about keep a dotted underline in the conversation, and a click on
// one opens its question again. A bare `/btw` lists them.
//
// Inside, the side question is a conversation like any other: the same
// timeline and composer as the chat's (BranchChat), on the side branch
// (`sideView`). Before its first answer the card is the list or the passage
// asked about, over that composer; sending asks it (`askSide`).
import { computed, nextTick, onBeforeUnmount, onMounted, watch } from 'vue'
import { t } from '../../i18n'
import { pathOf, provideBranchView, sideView, state, useBranchView } from '../../lib/store'
import type { BranchId, Quote } from '../../lib/types'
import { closeSide as close, openSide as open, paintQuotes } from '../../lib/state/side'
import { revealNode, timelineRendered } from '../../lib/virtualEntries'
import BranchChat from './BranchChat.vue'
import ConversationView from './ConversationView.vue'
import Icon from '../ui/Icon.vue'

/** The conversation it sits on (BranchChat's root): what it looks for
 *  quoted passages in. */
const props = defineProps<{ root: () => HTMLElement | null }>()

const view = useBranchView()
const aside = view.aside
const side = aside.card
const sideBranches = aside.branches
const openSide = (id: BranchId | null) => open(aside, id)
const closeSide = () => close(aside)

/** What the card's timeline and composer show: the open side question. */
const inner = sideView(view)
provideBranchView(inner)

const quoteOf = (id: BranchId | null): Quote | null => {
  const b = id ? state.branches[id] : null
  return b?.kind.type === 'side' ? (b.kind.quote ?? null) : null
}
const quote = computed(() => (side.branch ? quoteOf(side.branch) : side.quote))
const listing = computed(() => !side.branch && !side.quote && sideBranches.value.length > 0)
const busy = computed(() => !!side.branch && !!state.busy[side.branch])

/** A side question's question: its first prompt (the framing note before
 *  it is the model's). */
function question(id: BranchId): string {
  const b = state.branches[id]
  const first = pathOf(b, b?.forked_from).find((n) => n.body.type === 'user')
  return first?.body.type === 'user' ? first.body.content.text.trim() : ''
}

// Opening (or switching) puts the caret in the card's composer.
watch(
  () => [side.open, side.branch, side.quote] as const,
  ([open]) => {
    if (open && !side.folded) inner.aside.focus.value++
  },
)

/** Scroll the conversation to a quoted passage's message. */
async function locate(q: Quote): Promise<void> {
  if (!view.sub) await revealNode(q.node)
  const el = props.root()?.querySelector<HTMLElement>(`.tl-col [data-path-node="${CSS.escape(q.node)}"]`)
  el?.scrollIntoView({ behavior: 'smooth', block: 'center' })
}

// ---- the passages asked about, underlined in the conversation ----

const marked = new Map<Range, BranchId>()

/** `text` inside `root`, matched with whitespace ignored: a selection's
 *  string has line breaks where the rendered blocks have none. */
function findText(root: Element, text: string): Range | null {
  const needle = text.replace(/\s+/g, '')
  if (!needle) return null
  const at: { node: Text; offset: number }[] = []
  let flat = ''
  const walk = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  for (let n = walk.nextNode() as Text | null; n; n = walk.nextNode() as Text | null) {
    const s = n.data
    for (let i = 0; i < s.length; i++) {
      if (/\s/.test(s[i]!)) continue
      at.push({ node: n, offset: i })
      flat += s[i]
    }
  }
  const i = flat.indexOf(needle)
  if (i < 0) return null
  const start = at[i]!
  const end = at[i + needle.length - 1]!
  const r = document.createRange()
  r.setStart(start.node, start.offset)
  r.setEnd(end.node, end.offset + 1)
  return r
}

function paint(): void {
  if (!('highlights' in CSS)) return
  marked.clear()
  const root = props.root()
  for (const b of sideBranches.value) {
    const q = quoteOf(b.id)
    const el = q && root?.querySelector(`.tl-col .md[data-path-node="${CSS.escape(q.node)}"]`)
    const r = el && findText(el, q.text)
    if (r) marked.set(r, b.id)
  }
  paintQuotes(aside, [...marked.keys()])
}

// (the virtual timeline renders entries in and out: look again when it does)
watch(
  [sideBranches, () => view.nodes.value.length, () => state.sessionId, timelineRendered],
  () => void nextTick(paint),
  { immediate: true },
)

/** A click on an underlined passage opens what was asked about it. */
function onClick(e: MouseEvent): void {
  if (!marked.size || !getSelection()?.isCollapsed) return
  const target = e.target as Element | null
  if (!target?.closest?.('.tl-col') || !props.root()?.contains(target) || !view.holds(target)) return
  const pos = document.caretPositionFromPoint?.(e.clientX, e.clientY)
  if (!pos) return
  for (const [r, id] of marked) {
    try {
      if (r.comparePoint(pos.offsetNode, pos.offset) === 0) return openSide(id)
    } catch {
      // a range left in an unmounted message
    }
  }
}

onMounted(() => document.addEventListener('click', onClick))
onBeforeUnmount(() => {
  document.removeEventListener('click', onClick)
  paintQuotes(aside, null)
})
</script>

<template>
  <Transition
    enter-active-class="transition duration-300 ease-soft"
    enter-from-class="-translate-y-2 scale-95 opacity-0"
    leave-active-class="transition duration-150 ease-soft"
    leave-to-class="-translate-y-1 scale-[.97] opacity-0"
  >
    <section
      v-if="side.open"
      :data-view="inner.key"
      :data-branch="side.branch"
      class="absolute top-2 right-2 left-2 z-20 flex max-h-[min(64%,560px)] origin-top-right flex-col overflow-hidden rounded-card border border-line bg-surface shadow-pop transition-[width] duration-300 ease-soft [interpolate-size:allow-keywords] sm:right-4 sm:left-auto"
      :class="side.folded ? 'sm:w-fit' : 'sm:w-[400px]'"
      @keydown.esc="closeSide()"
    >
      <!-- what folding adds to the title row (the question) and takes from
           it (the way back to the list) slides in and out with it, so the
           card's width moves from where it was -->
      <header
        class="flex min-h-11 flex-none cursor-pointer items-center gap-1 pr-1.5 pl-2"
        @click="side.folded = !side.folded"
      >
        <span
          v-if="(side.branch || side.quote) && sideBranches.length"
          class="flex flex-none overflow-hidden transition-[max-width,opacity,margin] duration-300 ease-soft"
          :class="side.folded ? '-mr-1 max-w-0 opacity-0' : 'max-w-7'"
        >
          <button
            class="icon-btn size-7 flex-none"
            :title="t('chat.side.all')"
            :aria-label="t('chat.side.all')"
            :tabindex="side.folded ? -1 : 0"
            @click.stop="openSide(null)"
          >
            <Icon name="back" :size="14" />
          </button>
        </span>
        <span class="flex min-w-0 flex-1 items-center gap-1.5 pl-1.5 text-secondary">
          <Icon name="side" :size="15" class="flex-none text-agent" />
          <span class="flex-none font-bold text-agent">{{ t('chat.side.title') }}</span>
          <span
            v-if="side.branch"
            class="truncate text-ink-2 transition-[max-width,opacity,margin] duration-300 ease-soft"
            :class="side.folded ? 'max-w-[180px]' : '-ml-1.5 max-w-0 opacity-0'"
            >{{ question(side.branch) }}</span
          >
          <span v-if="busy" class="dot-live size-1.5 flex-none rounded-full bg-brand" />
        </span>
        <button
          class="icon-btn size-7 flex-none"
          :title="t('chat.side.close')"
          :aria-label="t('chat.side.close')"
          @click.stop="closeSide()"
        >
          <Icon name="close" :size="15" />
        </button>
      </header>

      <!-- folding closes the body's row, as the ask panel's does; its
           contents keep out of the card's width (inline-size containment),
           so the folded card hugs its title row -->
      <div
        class="grid min-h-0 transition-[grid-template-rows] duration-300 ease-soft [contain:inline-size]"
        :class="side.folded ? 'grid-rows-[0fr]' : 'grid-rows-[1fr]'"
        :inert="side.folded"
      >
        <div class="flex min-h-0 flex-col overflow-hidden border-t border-line-soft">
          <BranchChat v-slot="{ composerOf }">
            <div v-if="listing" class="flex min-h-0 flex-col overflow-y-auto px-2 py-2">
              <button
                v-for="b in sideBranches"
                :key="b.id"
                class="flex min-w-0 flex-col rounded-control px-2 py-1.5 text-left transition-colors hover:bg-surface-2"
                @click="openSide(b.id)"
              >
                <span class="flex min-w-0 items-center gap-1.5 text-secondary">
                  <span class="truncate">{{ question(b.id) }}</span>
                  <span v-if="state.busy[b.id]" class="dot-live size-1.5 flex-none rounded-full bg-brand" />
                </span>
                <span v-if="quoteOf(b.id)" class="truncate text-meta text-ink-3"
                  >“{{ quoteOf(b.id)!.text }}”</span
                >
              </button>
            </div>
            <template v-else>
              <button
                v-if="quote"
                class="mx-4 mt-3 line-clamp-3 flex-none border-l-[3px] border-agent pl-2.5 text-left text-meta text-ink-2 sm:mx-6"
                @click="locate(quote)"
              >
                {{ quote.text }}
              </button>
              <ConversationView v-if="side.branch" :composer-of="composerOf" />
            </template>
          </BranchChat>
        </div>
      </div>
    </section>
  </Transition>
</template>
