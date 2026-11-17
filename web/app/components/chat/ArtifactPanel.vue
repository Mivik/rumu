<script setup lang="ts">
// The open artifact: a sheet beside the chat on a wide screen, over it below.
// It shows one snapshot, steps through the versions of its file on the path,
// and flips between the rendered file and its source. A page runs in an
// opaque-origin frame with scripts and no network, so it can reach neither
// rumu's origin nor anything else; an SVG renders as an image, which runs no
// script at all. Images, audio, video and PDFs load straight from their blob;
// any other file only offers a download.
import { computed, ref, watch } from 'vue'
import { t } from '../../i18n'
import {
  artifactsOnPath,
  artifactText,
  artifactUrl,
  isTextKind,
  openArtifact,
  sandboxedPage,
  showArtifact,
} from '../../lib/artifacts'
import { renderMarkdown } from '../../lib/markdown'
import { toast } from '../../lib/state/ui'
import { copyText, fmtBytes, workspaceLabel } from '../../lib/util'
import Icon from '../ui/Icon.vue'
import FileView from './FileView.vue'

const shown = computed(() => openArtifact.value)
const artifact = computed(() => shown.value?.artifact ?? null)
const versions = computed(() =>
  artifact.value && shown.value?.version ? (artifactsOnPath.value.get(artifact.value.path) ?? []) : [],
)

const where = computed(() => {
  const path = artifact.value?.path ?? ''
  const i = path.indexOf(':')
  return i < 0 ? path : `${workspaceLabel(path.slice(0, i))} · ${path.slice(i + 1)}`
})

function step(by: number): void {
  const v = versions.value[(shown.value?.version ?? 1) - 1 + by]
  if (v) showArtifact(v.node.id)
}

// The snapshot's text, fetched once per blob; other kinds load from `url`.
const isText = computed(() => !!artifact.value && isTextKind(artifact.value.kind))
const url = computed(() => (artifact.value ? artifactUrl(artifact.value) : ''))
const text = ref<string | null>(null)
const failed = ref(false)
watch(
  () => artifact.value?.hash,
  (hash) => {
    text.value = null
    failed.value = false
    if (!hash || !isText.value) return
    artifactText(hash).then(
      (body) => {
        if (artifact.value?.hash === hash) text.value = body
      },
      () => {
        if (artifact.value?.hash === hash) failed.value = true
      },
    )
  },
  { immediate: true },
)

/** Code has nothing to render: it always shows as source. */
const renders = computed(() => isText.value && artifact.value?.kind !== 'code')
const source = ref(false)
const asSource = computed(() => source.value || !renders.value)

const page = computed(() => (text.value !== null ? sandboxedPage(text.value) : ''))
const svg = computed(() =>
  text.value !== null ? `data:image/svg+xml;charset=utf-8,${encodeURIComponent(text.value)}` : '',
)
const markdown = computed(() => renderMarkdown(text.value ?? '', { breaks: false }))

const size = computed(() => fmtBytes(artifact.value?.size ?? 0))

async function onCopy(): Promise<void> {
  if (text.value === null || !artifact.value) return
  const ok = await copyText(text.value)
  toast('info', ok ? t('chat.artifact.copied', { title: artifact.value.title }) : t('common.copyFailed'))
}

function onDownload(): void {
  const a = artifact.value
  if (!a || (isText.value && text.value === null)) return
  const href = isText.value
    ? URL.createObjectURL(new Blob([text.value!], { type: 'text/plain;charset=utf-8' }))
    : url.value
  const link = document.createElement('a')
  link.href = href
  link.download =
    a.path
      .slice(a.path.lastIndexOf(':') + 1)
      .split('/')
      .at(-1) || a.title
  link.click()
  if (isText.value) URL.revokeObjectURL(href)
}
</script>

<template>
  <section
    v-if="artifact && shown"
    class="sheet flex min-h-0 min-w-0 flex-col overflow-hidden rounded-sheet bg-surface"
  >
    <header class="flex min-h-14 flex-none items-center gap-2 border-b border-line-soft px-4">
      <span class="flex min-w-0 flex-col">
        <span class="truncate text-[15px] font-bold" :title="artifact.title">{{ artifact.title }}</span>
        <span class="truncate font-mono text-micro text-ink-3" :title="artifact.path">{{ where }}</span>
      </span>
      <span class="ml-auto flex flex-none items-center gap-0.5">
        <span
          v-if="versions.length > 1"
          class="mr-1 flex items-center font-mono text-micro text-ink-3"
          :aria-label="t('chat.artifact.versions')"
        >
          <button class="icon-btn size-6" :disabled="shown.version <= 1" @click="step(-1)">
            <Icon name="back" :size="13" />
          </button>
          <span class="tabular-nums">{{ shown.version }}/{{ versions.length }}</span>
          <button class="icon-btn size-6" :disabled="shown.version >= versions.length" @click="step(1)">
            <Icon name="forward" :size="13" />
          </button>
        </span>
        <button
          v-if="renders"
          class="icon-btn size-7"
          :class="{ 'is-on': source }"
          :title="source ? t('chat.artifact.rendered') : t('chat.artifact.source')"
          :aria-label="t('chat.artifact.source')"
          :aria-pressed="source"
          @click="source = !source"
        >
          <Icon name="code" :size="15" />
        </button>
        <button
          v-if="isText"
          class="icon-btn size-7"
          :title="t('chat.artifact.copy')"
          :aria-label="t('chat.artifact.copy')"
          @click="onCopy"
        >
          <Icon name="copy" :size="14" />
        </button>
        <button
          class="icon-btn size-7"
          :title="t('chat.artifact.download')"
          :aria-label="t('chat.artifact.download')"
          @click="onDownload"
        >
          <Icon name="download" :size="15" />
        </button>
        <button
          class="icon-btn size-7"
          :title="t('chat.artifact.close')"
          :aria-label="t('chat.artifact.close')"
          @click="showArtifact(null)"
        >
          <Icon name="close" :size="15" />
        </button>
      </span>
    </header>

    <div class="relative min-h-0 flex-1 overflow-auto">
      <p v-if="failed" class="p-6 text-meta text-danger">{{ t('chat.artifact.loadFailed') }}</p>
      <template v-else-if="text !== null">
        <FileView v-if="asSource" :text="text" :path="artifact.path" :args="null" fill />
        <iframe
          v-else-if="artifact.kind === 'html'"
          :key="artifact.hash"
          class="absolute inset-0 size-full bg-white"
          sandbox="allow-scripts allow-modals allow-downloads"
          referrerpolicy="no-referrer"
          :srcdoc="page"
          :title="artifact.title"
        />
        <div v-else-if="artifact.kind === 'svg'" class="flex min-h-full items-center justify-center p-6">
          <img :src="svg" :alt="artifact.title" class="max-h-full max-w-full" />
        </div>
        <div v-else class="md mx-auto max-w-[72ch] px-6 py-5" v-html="markdown" />
      </template>
      <div v-else-if="artifact.kind === 'image'" class="flex min-h-full items-center justify-center p-6">
        <img
          :key="artifact.hash"
          :src="url"
          :alt="artifact.title"
          class="max-h-full max-w-full"
          @error="failed = true"
        />
      </div>
      <div v-else-if="artifact.kind === 'audio'" class="flex min-h-full items-center justify-center p-6">
        <audio :key="artifact.hash" :src="url" controls class="w-full max-w-[480px]" @error="failed = true" />
      </div>
      <div v-else-if="artifact.kind === 'video'" class="absolute inset-0 flex items-center justify-center">
        <video
          :key="artifact.hash"
          :src="url"
          controls
          playsinline
          class="max-h-full max-w-full"
          @error="failed = true"
        />
      </div>
      <iframe
        v-else-if="artifact.kind === 'pdf'"
        :key="artifact.hash"
        class="absolute inset-0 size-full"
        :src="url"
        :title="artifact.title"
      />
      <div
        v-else-if="artifact.kind === 'file'"
        class="flex min-h-full flex-col items-center justify-center gap-3 p-6"
      >
        <Icon name="file" :size="40" class="text-ink-3" />
        <span class="flex max-w-full flex-col items-center">
          <span class="max-w-full truncate font-semibold">{{ artifact.title }}</span>
          <span class="font-mono text-micro text-ink-3">{{ size }}</span>
        </span>
        <button class="btn btn-primary" @click="onDownload">
          <Icon name="download" :size="15" />
          {{ t('chat.artifact.download') }}
        </button>
      </div>
    </div>
  </section>
</template>

<style scoped>
.icon-btn.is-on {
  background: var(--surface-2);
  color: var(--ink);
}
</style>
