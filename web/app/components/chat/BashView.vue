<script setup lang="ts">
// The body of an expanded `bash` row, under the row's own `$ command` line:
// the output, and the exit code when it isn't 0. A background call passes
// its task's output and `running` while it goes. An output that never ran
// (a tool error, a hook block) reads in danger.
import { computed } from 'vue'
import { t } from '../../i18n'
import { bashExit } from '../../lib/toolrows'

const props = defineProps<{
  output: string
  /** The call did not succeed (a non-zero exit, a tool error, a block). */
  failed: boolean
  running?: boolean
}>()

const result = computed(() => {
  const { code, body } = bashExit(props.output)
  return { code, body: body.replace(/\n+$/, '') }
})
</script>

<template>
  <div class="term">
    <pre v-if="result.body" class="term-out" :class="{ 'text-danger': failed && result.code === null }">{{
      result.body
    }}</pre>
    <div v-else-if="!running" class="term-foot">{{ t('common.noOutput') }}</div>
    <div v-if="running" class="term-foot flex items-center gap-1.5">
      <span class="dot-live size-1.5 rounded-full bg-brand" />{{ t('tools.status.running') }}
    </div>
    <div v-else-if="result.code" class="term-foot term-exit">
      {{ t('tools.result.exit', { code: result.code }) }}
    </div>
  </div>
</template>

<style scoped>
/* Edge to edge under the row line, like FileDiff. */
.term {
  border-top: 1px solid var(--line-soft);
  background: var(--surface);
  font-family: var(--font-mono);
  font-size: var(--text-micro);
  line-height: 1.55;
}

.term-out {
  max-height: 320px;
  overflow: auto;
  margin: 0;
  padding: 4px 10px;
  font: inherit;
  white-space: pre-wrap;
  word-break: break-word;
}

.term-foot {
  padding: 3px 10px;
  color: var(--ink-3);
}

.term-out + .term-foot {
  border-top: 1px solid var(--line-soft);
}

.term-exit {
  color: var(--danger);
  font-weight: 600;
}
</style>
