<script setup lang="ts">
// The message composer. It lives at the bottom of the chat, and at the center
// of the landing state — in which case sending the first message creates the
// session (store.sendPrompt does that).
//
// A sheet of paper with the writing on top and a footer that says how the
// prompt runs: the model and its reasoning effort on the left; the send button
// on the right — solid coral once there is something to send. Who a new
// session runs as and where it works are picked above the sheet
// (SessionPickers), so the footer is the same on the landing page and in a
// session.
// While rumu works, sending steers the turn and a stop button joins it.
// The writing and its attachments are the open session's draft (or the
// landing page's), kept per session across switches and reloads
// (lib/state/drafts).
//
// A tool call waiting on the user's approval unfolds from the top of the
// sheet (DecisionPanel), above the writing. The sheet is a column that may
// shrink (min-h-0): a request taller than the room the sheet has scrolls
// inside its panel instead of growing the sheet past the window's foot.
//
// Attachments attach three ways — paste, drop, the footer's paperclip key —
// and upload the moment they arrive, into the tray above the writing: an
// image as a thumbnail, a text document as a chip. A failed upload keeps its
// tile (the server's word arrived as a toast) until retried or removed. The
// writing stays plain text: like the Claude and ChatGPT web apps, the sent
// message leads with its attachments (`withLeadingAnchors`).
//
// On a phone the sheet folds to one line while you are elsewhere — a tap
// on the transcript or the sidebar, not a scroll — and unfolds when touched
// again. It stays
// open while it holds attachments or a decision.
//
// Text selected in a reply offers itself above the writing (QuoteTray): as a
// quote leading the message, or as a side question. `/btw` asks on the side
// too (SidePop); the sheet's edge turns the side's teal while it would.
//
// Editing a sent message (`editTarget`) loads its text and images here, the
// draft set aside until the edit is resent or cancelled; sending forks from
// just before the message (store.editPrompt).
//
// A steering prompt still queued (shown above the sheet, BranchChat) comes
// back into the writing on its key or on ↑ at the writing's start, unsent
// (store.takeBackPrompt); stopping the turn brings all of them back.
//
// It writes to the shown branch (`useBranchView`), with that branch's model,
// context, quotes and side questions. Beside the chat, in a sub-agent's
// panel, it only talks to the sub-agent: its own unsaved draft, and no edit
// or decisions (those stay with the main composer).
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, shallowRef, watch } from 'vue'
import { t } from '../../i18n'
import {
  compactBranch,
  editPrompt,
  landingAssistant,
  sendPrompt,
  state,
  stopTurn,
  takeBackPrompt,
  useBranchView,
} from '../../lib/store'
import type { PromptAttachment } from '../../lib/store'
import type { Attachment, Content, Node as TreeNode } from '../../lib/types'
import { kindOf, sniffFile, typedText, uploadBlob, withLeadingAnchors } from '../../lib/attachments'
import { composerFocus, editTarget as mainEdit, openLightbox, toast } from '../../lib/state/ui'
import { askSide, openSide, quoteBlock } from '../../lib/state/side'
import { dragging, setDropSink } from '../../lib/state/drops'
import {
  activeDraft,
  nextAttachmentId,
  releaseAttachment,
  storedAttachment,
  type Draft,
  type PendingAttachment,
} from '../../lib/state/drafts'
import Icon from '../ui/Icon.vue'
import DecisionPanel from './DecisionPanel.vue'
import ContextMeter from './ContextMeter.vue'
import DocumentChip from './DocumentChip.vue'
import ModelPicker from './ModelPicker.vue'
import QuoteTray from './QuoteTray.vue'

const MAX_ATTACHMENTS = 20

const view = useBranchView()
const { busy: isBusy, queued: queuedPrompts } = view
/** A sub-agent's composer never edits: this stays empty there. */
const editTarget = view.sub ? shallowRef<TreeNode | null>(null) : mainEdit
const quotes = view.aside.quotes
/** A sub-agent's composer writes on a draft of its own, kept while it is open. */
const subDraft: Draft | null = view.sub ? reactive({ text: '', attachments: [] }) : null

/** The message being edited, held apart from the session's draft. */
const editing = ref<Draft | null>(null)
/** What the sheet shows: the edit, else the open session's draft (lib/state/drafts). */
const current = computed(() => editing.value ?? subDraft ?? activeDraft.value)
const draft = computed({
  get: () => current.value.text,
  set: (text) => (current.value.text = text),
})
const attachments = computed({
  get: () => current.value.attachments,
  set: (attachments) => (current.value.attachments = attachments),
})
const rootEl = ref<HTMLElement>()
const textareaEl = ref<HTMLTextAreaElement>()
const fileEl = ref<HTMLInputElement>()

const uploading = computed(() => attachments.value.some((a) => a.status === 'uploading'))
const canSend = computed(
  () =>
    !uploading.value &&
    (draft.value.trim() !== '' ||
      quotes.value.length > 0 ||
      attachments.value.some((a) => a.status === 'done')),
)
/** The draft asks on the side (`/btw`), not in the conversation. */
const btw = computed(() => !!state.sessionId && !editTarget.value && /^\/btw(\s|$)/.test(draft.value))

async function run(item: PendingAttachment): Promise<void> {
  if (!item.file) return
  item.status = 'uploading'
  item.error = undefined
  try {
    item.attachment = await uploadBlob(item.file, item.ctrl.signal)
    item.status = 'done'
  } catch (err) {
    if (item.ctrl.signal.aborted) return // removed while in flight
    item.status = 'failed'
    item.error = (err as Error).message
    toast('error', t('chat.composer.uploadFailed'), item.error)
  }
}

/** Take the files in, each as what its bytes say it is (`sniffFile`): an
 *  image or a text document; anything else is refused with a word. */
async function attachFiles(list: FileList | File[]): Promise<void> {
  const files = [...list]
  // The buffer they land in is the one open now, not after the reads.
  const into = current.value
  const kinds = await Promise.all(files.map(sniffFile))
  const take = files.flatMap((file, i) => (kinds[i] ? [{ file, kind: kinds[i] }] : []))
  if (take.length < files.length) toast('error', t('chat.composer.notAttachable'))
  const room = MAX_ATTACHMENTS - into.attachments.length
  if (take.length > room) {
    toast('error', t('chat.composer.tooManyAttachments', { max: MAX_ATTACHMENTS }))
  }
  for (const { file, kind } of take.slice(0, Math.max(0, room))) {
    const item = reactive<PendingAttachment>({
      id: nextAttachmentId(),
      file,
      name: file.name,
      // a document has no picture; only an image gets a local preview
      url: kind === 'image' ? URL.createObjectURL(file) : '',
      status: 'uploading',
      ctrl: new AbortController(),
    })
    into.attachments.push(item)
    void run(item)
  }
}

/** Whether a tile shows as a picture: an image does, a document shows as a
 *  chip. Before the upload answers, only an image has a local preview. */
function picture(item: PendingAttachment): boolean {
  if (item.attachment) return kindOf(item.attachment) === 'image'
  return !!item.url
}

/** The size a chip shows: the stored bytes once uploaded, the file's own
 *  before that. */
function sizeOf(item: PendingAttachment): number {
  return item.attachment?.size ?? item.file?.size ?? 0
}

/** Show a tile full-size: the stored image once uploaded, the local
 *  preview before that. */
function preview(item: PendingAttachment): void {
  openLightbox(item.attachment ?? { src: item.url, name: item.name || null })
}

function removeAt(i: number): void {
  const item = attachments.value[i]
  if (!item) return
  releaseAttachment(item)
  attachments.value.splice(i, 1)
}

function onPaste(e: ClipboardEvent): void {
  const files = e.clipboardData?.files
  if (!files?.length) return
  e.preventDefault()
  void attachFiles(files)
}

function onPick(e: Event): void {
  const input = e.target as HTMLInputElement
  void attachFiles(input.files ?? [])
  input.value = ''
}

// ---- folding (below `sm` only; the classes carry the breakpoint) ----

const engaged = ref(false)
const idle = computed(
  () =>
    !engaged.value &&
    !!state.sessionId &&
    !editTarget.value &&
    !attachments.value.length &&
    !state.decisions.some((d) => d.session === state.sessionId),
)

/** A tap elsewhere in the app folds the sheet; floating panels (pickers,
 *  the lightbox) are teleported out of the app root and don't count. It
 *  listens for click, not pointerdown: a touch that turns into a scroll of
 *  the transcript is cancelled before it clicks, so reading keeps it open. */
function onTapAnywhere(e: MouseEvent): void {
  const root = rootEl.value
  const target = e.target as Node
  if (root && !root.contains(target) && root.closest('#__nuxt')?.contains(target)) engaged.value = false
}

/** Set by a press that unfolds the sheet: the footer grows under the finger
 *  before it lifts, so the click it ends in must not land on a footer key. */
let unfolding = false

function onPress(): void {
  unfolding = idle.value && matchMedia('(width < 40rem)').matches // `max-sm`
  engaged.value = true
}

function onClickCapture(e: MouseEvent): void {
  if (!unfolding) return
  unfolding = false
  if (e.target === textareaEl.value) return
  e.preventDefault()
  e.stopPropagation()
  textareaEl.value?.focus()
}

/** Unregisters this composer as the drop sink — and only this one. */
let dropSinkOff: (() => void) | undefined

onMounted(() => {
  // The window answers the drag (lib/state/drops), so a file dropped
  // anywhere in the app lands here.
  dropSinkOff = setDropSink((files) => void attachFiles(files))
  document.addEventListener('click', onTapAnywhere, true)
})

onBeforeUnmount(() => {
  dropSinkOff?.()
  document.removeEventListener('click', onTapAnywhere, true)
  endEdit()
  for (const item of subDraft?.attachments ?? []) releaseAttachment(item)
})

// ---- editing a sent message ----

function endEdit(): void {
  if (!editing.value) return
  for (const item of editing.value.attachments) releaseAttachment(item)
  editing.value = null
}

function userContent(node: TreeNode): Content {
  return (node.body as Extract<TreeNode['body'], { type: 'user' }>).content
}

function beginEdit(node: TreeNode): void {
  const content = userContent(node)
  const attachments = content.attachments ?? []
  editing.value = reactive({
    text: typedText(content.text, attachments),
    attachments: attachments.map((a) => storedAttachment(a)),
  })
  engaged.value = true
  void nextTick(() => {
    const el = textareaEl.value
    if (!el) return
    el.focus()
    el.setSelectionRange(el.value.length, el.value.length)
  })
}

watch(editTarget, (node) => {
  endEdit()
  if (node) beginEdit(node)
})

/** The message as it would be sent equals the edited one. */
function unchanged(node: TreeNode, text: string, sent: PromptAttachment[]): boolean {
  const content = userContent(node)
  const was = content.attachments ?? []
  return (
    text === content.text.trim() &&
    sent.length === was.length &&
    sent.every((a, i) => a.hash === was[i]!.hash)
  )
}

async function submit(): Promise<void> {
  if (!canSend.value) return
  const items = [...attachments.value]
  // Tiles that never made it stay behind; they are not part of the message.
  const done = items.filter((i) => i.status === 'done')
  const sent: PromptAttachment[] = done.map((i) => ({
    hash: i.attachment!.hash,
    name: i.name || undefined,
  }))
  // The anchors name each attachment by its kind, so the whole attachment
  // goes along, not just its hash.
  const sentAttachments: Attachment[] = done.map((i) => i.attachment!)
  const text = draft.value.trim()
  const target = editTarget.value
  const origin = current.value
  const landing = !state.sessionId
  // `/compact [focus]` compacts the shown branch instead.
  const compact = /^\/compact(?:\s+([\s\S]*))?$/.exec(text)
  if (compact && !target && !items.length && state.sessionId) {
    draft.value = ''
    if (!(await compactBranch(compact[1]?.trim() ?? '', view.branch.value))) draft.value = text
    return
  }
  // `/btw [question]` asks on the side instead, about the first quoted
  // passage if there is one; bare, it opens the side questions.
  const aside = /^\/btw(?:\s+([\s\S]*))?$/.exec(text)
  if (aside && !target && !items.length && state.sessionId) {
    const question = aside[1]?.trim() ?? ''
    const quote = quotes.value.shift() ?? null
    draft.value = ''
    openSide(view.aside, null, quote)
    if (question && !(await askSide(view.aside, question))) draft.value = text
    return
  }
  // Quoted passages lead the message, as markdown quotes.
  const quoted = quotes.value
  const body = withLeadingAnchors(
    [...quoted.map(quoteBlock), text].filter(Boolean).join('\n\n'),
    sentAttachments,
  )
  if (target && unchanged(target, body, sent)) {
    editTarget.value = null
    return
  }
  draft.value = ''
  quotes.value = []
  attachments.value = []
  const ok = target
    ? await editPrompt(target, body, sent)
    : await sendPrompt(body, sent, view.sub ? { model: view.model.value } : undefined, view.branch.value)
  if (ok) {
    for (const item of items) releaseAttachment(item)
    if (target && editTarget.value === target) editTarget.value = null
    return
  }
  // Put everything back so nothing is lost: into the edit if it is still on,
  // else where it was written — or, from the landing page, into the session
  // that may have been created before the prompt failed.
  const back = target
    ? editTarget.value === target
      ? editing.value
      : null
    : landing
      ? activeDraft.value
      : origin
  if (!back) {
    for (const item of items) releaseAttachment(item)
    return
  }
  back.text = text
  back.attachments = items
  quotes.value = quoted
}

// ---- taking back a queued prompt ----

/** Take a steering prompt the turn has not written yet (the latest one by
 *  default) back into the writing, ahead of what is written there; an edit
 *  in progress is dropped for it. */
async function takeBack(id = queuedPrompts.value.at(-1)?.id): Promise<void> {
  if (!id) return
  const content = await takeBackPrompt(id, view.branch.value)
  if (content) restore([content])
}

/** Stop the turn; the prompts it had queued come back into the writing. */
async function stop(): Promise<void> {
  const back = await stopTurn(view.branch.value)
  if (back.length) restore(back)
}

/** Put sent `contents` back into the writing, ahead of what is written
 *  there; an edit in progress is dropped for them. */
function restore(contents: Content[]): void {
  const into = subDraft ?? activeDraft.value
  if (editTarget.value) editTarget.value = null
  const attachments = contents.flatMap((c) => c.attachments ?? [])
  const text = contents
    .map((c) => typedText(c.text, c.attachments ?? []))
    .filter((s) => s.trim())
    .join('\n\n')
  into.text = [text, into.text].filter((s) => s.trim()).join('\n\n')
  into.attachments = [...attachments.map((a) => storedAttachment(a)), ...into.attachments]
  engaged.value = true
  void nextTick(() => {
    const el = textareaEl.value
    if (!el || into !== current.value) return
    el.focus()
    el.setSelectionRange(text.length, text.length)
  })
}

defineExpose({ takeBack })

function onKeydown(e: KeyboardEvent): void {
  if (e.key === 'Escape' && editTarget.value) {
    e.preventDefault()
    editTarget.value = null
    return
  }
  // ↑ at the very start of the writing takes the latest queued prompt back.
  if (
    e.key === 'ArrowUp' &&
    !e.isComposing &&
    !(e.shiftKey || e.altKey || e.ctrlKey || e.metaKey) &&
    !editTarget.value &&
    queuedPrompts.value.length
  ) {
    const el = e.target as HTMLTextAreaElement
    if (el.selectionStart === 0 && el.selectionEnd === 0) {
      e.preventDefault()
      void takeBack()
      return
    }
  }
  if (e.key !== 'Enter' || e.isComposing) return
  // Only a bare Enter sends; Shift and Alt break the line instead.
  if (!e.shiftKey && !e.altKey) {
    e.preventDefault()
    submit()
  } else if (e.altKey) {
    e.preventDefault()
    breakLine()
  }
}

/** Browsers give Shift+Enter a newline by themselves, Alt+Enter none: it is
 *  written here at the caret. */
function breakLine(): void {
  const el = textareaEl.value
  if (!el) return
  const { selectionStart: at, selectionEnd: to } = el
  draft.value = `${draft.value.slice(0, at)}\n${draft.value.slice(to)}`
  void nextTick(() => el.setSelectionRange(at + 1, at + 1))
}

/** Opening the landing page or a session puts the caret in the composer, the
 *  one thing to touch on either. Touch keyboards pop up on focus, so there it
 *  waits for a tap. The caret goes to the end of whatever draft is already
 *  there, so typing continues the thought. */
function focusComposer(): void {
  if (matchMedia('(pointer: coarse)').matches) return
  const el = textareaEl.value
  if (!el || document.activeElement === el) return
  el.focus()
  el.setSelectionRange(el.value.length, el.value.length)
}

onMounted(() => void nextTick(focusComposer))
// Landing → chat mounts a fresh composer, but switching between open sessions
// keeps this one mounted, so session changes focus it too. The sidebar's home
// and new-session keys land on the landing page without a session change when
// it is already open, so their request focuses it here.
if (!view.sub) {
  watch(
    () => state.sessionId,
    () => void nextTick(focusComposer),
  )
  watch(composerFocus, () => void nextTick(focusComposer))
}
// a quote just went into this composer
watch(view.aside.focus, () => void nextTick(focusComposer))

/** Whom the message goes to: the sub-agent shown, the session's assistant,
 * or the one picked on the landing page. */
const recipient = computed(() => {
  if (view.sub) return t('chat.agentView.title')
  const id = state.session?.assistant ?? landingAssistant.value
  return state.assistants.find((a) => a.id === id)?.name ?? 'rumu'
})

const placeholder = computed(() => {
  const name = recipient.value
  return isBusy.value
    ? t('chat.composer.steerPlaceholder', { name })
    : t('chat.composer.placeholder', { name })
})

const sendLabel = computed(() =>
  editTarget.value
    ? t('chat.user.resend')
    : isBusy.value
      ? t('chat.composer.steer')
      : t('chat.composer.send'),
)
</script>

<template>
  <!-- the footer's empty stretch is part of the page: clicking it writes -->
  <div
    ref="rootEl"
    class="composer relative mx-auto flex w-full max-w-[var(--col-max)] min-h-0 flex-col rounded-sheet"
    :class="{ 'is-drag': dragging, 'is-btw': btw }"
    @pointerdown="onPress"
    @click.capture="onClickCapture"
    @focusin="engaged = true"
    @click.self="textareaEl?.focus()"
  >
    <DecisionPanel v-if="!view.sub" />
    <div v-if="editTarget" class="flex items-center gap-2 pt-2 pr-2 pl-4 text-meta text-ink-2">
      <Icon name="edit" :size="13" />
      <span class="flex-1">{{ t('chat.user.edit') }}</span>
      <button
        class="icon-btn size-6"
        :title="t('chat.user.cancel')"
        :aria-label="t('chat.user.cancel')"
        @click="editTarget = null"
      >
        <Icon name="close" :size="13" />
      </button>
    </div>
    <div
      v-if="attachments.length"
      class="flex flex-wrap items-center gap-2 px-4"
      :class="editTarget ? 'pt-2' : 'pt-3'"
      role="group"
      :aria-label="t('chat.composer.tray')"
    >
      <div v-for="(item, i) in attachments" :key="item.id" class="relative flex items-center">
        <button
          v-if="picture(item)"
          type="button"
          class="block rounded-control transition-opacity hover:opacity-90"
          :title="item.name || t('chat.image.open')"
          :aria-label="t('chat.image.open')"
          @click="preview(item)"
        >
          <img
            :src="item.url"
            :alt="item.name"
            class="size-14 rounded-control border border-line bg-surface-2 object-cover"
            :class="{
              'opacity-60': item.status === 'uploading',
              'border-danger/60': item.status === 'failed',
            }"
          />
        </button>
        <DocumentChip
          v-else
          class="h-14"
          :class="{ 'opacity-60': item.status === 'uploading' }"
          :name="item.name"
          :size="sizeOf(item)"
        />
        <span
          v-if="item.status === 'uploading'"
          class="pointer-events-none absolute inset-0 flex items-center justify-center"
        >
          <span class="size-4 animate-spin rounded-full border-2 border-brand border-t-transparent" />
        </span>
        <button
          v-else-if="item.status === 'failed'"
          class="absolute inset-0 flex items-center justify-center rounded-control text-danger"
          :title="
            item.error ? `${t('chat.composer.retryUpload')} · ${item.error}` : t('chat.composer.retryUpload')
          "
          :aria-label="t('chat.composer.retryUpload')"
          @click="run(item)"
        >
          <Icon name="error" :size="14" />
        </button>
        <button
          class="absolute -top-1.5 -right-1.5 flex size-[18px] items-center justify-center rounded-full border border-line bg-surface text-ink-3 shadow-soft transition-colors hover:border-danger/50 hover:bg-danger-soft hover:text-danger"
          :title="t('chat.composer.removeAttachment')"
          :aria-label="t('chat.composer.removeAttachment')"
          @click="removeAt(i)"
        >
          <Icon name="close" :size="12" />
        </button>
      </div>
    </div>
    <QuoteTray />
    <textarea
      ref="textareaEl"
      v-model="draft"
      class="max-h-[240px] min-h-[calc(2lh+1rem)] resize-none rounded-none border-0 bg-transparent px-4 pb-1 text-message leading-normal transition-[min-height,max-height,padding] duration-300 ease-soft [field-sizing:content] focus:shadow-none"
      :class="[
        attachments.length || editTarget ? 'pt-2' : 'pt-3',
        idle &&
          'max-sm:max-h-[calc(1lh+1.25rem)] max-sm:min-h-[calc(1lh+1.25rem)] max-sm:overflow-hidden max-sm:py-2.5',
        idle && isBusy && 'max-sm:pr-12',
      ]"
      :placeholder="placeholder"
      @keydown="onKeydown"
      @paste="onPaste"
    />
    <!-- folded: only stopping stays a single press away -->
    <Transition
      enter-active-class="transition duration-200 ease-soft"
      enter-from-class="scale-75 opacity-0"
      leave-active-class="transition duration-100"
      leave-to-class="scale-75 opacity-0"
    >
      <button
        v-if="idle && isBusy"
        class="composer-stop absolute top-[calc(50%-16px)] right-1.5 sm:hidden"
        :title="t('chat.composer.stop')"
        :aria-label="t('chat.composer.stop')"
        @pointerdown.stop
        @click="stop"
      >
        <Icon name="stop" :size="16" />
      </button>
    </Transition>
    <div
      class="grid grid-rows-[1fr] transition-[grid-template-rows,opacity] duration-300 ease-soft"
      :class="idle && 'max-sm:grid-rows-[0fr] max-sm:opacity-0'"
    >
      <div class="min-h-0 overflow-hidden">
        <div
          class="flex min-w-0 items-center gap-1.5 pt-0.5 pr-2 pb-2 pl-3"
          @click.self="textareaEl?.focus()"
        >
          <ModelPicker />
          <ContextMeter />
          <span class="flex-1" @click.self="textareaEl?.focus()" />
          <input
            ref="fileEl"
            type="file"
            multiple
            class="hidden"
            :aria-label="t('chat.composer.attach')"
            @change="onPick"
          />
          <button
            class="composer-attach"
            :title="t('chat.composer.attach')"
            :aria-label="t('chat.composer.attach')"
            @click="fileEl?.click()"
          >
            <Icon name="attach" :size="16" />
          </button>
          <Transition
            enter-active-class="transition duration-200 ease-spring"
            enter-from-class="scale-75 opacity-0"
            leave-active-class="transition duration-100"
            leave-to-class="scale-75 opacity-0"
          >
            <button
              v-if="isBusy"
              class="composer-stop"
              :title="t('chat.composer.stop')"
              :aria-label="t('chat.composer.stop')"
              @click="stop"
            >
              <Icon name="stop" :size="16" />
            </button>
          </Transition>
          <button
            class="composer-send"
            :disabled="!canSend"
            :title="sendLabel"
            :aria-label="sendLabel"
            @click="submit"
          >
            <Icon name="send" :size="16" />
          </button>
        </div>
      </div>
    </div>
  </div>
</template>
