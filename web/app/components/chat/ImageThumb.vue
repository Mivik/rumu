<script setup lang="ts">
// A message image: a thumbnail sized from the attachment's frozen
// dimensions (so nothing shifts as it loads), opening full-size on click.
// Where it sits is the parent's business.
//
// The box's aspect is clamped to [1/MAX_ASPECT, MAX_ASPECT]: a long
// screenshot or a panorama would otherwise shrink to a sliver. What falls
// outside is cropped (object-cover, anchored at the top, where a long page
// starts); the lightbox shows the whole. `tile` gives the box a fixed height
// of maxH (width from the clamped aspect, up to maxW), so several images line
// up as one even row; otherwise the image fits within maxH × maxW and is
// never enlarged.
import { computed } from 'vue'
import { t } from '../../i18n'
import type { Attachment } from '../../lib/types'
import { blobUrl } from '../../lib/attachments'
import { openLightbox } from '../../lib/store'

const props = withDefaults(
  defineProps<{ attachment: Attachment; maxH?: number; maxW?: number; tile?: boolean }>(),
  { maxH: 200, maxW: 320, tile: false },
)

const MAX_ASPECT = 2.5

const size = computed(() => {
  const { width, height } = props.attachment
  const r = Math.min(MAX_ASPECT, Math.max(1 / MAX_ASPECT, width / height))
  if (props.tile) {
    const w = Math.min(props.maxW, Math.round(props.maxH * r))
    return { w, h: props.maxH }
  }
  // The uncropped part of the image, at natural scale.
  const cw = Math.min(width, height * r)
  const ch = Math.min(height, width / r)
  const s = Math.min(1, props.maxH / ch, props.maxW / cw)
  return { w: Math.max(1, Math.round(cw * s)), h: Math.max(1, Math.round(ch * s)) }
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
    <!-- an explicit aspect-ratio: once loaded, h-auto would otherwise follow the image's own -->
    <img
      :src="blobUrl(attachment.hash)"
      :alt="attachment.name ?? t('chat.image.alt')"
      :width="size.w"
      :height="size.h"
      :style="{ aspectRatio: `${size.w} / ${size.h}` }"
      class="block h-auto max-w-full object-cover object-top"
      loading="lazy"
      decoding="async"
    />
  </button>
</template>
