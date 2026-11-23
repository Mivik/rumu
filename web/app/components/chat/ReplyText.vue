<script setup lang="ts">
// A finished reply's text, rendered as markdown. Its own component so the
// HTML is a computed of the node: a rebuilt timeline (each node of a turn
// rebuilds it) hands the group new item objects but the same node, so this
// neither re-renders nor parses again. It renders on mount, and again when
// a grammar, KaTeX or an image the text names lands.
import { computed } from 'vue'
import type { Node } from '../../lib/types'
import { renderMarkdown } from '../../lib/markdown'
import { pathImage } from '../../lib/artifacts'
import { contentText } from '../../lib/util'

const props = defineProps<{ node: Node }>()

const html = computed(() =>
  props.node.body.type === 'assistant'
    ? renderMarkdown(contentText(props.node.body.content), {
        image: (a) => pathImage(a, props.node.id),
        widgets: true,
      })
    : '',
)
</script>

<template>
  <div :data-path-node="node.id" class="md" v-html="html" />
</template>
