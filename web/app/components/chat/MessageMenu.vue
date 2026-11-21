<script setup lang="ts">
// The more menu at the end of a message's actions. Copying its `@rumu`
// address gives the model and cards a handle on it:
// `@rumu:/sessions/<s>/<node>`, or the turn's `<from>..<to>` stretch.
import { t } from '../../i18n'
import { copyAddress, nodeAddress } from '../../lib/links'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'

const props = defineProps<{ from: string; to?: string }>()

const MENU_ITEM =
  'flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-secondary text-ink-2 transition-colors duration-150 hover:bg-surface-2 hover:text-ink'

function copy(close: () => void): void {
  close()
  void copyAddress(nodeAddress(props.from, props.to))
}
</script>

<template>
  <Popover menu role="menu" placement="bottom-end" :label="t('chat.message.more')">
    <template #trigger="{ open, toggle }">
      <button
        type="button"
        class="icon-btn size-5"
        :title="t('chat.message.more')"
        :aria-label="t('chat.message.more')"
        aria-haspopup="menu"
        :aria-expanded="open"
        @click="toggle"
      >
        <Icon name="more" :size="12" />
      </button>
    </template>
    <template #default="{ close }">
      <button type="button" role="menuitem" :class="MENU_ITEM" @click="copy(close)">
        <Icon name="link" :size="14" />
        {{ t('chat.message.copyAddress') }}
      </button>
    </template>
  </Popover>
</template>
