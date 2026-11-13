<script setup lang="ts">
// The open artifact: a sheet beside the chat on a wide screen, over it below.
// It shows one snapshot, steps through the versions of its file on the path,
// and flips between the rendered file and its source. A page runs in an
// opaque-origin frame with scripts and no network, so it can reach neither
// rumu's origin nor anything else; an SVG renders as an image, which runs no
// script at all.
import { computed, ref, watch } from 'vue'
import { t } from '../../i18n'
import { artifactsOnPath, artifactText, openArtifact, sandboxedPage, showArtifact } from '../../lib/artifacts'
import { renderMarkdown } from '../../lib/markdown'
import { toast } from '../../lib/state/ui'
import { copyText, workspaceLabel } from '../../lib/util'
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

// The snapshot's text, fetched once per blob.
const text = ref<string | null>(null)
const failed = ref(false)
watch(
  () => artifact.value?.hash,
  (hash) => {
    text.value = null
    failed.value = false
    if (!hash) return
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
const renders = computed(() => !!artifact.value && artifact.value.kind !== 'code')
const source = ref(false)
const asSource = computed(() => source.value || !renders.value)

const page = computed(() => (text.value !== null ? sandboxedPage(text.value) : ''))
const svg = computed(() =>
  text.value !== null ? `data:image/svg+xml;charset=utf-8,${encodeURIComponent(text.value)}` : '',
)
const markdown = computed(() => renderMarkdown(text.value ?? '', { breaks: false }))

async function onCopy(): Promise<void> {
  if (text.value === null || !artifact.value) return
  const ok = await copyText(text.value)
  toast('info', ok ? t('chat.artifact.copied', { title: artifact.value.title }) : t('common.copyFailed'))
}

function onDownload(): void {
  const a = artifact.value
  if (text.value === null || !a) return
  const url = URL.createObjectURL(new Blob([text.value], { type: 'text/plain;charset=utf-8' }))
  const link = document.createElement('a')
  link.href = url
  link.download =
    a.path
      .slice(a.path.lastIndexOf(':') + 1)
      .split('/')
      .at(-1) || a.title
  link.click()
  URL.revokeObjectURL(url)
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
    </div>
  </section>
</template>

<style scoped>
.icon-btn.is-on {
  background: var(--surface-2);
  color: var(--ink);
}
</style>
