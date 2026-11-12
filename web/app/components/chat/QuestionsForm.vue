<script setup lang="ts">
// The questions of an `ask` to the user (`kind: "questions"`), inside the
// composer's decision panel. One question shows at a time — tabs switch
// between several — its options listed with their descriptions, and below
// them a line to answer in the user's own words, which every question has.
// Picking an option of a single-choice question moves on to the next one.
import { computed, nextTick, reactive, ref } from 'vue'
import { t } from '../../i18n'
import { answerDecision } from '../../lib/store'
import type { QuestionsRequest } from '../../lib/types'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ id: string; request: QuestionsRequest }>()

/** Where each question's answer stands. `own` is whether the user's own
 *  words count (for a question without options, always). */
const drafts = reactive(
  props.request.questions.map((q) => ({ selected: [] as string[], custom: '', own: !q.options.length })),
)
const index = ref(0)
const customEl = ref<HTMLInputElement>()

const question = computed(() => props.request.questions[index.value]!)
const draft = computed(() => drafts[index.value]!)
const last = computed(() => index.value === props.request.questions.length - 1)

function answered(i: number): boolean {
  const d = drafts[i]!
  return d.selected.length > 0 || (d.own && d.custom.trim() !== '')
}

const complete = computed(() => drafts.every((_, i) => answered(i)))

function pick(label: string): void {
  const d = draft.value
  if (question.value.multiple) {
    const at = d.selected.indexOf(label)
    if (at >= 0) d.selected.splice(at, 1)
    else d.selected.push(label)
    return
  }
  d.selected = [label]
  d.own = false
  if (!last.value) setTimeout(() => go(index.value + 1), 160)
}

/** The own-words row: choosing it (or typing into it) makes it count; for a
 *  single choice it replaces the picked option. */
function useOwn(): void {
  const d = draft.value
  d.own = true
  if (!question.value.multiple) d.selected = []
}

function toggleOwn(): void {
  if (draft.value.own && question.value.options.length) {
    draft.value.own = false
    return
  }
  useOwn()
  void nextTick(() => customEl.value?.focus())
}

function go(i: number): void {
  index.value = i
  // A question without options is answered by typing: put the caret there.
  if (!props.request.questions[i]!.options.length) void nextTick(() => customEl.value?.focus())
}

function next(): void {
  if (!last.value) go(index.value + 1)
  else if (complete.value) submit()
}

function submit(): void {
  void answerDecision(props.id, {
    answers: drafts.map((d) => ({
      selected: d.selected,
      custom: d.own && d.custom.trim() ? d.custom.trim() : undefined,
    })),
  })
}

function onCustomKey(e: KeyboardEvent): void {
  if (e.key === 'Enter' && !e.isComposing) {
    e.preventDefault()
    next()
  }
}
</script>

<template>
  <!-- The tabs stay on the panel while the question, its options and the keys
       scroll under them: a phone's panel is short, and which question you are
       on should not depend on how far down you are. -->
  <div class="flex min-h-0 flex-1 flex-col">
    <!-- tabs, when there are several questions -->
    <div v-if="request.questions.length > 1" class="mt-2 flex flex-none flex-wrap gap-1" role="tablist">
      <button
        v-for="(q, i) in request.questions"
        :key="i"
        type="button"
        role="tab"
        :aria-selected="i === index"
        class="inline-flex items-center gap-1 rounded-full px-2.5 py-0.5 text-meta font-semibold transition-colors"
        :class="
          i === index ? 'bg-ink text-surface' : 'bg-surface text-ink-2 ring-1 ring-line-soft hover:text-ink'
        "
        @click="go(i)"
      >
        <Icon v-if="answered(i)" name="ok" :size="12" :class="i === index ? '' : 'text-pine'" />
        {{ q.header || t('chat.ask.questionN', { n: i + 1 }) }}
      </button>
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto">
      <p class="mt-2.5 text-message leading-snug font-semibold text-ink">{{ question.question }}</p>
      <p v-if="question.multiple" class="mt-0.5 text-micro text-ink-3">{{ t('chat.ask.multiple') }}</p>

      <div
        class="mt-2 divide-y divide-line-soft overflow-hidden rounded-control bg-surface ring-1 ring-line-soft"
        :role="question.multiple ? 'group' : 'radiogroup'"
        :aria-label="question.question"
      >
        <button
          v-for="o in question.options"
          :key="o.label"
          type="button"
          :role="question.multiple ? 'checkbox' : 'radio'"
          :aria-checked="draft.selected.includes(o.label)"
          class="q-option"
          :class="{ 'is-on': draft.selected.includes(o.label) }"
          @click="pick(o.label)"
        >
          <span class="q-mark" :class="question.multiple ? 'rounded-[5px]' : 'rounded-full'">
            <Icon v-if="question.multiple" name="ok" :size="11" />
          </span>
          <span class="min-w-0">
            <span class="block text-secondary font-semibold text-ink">{{ o.label }}</span>
            <span v-if="o.description" class="block text-meta text-ink-3">{{ o.description }}</span>
          </span>
        </button>
        <!-- the user's own words: always there -->
        <div class="q-option" :class="{ 'is-on': question.options.length && draft.own }">
          <button
            v-if="question.options.length"
            type="button"
            :role="question.multiple ? 'checkbox' : 'radio'"
            :aria-checked="draft.own"
            :aria-label="t('chat.ask.custom')"
            class="q-mark"
            :class="question.multiple ? 'rounded-[5px]' : 'rounded-full'"
            @click="toggleOwn"
          >
            <Icon v-if="question.multiple" name="ok" :size="11" />
          </button>
          <input
            ref="customEl"
            v-model="draft.custom"
            class="min-w-0 flex-1 border-0 bg-transparent p-0 text-secondary focus:shadow-none"
            :placeholder="question.options.length ? t('chat.ask.custom') : t('chat.ask.answer')"
            @focus="useOwn"
            @keydown="onCustomKey"
          />
        </div>
      </div>

      <div class="mt-2.5 flex items-center justify-end gap-1.5">
        <button type="button" class="btn px-3 py-1" @click="answerDecision(id, {})">
          {{ t('chat.ask.skip') }}
        </button>
        <button v-if="!last" type="button" class="btn px-3 py-1" @click="next">
          {{ t('chat.ask.next') }}
        </button>
        <button v-else type="button" class="btn btn-primary px-3 py-1" :disabled="!complete" @click="submit">
          {{ t('chat.ask.submit') }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.q-option {
  display: flex;
  width: 100%;
  align-items: flex-start;
  gap: 10px;
  padding: 8px 12px;
  text-align: left;
  transition: background-color 0.15s;
}

button.q-option:not(.is-on):hover {
  background: var(--surface-2);
}

.q-option.is-on {
  background: color-mix(in srgb, var(--brand-soft) 55%, var(--surface));
}

/* the radio dot / check box; it fills coral when its row is chosen */
.q-mark {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  margin-top: 2px;
  border: 1.5px solid color-mix(in srgb, var(--ink) 28%, var(--line));
  background: var(--surface);
  color: transparent;
  transition:
    background-color 0.15s,
    border-color 0.15s,
    box-shadow 0.15s;
}

.is-on > .q-mark {
  border-color: var(--brand);
  background: var(--brand);
  color: var(--on-brand);
}

/* a chosen radio: a ring around a dot rather than a filled disc */
.is-on > .q-mark.rounded-full {
  background: var(--surface);
  box-shadow:
    inset 0 0 0 3px var(--surface),
    inset 0 0 0 8px var(--brand);
}
</style>
