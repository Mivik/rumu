<script setup lang="ts">
// Landing hero (no session open): the wordmark writes itself in, then a short
// prompt. The composer sits right below it, in the middle of the sheet — on a
// phone, at the page's foot.
import { t } from '../../i18n'
import { connected } from '../../lib/store'
import RumuMark from '../brand/RumuMark.vue'
</script>

<template>
  <div class="reveal flex flex-col items-center text-center">
    <RumuMark class="text-ink" :height="44" draw :offline="!connected" />
    <p class="mt-4 text-secondary text-ink-3">{{ t('chat.welcome') }}</p>
  </div>
</template>
