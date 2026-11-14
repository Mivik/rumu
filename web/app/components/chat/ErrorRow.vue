<script setup lang="ts">
// The row shown where an owed reply would be: the branch's last turn failed
// and never got its answer. Shaped like an assistant turn — the mascot heads
// it on its own line — so the failure reads as the reply that is missing, not
// as a floating banner. It comes from the branch's recorded failure while the
// reply is still owed (`owedReply`), so it survives a reload; answered
// failures and hook-refused prompts stay quiet. Retry runs the turn again in
// place.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { catOf } from '../../lib/assistantFace'
import type { OwedReply } from '../../lib/store'
import { retryBranch, state } from '../../lib/store'
import { vEnter } from '../../lib/motion'
import RumuCat from '../brand/RumuCat.vue'

defineProps<{ owed: OwedReply }>()

/** The mascot's "x u x": eyes folded shut (the squint fold) and a greyed
 * mouth. Everything else stays at rest, so the head is the one every other
 * turn shows — only the face carries the failure. */
const sadPose = { squint: 1, mouthGrey: 0.5 }
const cat = computed(() => (state.session ? catOf(state.assistants, state.session.assistant) : undefined))

const retrying = ref(false)
async function onRetry(): Promise<void> {
  if (retrying.value) return
  retrying.value = true
  try {
    await retryBranch()
  } finally {
    retrying.value = false
  }
}
</script>

<template>
  <div class="flex min-w-0 flex-col gap-2">
    <!-- the turn's head, as on any assistant turn -->
    <div class="tl-head">
      <RumuCat :pose="sadPose" :cat="cat" :size="52" :paper="false" class="-ml-[6px]" />
    </div>

    <div
      v-enter="'enter-fade'"
      class="flex min-w-0 items-center gap-2.5 rounded-control border border-danger/35 bg-danger-soft py-1.5 pr-1.5 pl-3"
    >
      <span class="min-w-0 flex-1 truncate text-meta text-danger" :title="owed.message || undefined">
        {{ owed.message || t('chat.error.interrupted') }}
      </span>
      <button
        class="flex-none rounded-xs border border-danger/40 px-2 py-0.5 text-micro font-semibold text-danger transition-colors hover:bg-danger/10 disabled:opacity-50"
        :disabled="retrying"
        @click="onRetry"
      >
        {{ t('chat.error.retry') }}
      </button>
    </div>
  </div>
</template>
