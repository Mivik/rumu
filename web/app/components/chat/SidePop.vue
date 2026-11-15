<script setup lang="ts">
// The side questions card, at the top right of the sheet over the
// conversation. It opens only when asked (`/btw`, or a selected passage's
// side key), folds to its title row, and closes to nothing: the passages
// asked about keep a dotted underline in the conversation, and a click on
// one opens its question again. A bare `/btw` lists them.
//
// A side branch has no tools, so a turn is just text: the card renders its
// prompts and replies plainly rather than through the timeline's groups.
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { t } from '../../i18n'
import { pathOf, state } from '../../lib/store'
import type { BranchId, Node, Quote } from '../../lib/types'
import { renderMarkdown } from '../../lib/markdown'
import { pathNodes } from '../../lib/state/derived'
import { addQuote, askSide, closeSide, openSide, side, sideBranches } from '../../lib/state/side'
import { requestComposerFocus } from '../../lib/state/ui'
import Icon from '../ui/Icon.vue'

const quoteOf = (id: BranchId | null): Quote | null => {
  const b = id ? state.branches[id] : null
  return b?.kind.type === 'side' ? (b.kind.quote ?? null) : null
}
const quote = computed(() => (side.branch ? quoteOf(side.branch) : side.quote))
const listing = computed(() => !side.branch && !side.quote && sideBranches.value.length > 0)
const busy = computed(() => !!side.branch && !!state.busy[side.branch])

/** A side branch's own turns: its prompts and replies, from the first
 *  prompt on (the framing note before it is the model's). */
function turnsOf(id: BranchId): Node[] {
  const b = state.branches[id]
  const own = pathOf(b, b?.forked_from)
  const at = own.findIndex((n) => n.body.type === 'user')
  return at < 0 ? [] : own.slice(at).filter((n) => n.body.type === 'user' || textOf(n))
}

function textOf(n: Node): string {
  return n.body.type === 'user' || n.body.type === 'assistant' ? n.body.content.text.trim() : ''
}

const turns = computed(() => (side.branch ? turnsOf(side.branch) : []))
const streaming = computed(() => (side.branch && busy.value ? (state.deltas[side.branch]?.text ?? '') : ''))
const hasBody = computed(() => listing.value || !!quote.value || turns.value.length > 0 || busy.value)

function question(id: BranchId): string {
  const first = turnsOf(id)[0]
  return first ? textOf(first) : ''
}

// ---- writing ----

const draft = ref('')
const sending = ref(false)
const inputEl = ref<HTMLTextAreaElement>()

async function send(): Promise<void> {
  const text = draft.value.trim()
  if (!text || sending.value) return
  sending.value = true
  draft.value = ''
  if (!(await askSide(text))) draft.value = text
  sending.value = false
}

function onKeydown(e: KeyboardEvent): void {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
    e.preventDefault()
    void send()
  }
}

/** Quote a reply into the composer: what a side question turned up joins
 *  the main line only this way. */
function bringBack(n: Node): void {
  addQuote({ node: n.id, text: textOf(n) })
  requestComposerFocus()
}

// Opening (or switching) puts the caret in the card; the touch keyboard
// waits for a tap, as the composer's does.
watch(
  () => [side.open, side.branch, side.quote] as const,
  ([open]) => {
    if (!open || side.folded || matchMedia('(pointer: coarse)').matches) return
    void nextTick(() => inputEl.value?.focus())
  },
)

// Follow the reply as it grows.
const scrollEl = ref<HTMLElement>()
watch([turns, streaming], () => {
  void nextTick(() => {
    const el = scrollEl.value
    if (el) el.scrollTop = el.scrollHeight
  })
})

/** Scroll the conversation to a quoted passage's message. */
function locate(q: Quote): void {
  const el = document.querySelector<HTMLElement>(`.tl-col [data-path-node="${CSS.escape(q.node)}"]`)
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
  for (const b of sideBranches.value) {
    const q = quoteOf(b.id)
    const el = q && document.querySelector(`.tl-col .md[data-path-node="${CSS.escape(q.node)}"]`)
    const r = el && findText(el, q.text)
    if (r) marked.set(r, b.id)
  }
  CSS.highlights.set('side-quote', new Highlight(...marked.keys()))
}

watch([sideBranches, () => pathNodes.value.length, () => state.sessionId], () => void nextTick(paint), {
  immediate: true,
})

/** A click on an underlined passage opens what was asked about it. */
function onClick(e: MouseEvent): void {
  if (!marked.size || !getSelection()?.isCollapsed) return
  if (!(e.target as Element | null)?.closest?.('.tl-col')) return
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
  if ('highlights' in CSS) CSS.highlights.delete('side-quote')
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
          <div v-if="hasBody" ref="scrollEl" class="min-h-0 flex-1 overflow-y-auto px-4 py-3">
            <div v-if="listing" class="-mx-2 flex flex-col">
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
                class="mb-3 line-clamp-3 w-full border-l-[3px] border-agent pl-2.5 text-left text-meta text-ink-2"
                @click="locate(quote)"
              >
                {{ quote.text }}
              </button>
              <div class="flex flex-col gap-3">
                <template v-for="n in turns" :key="n.id">
                  <p
                    v-if="n.body.type === 'user'"
                    class="ml-auto max-w-[85%] rounded-card bg-agent-soft px-3 py-1.5 text-secondary whitespace-pre-wrap"
                  >
                    {{ textOf(n) }}
                  </p>
                  <div v-else class="flex flex-col items-start gap-0.5">
                    <div class="md text-secondary" v-html="renderMarkdown(textOf(n))" />
                    <button
                      class="-ml-1.5 inline-flex items-center gap-1 rounded-control px-1.5 py-0.5 text-meta text-ink-3 transition-colors hover:bg-surface-2 hover:text-ink"
                      @click="bringBack(n)"
                    >
                      <Icon name="unside" :size="13" />{{ t('chat.side.bringBack') }}
                    </button>
                  </div>
                </template>
                <div
                  v-if="busy"
                  class="md streaming-caret text-secondary"
                  v-html="renderMarkdown(streaming)"
                />
              </div>
            </template>
          </div>
          <div class="flex flex-none items-end gap-1.5 p-2" :class="{ 'border-t border-line-soft': hasBody }">
            <textarea
              ref="inputEl"
              v-model="draft"
              rows="1"
              class="max-h-[120px] min-h-0 flex-1 resize-none border-0 bg-transparent px-2 py-1 text-secondary [field-sizing:content] focus:shadow-none"
              :placeholder="side.branch ? t('chat.side.followUp') : t('chat.side.placeholder')"
              @keydown="onKeydown"
            />
            <button
              class="composer-send is-side size-7"
              :disabled="!draft.trim() || sending"
              :title="t('chat.composer.send')"
              :aria-label="t('chat.composer.send')"
              @click="send"
            >
              <Icon name="send" :size="14" />
            </button>
          </div>
        </div>
      </div>
    </section>
  </Transition>
</template>
