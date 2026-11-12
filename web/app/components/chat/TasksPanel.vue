<script setup lang="ts">
// The active branch's running background tasks, as a tab on the composer's
// top edge: one task shows itself; more fold into a count that opens the
// list. A row opens its sub-agent's transcript or the task's output beside
// the chat (TaskOutputPanel).
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { t } from '../../i18n'
import { killTask, openTaskDetail, runningTasks, state, taskChild } from '../../lib/store'
import { taskElapsed, taskLabel } from '../../lib/tasks'
import type { TaskInfo } from '../../lib/types'
import Icon from '../ui/Icon.vue'

/** Recompute elapsed times once a second. */
const now = ref(Date.now())
let timer: ReturnType<typeof setInterval> | undefined
onMounted(() => {
  timer = setInterval(() => (now.value = Date.now()), 1000)
})
onBeforeUnmount(() => clearInterval(timer))

const expanded = ref(false)
const folds = computed(() => runningTasks.value.length > 1)
const showRows = computed(() => !folds.value || expanded.value)

function isOpen(tk: TaskInfo): boolean {
  if (tk.kind.type !== 'agent') return state.taskDetail === tk.id
  const child = taskChild(tk.id)
  return !!child && state.agentView === child
}
</script>

<template>
  <!-- every part grows out of the composer's edge: 0fr ↔ 1fr rows -->
  <Transition name="fold">
    <div v-if="runningTasks.length" class="fold mx-auto w-full max-w-[var(--col-max)] px-5">
      <div>
        <div class="tasks-tab">
          <Transition name="fold">
            <div v-if="folds" class="fold">
              <div>
                <button type="button" class="tasks-row text-ink-2" @click="expanded = !expanded">
                  <span class="flex size-4 flex-none items-center justify-center">
                    <span class="dot-live size-1.5 rounded-full bg-brand" />
                  </span>
                  <span class="flex-1 truncate text-left">
                    {{ t('tools.tasks.running', { count: runningTasks.length }) }}
                  </span>
                  <Icon
                    name="down"
                    :size="14"
                    class="text-ink-3 transition-[rotate] duration-300 ease-soft"
                    :class="{ 'rotate-180': showRows }"
                  />
                </button>
              </div>
            </div>
          </Transition>

          <div class="fold" :class="{ 'is-shut': !showRows }" :inert="!showRows || undefined">
            <div>
              <TransitionGroup tag="div" name="fold" class="max-h-[40vh] overflow-y-auto">
                <div v-for="tk in runningTasks" :key="tk.id" class="fold">
                  <div>
                    <div class="tasks-row" :class="{ 'is-open': isOpen(tk) }">
                      <button
                        type="button"
                        class="flex min-w-0 flex-1 items-center gap-2"
                        :title="`${tk.id} · ${taskLabel(tk)}`"
                        @click="openTaskDetail(tk.id)"
                      >
                        <Icon
                          :name="tk.kind.type === 'agent' ? 'agent' : 'tool'"
                          :size="14"
                          :class="tk.kind.type === 'agent' ? 'text-agent' : 'text-tool'"
                        />
                        <span v-if="tk.kind.type === 'agent'" class="truncate text-ink">{{ tk.summary }}</span>
                        <span v-else class="truncate font-mono text-micro">
                          <span class="font-semibold text-tool">{{ tk.kind.name }}</span>
                          <span class="ml-1.5 text-ink-2">{{ tk.kind.args_summary }}</span>
                        </span>
                      </button>
                      <span
                        class="dot-live size-1.5 flex-none rounded-full bg-brand transition-opacity duration-300"
                        :class="{ 'opacity-0': folds }"
                      />
                      <span class="flex-none font-mono text-micro text-ink-3 tabular-nums">{{
                        taskElapsed(tk, now)
                      }}</span>
                      <button
                        type="button"
                        class="icon-btn stop size-5 hover:text-danger"
                        :title="t('tools.tasks.kill', { id: tk.id })"
                        :aria-label="t('tools.tasks.killAria', { id: tk.id })"
                        @click="killTask(tk.id)"
                      >
                        <Icon name="stop" :size="12" />
                      </button>
                    </div>
                  </div>
                </div>
              </TransitionGroup>
            </div>
          </div>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
/* A block that opens and shuts by its own height: a one-row grid whose row
 * eases between 0fr and 1fr, its only child clipped to it. Entering and
 * leaving (Transition name="fold") run the same way. */
.fold {
  display: grid;
  grid-template-rows: 1fr;
  transition:
    grid-template-rows 0.3s var(--ease-soft),
    opacity 0.25s var(--ease-soft);
}
.fold > * {
  min-height: 0;
  overflow: hidden;
}
.fold.is-shut,
.fold-enter-from,
.fold-leave-to {
  grid-template-rows: 0fr;
  opacity: 0;
}

/* a tab standing on the composer's top edge, inside its rounded corners */
.tasks-tab {
  display: flex;
  flex-direction: column;
  padding: 0 4px 6px;
  border: 1px solid var(--line);
  border-bottom: 0;
  border-radius: 12px 12px 0 0;
  background: color-mix(in srgb, var(--surface-2) 60%, var(--surface));
  font-size: var(--text-meta);
}
.tasks-row {
  display: flex;
  width: 100%;
  min-height: 30px;
  /* the gap above each row: a margin inside the fold's clip, so a folding
   * row takes its gap with it */
  margin-top: 4px;
  align-items: center;
  gap: 8px;
  padding: 0 6px 0 8px;
  border-radius: var(--radius-control);
  transition: background-color 0.15s;
}
.tasks-row:hover,
.tasks-row.is-open {
  background: color-mix(in srgb, var(--ink) 5%, transparent);
}

/* stop shows on the row under the pointer (always, without hover) */
@media (hover: hover) {
  .tasks-row .stop {
    opacity: 0;
    transition:
      opacity 0.15s,
      background-color 0.15s,
      color 0.15s;
  }
  .tasks-row:hover .stop,
  .tasks-row .stop:focus-visible {
    opacity: 1;
  }
}
</style>
