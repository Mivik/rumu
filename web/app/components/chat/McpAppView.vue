<script setup lang="ts">
// An MCP App view (lib/mcpApps.ts): the page a tool's server gave for its
// result, in the turn that called it, as tall as the page reports.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { t } from '../../i18n'
import type { Node } from '../../lib/types'
import { AppBridge, MAX_HEIGHT, appCall, appOf, appPage, viewHeights as heights } from '../../lib/mcpApps'
import { branchOf } from '../../lib/widgets'

const props = defineProps<{ node: Node }>()

const app = computed(() => (props.node.body.type === 'tool_result' ? appOf(props.node.body.content) : null))

const box = ref<HTMLElement | null>(null)
const frame = ref<HTMLIFrameElement | null>(null)
const height = ref(heights.get(props.node.id) ?? 240)
const failed = ref<string | null>(null)
let bridge: AppBridge | null = null
let themes: MutationObserver | null = null

onMounted(async () => {
  const a = app.value
  if (!a) return
  try {
    const [page, call] = await Promise.all([appPage(a), appCall(a)])
    if (!frame.value || !box.value) return
    bridge = new AppBridge({
      frame: frame.value,
      box: box.value,
      app: a,
      call,
      node: props.node.id,
      branch: () => (box.value ? branchOf(box.value) : null),
      onHeight: (h) => {
        heights.set(props.node.id, h)
        height.value = h
      },
    })
    frame.value.srcdoc = page
    themes = new MutationObserver(() => bridge?.themeChanged())
    themes.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })
  } catch (err) {
    failed.value = (err as Error).message
  }
})

onBeforeUnmount(() => {
  themes?.disconnect()
  bridge?.close()
})
</script>

<template>
  <div
    v-if="app"
    ref="box"
    :data-path-node="node.id"
    class="mcp-app"
    :class="{ 'is-bare': app.prefers_border === false }"
  >
    <iframe
      v-if="!failed"
      ref="frame"
      sandbox="allow-scripts allow-forms"
      referrerpolicy="no-referrer"
      :title="t('chat.mcpApp.title', { tool: `${app.server}/${app.tool}` })"
      :style="{ height: `${Math.min(height, MAX_HEIGHT)}px` }"
    />
    <div v-else class="px-3 py-2 text-meta text-danger">{{ t('chat.mcpApp.failed', { error: failed }) }}</div>
  </div>
</template>

<style scoped>
.mcp-app {
  overflow: hidden;
  border: 1px solid var(--line);
  border-radius: var(--radius-control);
  background: var(--surface);
}
.mcp-app.is-bare {
  border-color: transparent;
  background: transparent;
}
.mcp-app > iframe {
  display: block;
  width: 100%;
  border: 0;
  background: transparent;
}
.mcp-app:fullscreen {
  border: 0;
  border-radius: 0;
  background: var(--surface);
}
.mcp-app:fullscreen > iframe {
  height: 100% !important;
}
</style>
