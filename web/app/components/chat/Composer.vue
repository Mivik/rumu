<script setup lang="ts">
// The message composer. It lives at the bottom of the chat, and at the center
// of the landing state — in which case sending the first message creates the
// session (store.sendPrompt does that).
//
// A sheet of paper with the writing on top and a footer that says where the
// prompt will go: the branch's default workspace (where its commands run —
// it opens the list of all workspaces) and the model it runs on, on the
// left; its reasoning effort and the send button on the right — solid coral once there is something
// to send. While rumu works, sending steers the turn and a stop button
// joins it.
//
// A tool call waiting on the user's approval unfolds from the top of the
// sheet (DecisionPanel), above the writing.
//
// Images attach three ways — paste, drop, the footer's picture key — and
// upload the moment they arrive, into the tray above the writing; a failed
// upload keeps its tile (the server's word arrived as a toast) until retried
// or removed. The writing stays plain text: like the Claude and ChatGPT web
// apps, the sent message leads with its images (`withLeadingAnchors`).
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { t } from '../../i18n'
import { composerWorkspaces, isBusy, sendPrompt, state, stopTurn } from '../../lib/store'
import type { PromptAttachment } from '../../lib/store'
import type { Attachment } from '../../lib/types'
import { uploadBlob, withLeadingAnchors } from '../../lib/attachments'
import { openLightbox, toast } from '../../lib/state/ui'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'
import WorkspacesPanel from '../WorkspacesPanel.vue'
import DecisionPanel from './DecisionPanel.vue'
import EffortPicker from './EffortPicker.vue'
import ModelPicker from './ModelPicker.vue'

/** One image being attached: the file, its local preview, and where its
 * upload stands. */
interface PendingImage {
  id: number
  file: File
  /** Object URL of the original bytes, for the tray's preview. */
  url: string
  status: 'uploading' | 'done' | 'failed'
  attachment?: Attachment
  error?: string
  ctrl: AbortController
}

const MAX_IMAGES = 20

const draft = ref('')
const attachments = ref<PendingImage[]>([])
const textareaEl = ref<HTMLTextAreaElement>()
const fileEl = ref<HTMLInputElement>()

const uploading = computed(() => attachments.value.some((a) => a.status === 'uploading'))
const canSend = computed(
  () => !uploading.value && (draft.value.trim() !== '' || attachments.value.some((a) => a.status === 'done')),
)

let attachSeq = 0

async function run(item: PendingImage): Promise<void> {
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
  openLightbox(item.attachment ?? { src: item.url, name: item.file.name || null })
}

function removeAt(i: number): void {
  const item = attachments.value[i]
  if (!item) return
  item.ctrl.abort()
  URL.revokeObjectURL(item.url)
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

onBeforeUnmount(() => {
  for (const item of attachments.value) {
    item.ctrl.abort()
    URL.revokeObjectURL(item.url)
  }
})

async function submit(): Promise<void> {
  if (!canSend.value) return
  const items = [...attachments.value]
  // Tiles that never made it stay behind; they are not part of the message.
  const sent: PromptAttachment[] = items
    .filter((i) => i.status === 'done')
    .map((i) => ({ hash: i.attachment!.hash, name: i.file.name || undefined }))
  const text = draft.value.trim()
  draft.value = ''
  attachments.value = []
  // Put everything back if creating/sending failed so nothing is lost.
  if (!(await sendPrompt(withLeadingAnchors(text, sent.length), sent))) {
    draft.value = text
    attachments.value = items
  } else {
    for (const item of items) URL.revokeObjectURL(item.url)
  }
}

function onKeydown(e: KeyboardEvent): void {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
    e.preventDefault()
    submit()
  }
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

const placeholder = computed(() => {
  return isBusy.value ? t('chat.composer.steerPlaceholder') : t('chat.composer.placeholder')
})

/** The chip: the default workspace (where it lives on hover), or the action
 *  to add one. `@rumu` is never shown. On the landing page it is the pick
 *  the new session starts with. */
const workspace = computed(() => {
  const ws = composerWorkspaces.value
  if (!ws) return null
  const own = ws.workspaces.find((w) => w.name === ws.default)
  if (own)
    return { name: own.name, title: own.ssh ? `${own.ssh}:${own.root}` : own.root, icon: 'mount', own: true }
  return ws.workspaces.length
    ? { name: t('workspaces.title'), title: '', icon: 'mount', own: false }
    : { name: t('workspaces.add'), title: '', icon: 'plus', own: false }
})
</script>

<template>
  <!-- the footer's empty stretch is part of the page: clicking it writes -->
  <div
    class="composer mx-auto flex w-full max-w-[var(--col-max)] flex-col rounded-sheet"
    :class="{ 'is-drag': dragDepth > 0 }"
    @click.self="textareaEl?.focus()"
    @dragenter="onDragenter"
    @dragover="onDragover"
    @dragleave="onDragleave"
    @drop="onDrop"
  >
    <DecisionPanel />
    <div v-if="attachments.length" class="flex flex-wrap gap-2 px-4 pt-3">
      <div v-for="(item, i) in attachments" :key="item.id" class="relative">
        <button
          type="button"
          class="block rounded-control transition-opacity hover:opacity-90"
          :title="item.file.name || t('chat.image.open')"
          :aria-label="t('chat.image.open')"
          @click="preview(item)"
        >
          <img
            :src="item.url"
            :alt="item.file.name"
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
    <textarea
      ref="textareaEl"
      v-model="draft"
      class="max-h-[240px] min-h-[calc(2lh+1rem)] resize-none rounded-none border-0 bg-transparent px-4 pb-1 text-message leading-normal [field-sizing:content] focus:shadow-none"
      :class="attachments.length ? 'pt-2' : 'pt-3'"
      :placeholder="placeholder"
      @keydown="onKeydown"
      @paste="onPaste"
    />
    <div class="flex min-w-0 items-center gap-1.5 pt-0.5 pr-2 pb-2 pl-3" @click.self="textareaEl?.focus()">
      <Popover v-if="workspace" :label="t('workspaces.title')" placement="top-start">
        <template #trigger="{ open, toggle }">
          <button
            type="button"
            class="inline-flex min-w-0 items-center gap-1.5 rounded-full bg-surface-2 py-1 pr-2 pl-2.5 text-meta font-semibold text-ink-2 transition-colors hover:text-ink"
            :class="{ 'text-ink': open }"
            :title="workspace.title"
            :aria-expanded="open"
            @click="toggle"
          >
            <Icon :name="workspace.icon" :size="14" class="text-tool" />
            <span class="truncate" :class="{ 'font-card': workspace.own }">{{ workspace.name }}</span>
            <Icon name="down" :size="12" class="text-ink-3" />
          </button>
        </template>
        <template #default="{ close }">
          <div class="panel-head">{{ t('workspaces.title') }}</div>
          <WorkspacesPanel @close="close" />
        </template>
      </Popover>
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
      <EffortPicker />
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
        :title="isBusy ? t('chat.composer.steer') : t('chat.composer.send')"
        :aria-label="isBusy ? t('chat.composer.steer') : t('chat.composer.send')"
        @click="submit"
      >
        <Icon name="send" :size="16" />
      </button>
    </div>
  </div>
</template>
