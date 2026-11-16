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
//
// A tool call waiting on the user's approval unfolds from the top of the
// sheet (DecisionPanel), above the writing. The sheet is a column that may
// shrink (min-h-0): a request taller than the room the sheet has scrolls
// inside its panel instead of growing the sheet past the window's foot.
//
// Images attach three ways — paste, drop, the footer's picture key — and
// upload the moment they arrive, into the tray above the writing; a failed
// upload keeps its tile (the server's word arrived as a toast) until retried
// or removed. The writing stays plain text: like the Claude and ChatGPT web
// apps, the sent message leads with its images (`withLeadingAnchors`).
//
// On a phone the sheet folds to one line while you are elsewhere — a tap
// on the transcript or the sidebar, not a scroll — and unfolds when touched
// again. It stays
// open while it holds images or a decision.
//
// Text selected in a reply offers itself above the writing (QuoteTray): as a
// quote leading the message, or as a side question. `/btw` asks on the side
// too (SidePop); the sheet's edge turns the side's teal while it would.
//
// Editing a sent message (`editTarget`) loads its text and images here, the
// draft set aside until the edit is resent or cancelled; sending forks from
// just before the message (store.editPrompt).
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { t } from '../../i18n'
import {
  compactBranch,
  editPrompt,
  isBusy,
  landingAssistant,
  sendPrompt,
  state,
  stopTurn,
} from '../../lib/store'
import type { PromptAttachment } from '../../lib/store'
import type { Attachment, Content, Node as TreeNode } from '../../lib/types'
import { blobUrl, typedText, uploadBlob, withLeadingAnchors } from '../../lib/attachments'
import { composerFocus, editTarget, openLightbox, toast } from '../../lib/state/ui'
import { askSide, openSide, quoteBlock, quotes } from '../../lib/state/side'
import Icon from '../ui/Icon.vue'
import DecisionPanel from './DecisionPanel.vue'
import ModelPicker from './ModelPicker.vue'
import QuoteTray from './QuoteTray.vue'

/** One image being attached: the file, its local preview, and where its
 * upload stands. An image the edited message already carries has no file. */
interface PendingImage {
  id: number
  file?: File
  name: string
  /** Object URL of the file's bytes, or the stored blob, for the tray's preview. */
  url: string
  status: 'uploading' | 'done' | 'failed'
  attachment?: Attachment
  error?: string
  ctrl: AbortController
}

const MAX_IMAGES = 20

const draft = ref('')
const attachments = ref<PendingImage[]>([])
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

let attachSeq = 0

async function run(item: PendingImage): Promise<void> {
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

function attachFiles(list: FileList | File[]): void {
  const files = [...list]
  const images = files.filter((f) => f.type.startsWith('image/'))
  if (images.length < files.length) toast('error', t('chat.composer.notAnImage'))
  const room = MAX_IMAGES - attachments.value.length
  if (images.length > room) {
    toast('error', t('chat.composer.tooManyImages', { max: MAX_IMAGES }))
    images.length = Math.max(0, room)
  }
  if (!images.length) return
  for (const file of images) {
    const item = reactive<PendingImage>({
      id: ++attachSeq,
      file,
      name: file.name,
      url: URL.createObjectURL(file),
      status: 'uploading',
      ctrl: new AbortController(),
    })
    attachments.value.push(item)
    void run(item)
  }
}

/** Show a tile full-size: the stored image once uploaded, the local
 *  preview before that. */
function preview(item: PendingImage): void {
  openLightbox(item.attachment ?? { src: item.url, name: item.name || null })
}

/** Drop a tile: stop its upload and free its local preview. */
function release(item: PendingImage): void {
  item.ctrl.abort()
  if (item.file) URL.revokeObjectURL(item.url)
}

function removeAt(i: number): void {
  const item = attachments.value[i]
  if (!item) return
  release(item)
  attachments.value.splice(i, 1)
}

function onPaste(e: ClipboardEvent): void {
  const files = e.clipboardData?.files
  if (!files?.length) return
  e.preventDefault()
  attachFiles(files)
}

// ---- drop target (a counter: dragleave fires per child element) ----

const dragDepth = ref(0)

function hasFiles(e: DragEvent): boolean {
  return !!e.dataTransfer?.types?.includes('Files')
}

function onDragenter(e: DragEvent): void {
  if (!hasFiles(e)) return
  e.preventDefault()
  dragDepth.value++
}

function onDragover(e: DragEvent): void {
  if (hasFiles(e)) e.preventDefault()
}

function onDragleave(): void {
  dragDepth.value = Math.max(0, dragDepth.value - 1)
}

function onDrop(e: DragEvent): void {
  if (!hasFiles(e)) return
  e.preventDefault()
  dragDepth.value = 0
  const files = e.dataTransfer?.files
  if (files?.length) attachFiles(files)
}

function onPick(e: Event): void {
  const input = e.target as HTMLInputElement
  attachFiles(input.files ?? [])
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

onMounted(() => document.addEventListener('click', onTapAnywhere, true))

onBeforeUnmount(() => {
  document.removeEventListener('click', onTapAnywhere, true)
  for (const item of [...attachments.value, ...(stash?.attachments ?? [])]) release(item)
})

// ---- editing a sent message ----

/** The draft set aside while a message is edited. */
let stash: { draft: string; attachments: PendingImage[] } | null = null

function endEdit(): void {
  if (!stash) return
  for (const item of attachments.value) release(item)
  draft.value = stash.draft
  attachments.value = stash.attachments
  stash = null
}

function userContent(node: TreeNode): Content {
  return (node.body as Extract<TreeNode['body'], { type: 'user' }>).content
}

function beginEdit(node: TreeNode): void {
  const content = userContent(node)
  const images = content.attachments ?? []
  stash = { draft: draft.value, attachments: attachments.value }
  draft.value = typedText(content.text, images.length)
  attachments.value = images.map((a) =>
    reactive<PendingImage>({
      id: ++attachSeq,
      name: a.name ?? '',
      url: blobUrl(a.hash),
      status: 'done',
      attachment: a,
      ctrl: new AbortController(),
    }),
  )
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
  const sent: PromptAttachment[] = items
    .filter((i) => i.status === 'done')
    .map((i) => ({ hash: i.attachment!.hash, name: i.name || undefined }))
  const text = draft.value.trim()
  const target = editTarget.value
  // `/compact [focus]` compacts the open session's branch instead.
  const compact = /^\/compact(?:\s+([\s\S]*))?$/.exec(text)
  if (compact && !target && !items.length && state.sessionId) {
    draft.value = ''
    if (!(await compactBranch(compact[1]?.trim() ?? ''))) draft.value = text
    return
  }
  // `/btw [question]` asks on the side instead, about the first quoted
  // passage if there is one; bare, it opens the side questions.
  const aside = /^\/btw(?:\s+([\s\S]*))?$/.exec(text)
  if (aside && !target && !items.length && state.sessionId) {
    const question = aside[1]?.trim() ?? ''
    const quote = quotes.value.shift() ?? null
    draft.value = ''
    openSide(null, quote)
    if (question && !(await askSide(question))) draft.value = text
    return
  }
  // Quoted passages lead the message, as markdown quotes.
  const quoted = quotes.value
  const body = withLeadingAnchors([...quoted.map(quoteBlock), text].filter(Boolean).join('\n\n'), sent.length)
  if (target && unchanged(target, body, sent)) {
    editTarget.value = null
    return
  }
  draft.value = ''
  quotes.value = []
  attachments.value = []
  // Put everything back if creating/sending failed so nothing is lost.
  const ok = target ? await editPrompt(target, body, sent) : await sendPrompt(body, sent)
  if (ok) {
    for (const item of items) release(item)
    if (target && editTarget.value === target) editTarget.value = null
  } else if (editTarget.value === target) {
    draft.value = text
    quotes.value = quoted
    attachments.value = items
  } else {
    for (const item of items) release(item)
  }
}

function onKeydown(e: KeyboardEvent): void {
  if (e.key === 'Escape' && editTarget.value) {
    e.preventDefault()
    editTarget.value = null
    return
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
// keeps this one mounted, so session changes focus it too.
watch(
  () => state.sessionId,
  () => void nextTick(focusComposer),
)
// The sidebar's home and new-session keys land on the landing page without a
// session change when it is already open, so their request focuses it here.
watch(composerFocus, () => void nextTick(focusComposer))

/** Whom the message goes to: the session's assistant, or the one picked on
 * the landing page. */
const recipient = computed(() => {
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
    :class="{ 'is-drag': dragDepth > 0, 'is-btw': btw }"
    @pointerdown="onPress"
    @click.capture="onClickCapture"
    @focusin="engaged = true"
    @click.self="textareaEl?.focus()"
    @dragenter="onDragenter"
    @dragover="onDragover"
    @dragleave="onDragleave"
    @drop="onDrop"
  >
    <DecisionPanel />
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
    <div v-if="attachments.length" class="flex flex-wrap gap-2 px-4" :class="editTarget ? 'pt-2' : 'pt-3'">
      <div v-for="(item, i) in attachments" :key="item.id" class="relative">
        <button
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
          :title="t('chat.composer.removeImage')"
          :aria-label="t('chat.composer.removeImage')"
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
        @click="stopTurn()"
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
          <span class="flex-1" @click.self="textareaEl?.focus()" />
          <input
            ref="fileEl"
            type="file"
            accept="image/*"
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
            <Icon name="image" :size="16" />
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
              @click="stopTurn()"
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
