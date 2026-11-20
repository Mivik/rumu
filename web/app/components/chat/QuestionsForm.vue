<script setup lang="ts">
// The questions of an `ask` to the user (`kind: "questions"`), inside the
// composer's decision panel. One question shows at a time — tabs switch
// between several — its options listed with their descriptions, and a last
// row for the user's own words, which every question has: one more box to
// tick for several choices, one more radio for a single choice. Each option
// of a single choice has a line of its own for a note sent along with it: the
// pen at the end of a row opens that line (and turns into a close key while
// it is open), and the line and what was written in it stay put through the
// options being tried and picked — picking one opens no line and closes none,
// it only moves on to the next question. Only the option picked has its note
// sent.
import { computed, nextTick, reactive, ref } from 'vue'
import { t } from '../../i18n'
import { answerDecision } from '../../lib/store'
import type { QuestionsRequest } from '../../lib/types'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ id: string; request: QuestionsRequest }>()

/** One question's answer: the options picked, the user's own words and
 *  whether their row is chosen, and — for a single choice — a note per option
 *  label together with the labels whose line for one is open. Notes belong to
 *  their option, not to the option picked: opening, writing and closing one
 *  is independent of which option is picked and of the others' lines. */
type Draft = {
  selected: string[]
  custom: string
  useCustom: boolean
  notes: Record<string, string>
  openNotes: string[]
}

const drafts = reactive<Draft[]>(
  props.request.questions.map((q) => ({
    selected: [],
    custom: '',
    useCustom: false,
    notes: Object.fromEntries(q.options.map((o) => [o.label, ''])),
    openNotes: [],
  })),
)
const rootEl = ref<HTMLElement>()
const index = ref(0)
const customEl = ref<HTMLInputElement>()
const optionsEl = ref<HTMLElement>()

const question = computed(() => props.request.questions[index.value]!)
const draft = computed(() => drafts[index.value]!)
const last = computed(() => index.value === props.request.questions.length - 1)

function customCounts(i: number): boolean {
  const d = drafts[i]!
  return d.custom.trim() !== '' && (d.useCustom || !props.request.questions[i]!.options.length)
}

function picked(i: number, label: string): boolean {
  return drafts[i]!.selected.includes(label)
}

function answered(i: number): boolean {
  return customCounts(i) || drafts[i]!.selected.length > 0
}

/** The line for a note on an option of a single choice: open once the pen at
 *  the end of its row opened it, and it stays open across the options tried
 *  and picked — its own text stays with it. */
function noteOpen(i: number, label: string): boolean {
  return drafts[i]!.openNotes.includes(label)
}

/** Close that option's line and drop its note: closing stands for no note. */
function closeNote(label: string): void {
  const d = draft.value
  d.openNotes = d.openNotes.filter((l) => l !== label)
  d.notes[label] = ''
}

const complete = computed(() => drafts.every((_, i) => answered(i)))

/** `note`: the pen at the end of a row — picks that option of a single
 *  choice and opens its line for a note, staying on the question with the
 *  caret in it. Picking the option itself opens no line (a note is written
 *  because the user went for it) and closes none: every line opened stays
 *  where it is, with what was written in it. */
function pick(label: string, note = false): void {
  const d = draft.value
  if (question.value.multiple) {
    const at = d.selected.indexOf(label)
    if (at >= 0) d.selected.splice(at, 1)
    else d.selected.push(label)
    return
  }
  d.selected = [label]
  d.useCustom = false
  // A pick moves on to the next question, unless a note is being written on
  // this one: that keeps the question (and its lines) in view.
  if (!note) {
    if (!last.value && !d.openNotes.length) setTimeout(() => go(index.value + 1), 160)
    return
  }
  if (!d.openNotes.includes(label)) d.openNotes.push(label)
  // Its own line, wherever it sits in the list.
  void nextTick(() => {
    const rows = optionsEl.value?.querySelectorAll<HTMLInputElement>('input.q-note') ?? []
    for (const row of rows) if (row.dataset.note === label) row.focus()
  })
}

/** Clicking into the box of the user's own words, or typing in it, chooses
 *  that row — for a single choice in place of the option picked. It counts as
 *  an answer once something is written in it. */
function chooseCustom(): void {
  const d = draft.value
  d.useCustom = true
  if (!question.value.multiple) d.selected = []
}

function toggleCustom(): void {
  if (question.value.multiple && draft.value.useCustom) {
    draft.value.useCustom = false
    return
  }
  chooseCustom()
  customEl.value?.focus()
}

function go(i: number): void {
  const active = document.activeElement
  const typing = active instanceof HTMLInputElement && !!rootEl.value?.contains(active)
  index.value = i
  // The inputs serve every question, so a caret in one would stay put: send it
  // on to the next question's first option instead. A question without
  // options is answered by typing, so the caret goes into the box.
  const options = props.request.questions[i]!.options.length
  if (typing || !options) {
    void nextTick(() => (options ? optionsEl.value?.querySelector('button') : customEl.value)?.focus())
  }
}

function next(): void {
  if (!last.value) go(index.value + 1)
  else if (complete.value) submit()
}

function submit(): void {
  void answerDecision(props.id, {
    // A single choice is either an option, its note going along as the words,
    // or the user's own words alone; the tool tells the two apart.
    answers: drafts.map((d, i) => {
      const label = d.selected[0]
      const note = !props.request.questions[i]!.multiple && label !== undefined ? d.notes[label]!.trim() : ''
      return {
        selected: d.selected,
        custom: customCounts(i) ? d.custom.trim() : note || undefined,
      }
    }),
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
  <div ref="rootEl" class="flex min-h-0 flex-1 flex-col">
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
        v-if="question.options.length"
        ref="optionsEl"
        class="mt-2 divide-y divide-line-soft overflow-hidden rounded-control bg-surface ring-1 ring-line-soft"
        :role="question.multiple ? 'group' : 'radiogroup'"
        :aria-label="question.question"
      >
        <div v-for="o in question.options" :key="o.label" :class="{ 'is-on': picked(index, o.label) }">
          <div class="q-line flex">
            <button
              type="button"
              :role="question.multiple ? 'checkbox' : 'radio'"
              :aria-checked="picked(index, o.label)"
              class="q-option min-w-0 flex-1"
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
            <!-- a single choice: the pen picks this one and opens its line
                 for a note, and closes it again while it is open -->
            <button
              v-if="!question.multiple"
              type="button"
              class="flex-none self-stretch px-3 text-ink-3 transition-colors hover:text-ink"
              :title="t(noteOpen(index, o.label) ? 'chat.ask.closeNote' : 'chat.ask.addNote')"
              :aria-label="t(noteOpen(index, o.label) ? 'chat.ask.closeNote' : 'chat.ask.addNote')"
              @click="noteOpen(index, o.label) ? closeNote(o.label) : pick(o.label, true)"
            >
              <Icon :name="noteOpen(index, o.label) ? 'close' : 'edit'" :size="14" />
            </button>
          </div>
          <!-- a single choice: this option's own note, its line open while
               the pen asked for it -->
          <div v-if="noteOpen(index, o.label)" class="-mt-1 pr-3 pb-2 pl-[38px]">
            <input
              v-model="draft.notes[o.label]"
              :data-note="o.label"
              class="q-note w-full rounded-[6px] border-0 bg-surface px-2 py-1 text-meta ring-1 ring-line-soft focus:shadow-none"
              :placeholder="t('chat.ask.note')"
              :aria-label="t('chat.ask.note')"
              @keydown="onCustomKey"
            />
          </div>
        </div>
        <!-- the user's own words: one more box to tick, or one more radio -->
        <div class="q-option" :class="{ 'is-on': draft.useCustom }">
          <button
            type="button"
            :role="question.multiple ? 'checkbox' : 'radio'"
            :aria-checked="draft.useCustom"
            :aria-label="t('chat.ask.custom')"
            class="q-mark"
            :class="question.multiple ? 'rounded-[5px]' : 'rounded-full'"
            @click="toggleCustom"
          >
            <Icon v-if="question.multiple" name="ok" :size="11" />
          </button>
          <input
            ref="customEl"
            v-model="draft.custom"
            class="min-w-0 flex-1 border-0 bg-transparent p-0 text-secondary focus:shadow-none"
            :placeholder="t('chat.ask.custom')"
            @click="chooseCustom"
            @input="chooseCustom"
            @keydown="onCustomKey"
          />
        </div>
      </div>
      <!-- nothing to pick from: the words are the answer itself -->
      <div v-else class="mt-2 flex rounded-control bg-surface px-3 py-2 ring-1 ring-line-soft">
        <input
          ref="customEl"
          v-model="draft.custom"
          class="min-w-0 flex-1 border-0 bg-transparent p-0 text-secondary focus:shadow-none"
          :placeholder="t('chat.ask.answer')"
          @keydown="onCustomKey"
        />
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

:not(.is-on) > .q-line:hover {
  background: var(--surface-2);
}

.is-on {
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

.is-on .q-mark {
  border-color: var(--brand);
  background: var(--brand);
  color: var(--on-brand);
}

/* a chosen radio: a ring around a dot rather than a filled disc */
.is-on .q-mark.rounded-full {
  background: var(--surface);
  box-shadow:
    inset 0 0 0 3px var(--surface),
    inset 0 0 0 8px var(--brand);
}
</style>
