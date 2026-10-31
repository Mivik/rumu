<script setup lang="ts">
// A message image: a thumbnail sized from the attachment's frozen
// dimensions (so nothing shifts as it loads), opening full-size on click.
// Where it sits is the parent's business.
import { computed } from 'vue'
import { t } from '../../i18n'
import type { Attachment } from '../../lib/types'
import { blobUrl } from '../../lib/attachments'
import { openLightbox } from '../../lib/store'

const props = withDefaults(defineProps<{ attachment: Attachment; maxH?: number; maxW?: number }>(), {
  maxH: 200,
  maxW: 320,
})

const size = computed(() => {
  const { width, height } = props.attachment
  const s = Math.min(1, props.maxH / height, props.maxW / width)
  return { w: Math.max(1, Math.round(width * s)), h: Math.max(1, Math.round(height * s)) }
})
</script>

<template>
  <button
    type="button"
    class="block max-w-full overflow-hidden rounded-control border border-line bg-surface-2 transition-opacity hover:opacity-90"
    :title="t('chat.image.open')"
    :aria-label="t('chat.image.open')"
    @click="openLightbox(attachment)"
  >
    <img
      :src="blobUrl(attachment.hash)"
      :alt="attachment.name ?? t('chat.image.alt')"
      :width="size.w"
      :height="size.h"
      class="block h-auto max-w-full"
      loading="lazy"
      decoding="async"
    />
  </button>
</template>
