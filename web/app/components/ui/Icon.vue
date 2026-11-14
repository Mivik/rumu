<script setup lang="ts">
// Material Design Icons, addressed by the app's own semantic names.
//
// The svg sits on the baseline of an inline-block that inherits the surrounding font and line
// height, so beside text it centres on that font's cap height under any flex alignment, with no
// per-site nudging. Icon-only containers set `line-height: 0` to centre it on its own box.
import {
  mdiAccountOutline,
  mdiApplicationOutline,
  mdiArrowLeft,
  mdiArrowRight,
  mdiArrowTopRight,
  mdiArrowUp,
  mdiCancel,
  mdiCardsOutline,
  mdiChartBoxOutline,
  mdiCheck,
  mdiChevronDown,
  mdiChevronRight,
  mdiClose,
  mdiCloseCircleOutline,
  mdiCodeTags,
  mdiCogOutline,
  mdiCommentQuestionOutline,
  mdiCreationOutline,
  mdiCompassOutline,
  mdiConsole,
  mdiContentCopy,
  mdiDotsHorizontal,
  mdiDownload,
  mdiEyeOutline,
  mdiFileDocumentOutline,
  mdiFileOutline,
  mdiFilePlusOutline,
  mdiFolderOutline,
  mdiFormatListBulleted,
  mdiHistory,
  mdiHomeOutline,
  mdiImageOutline,
  mdiLayersOutline,
  mdiLightbulbOutline,
  mdiLinkVariant,
  mdiMagnify,
  mdiMenu,
  mdiMessageTextOutline,
  mdiMusicNoteOutline,
  mdiNoteTextOutline,
  mdiPencilOutline,
  mdiPinOutline,
  mdiPlus,
  mdiPuzzleOutline,
  mdiRefresh,
  mdiSourceFork,
  mdiStop,
  mdiSwapHorizontal,
  mdiTimelineTextOutline,
  mdiTrashCanOutline,
  mdiVideoOutline,
  mdiWrenchOutline,
} from '@mdi/js'

defineOptions({ inheritAttrs: false })

const props = withDefaults(defineProps<{ name: string; size?: number }>(), { size: 14 })

const paths: Record<string, string> = {
  user: mdiAccountOutline,
  memory: mdiLayersOutline,
  pin: mdiPinOutline,
  link: mdiLinkVariant,
  mount: mdiFolderOutline,
  tool: mdiWrenchOutline,
  read: mdiEyeOutline,
  write: mdiFilePlusOutline,
  list: mdiFormatListBulleted,
  bash: mdiConsole,
  deck: mdiCardsOutline,
  switch: mdiSwapHorizontal,
  ask: mdiCommentQuestionOutline,
  mcp: mdiPuzzleOutline,
  regenerate: mdiRefresh,
  copy: mdiContentCopy,
  edit: mdiPencilOutline,
  trash: mdiTrashCanOutline,
  agent: mdiCompassOutline,
  fork: mdiSourceFork,
  blocked: mdiCancel,
  error: mdiCloseCircleOutline,
  ok: mdiCheck,
  note: mdiNoteTextOutline,
  history: mdiHistory,
  file: mdiFileOutline,
  external: mdiArrowTopRight,
  close: mdiClose,
  menu: mdiMenu,
  more: mdiDotsHorizontal,
  chevron: mdiChevronRight,
  down: mdiChevronDown,
  stats: mdiChartBoxOutline,
  search: mdiMagnify,
  plus: mdiPlus,
  send: mdiArrowUp,
  up: mdiArrowUp,
  back: mdiArrowLeft,
  forward: mdiArrowRight,
  home: mdiHomeOutline,
  image: mdiImageOutline,
  audio: mdiMusicNoteOutline,
  video: mdiVideoOutline,
  document: mdiFileDocumentOutline,
  stop: mdiStop,
  brain: mdiLightbulbOutline,
  model: mdiCreationOutline,
  settings: mdiCogOutline,
  chat: mdiMessageTextOutline,
  trace: mdiTimelineTextOutline,
  artifact: mdiApplicationOutline,
  code: mdiCodeTags,
  download: mdiDownload,
}
</script>

<template>
  <span class="icon">
    <svg
      v-bind="$attrs"
      :width="size"
      :height="size"
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
      :style="{ verticalAlign: `calc((1cap - ${size}px) / 2)` }"
    >
      <path :d="paths[props.name] ?? paths.note" />
    </svg>
  </span>
</template>

<style scoped>
.icon {
  display: inline-block;
  flex: none;
}

.icon > svg {
  display: inline; /* preflight makes svg block, which ignores vertical-align */
}
</style>
