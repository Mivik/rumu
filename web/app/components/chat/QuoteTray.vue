<script setup lang="ts">
// Above the composer's writing: the passages quoted into the message (picked
// from a selection in the conversation, SelectionBar, or brought back from a
// side question), sent as markdown quotes ahead of it.
import { t } from '../../i18n'
import { quotes } from '../../lib/state/side'
import Icon from '../ui/Icon.vue'
</script>

<template>
  <!-- each row opens and closes its own height, so the sheet grows and
       shrinks around it instead of jumping -->
  <TransitionGroup tag="div" name="tray" class="flex flex-col px-3">
    <div v-for="(q, i) in quotes" :key="`${q.node}:${q.text}`" class="tray-row">
      <div>
        <div class="flex min-w-0 items-start gap-2 pt-2 pl-1">
          <p class="line-clamp-2 min-w-0 flex-1 border-l-[3px] border-brand pl-2.5 text-meta text-ink-2">
            {{ q.text }}
          </p>
          <button
            class="icon-btn size-6 flex-none"
            :title="t('chat.side.unquote')"
            :aria-label="t('chat.side.unquote')"
            @click="quotes.splice(i, 1)"
          >
            <Icon name="close" :size="13" />
          </button>
        </div>
      </div>
    </div>
  </TransitionGroup>
</template>

<style scoped>
.tray-row {
  display: grid;
  grid-template-rows: 1fr;
}

.tray-row > div {
  min-height: 0;
  overflow: hidden;
}

.tray-enter-active,
.tray-leave-active {
  transition:
    grid-template-rows 0.28s var(--ease-soft),
    opacity 0.2s;
}

.tray-enter-from,
.tray-leave-to {
  grid-template-rows: 0fr;
  opacity: 0;
}
</style>
