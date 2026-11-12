<script setup lang="ts">
// One step's tool calls, inside an opened activity run: one outlined box,
// one terse log line per call, hairlines between:
//   [status icon] name · short target · short result · [card chip/status]
// Rows stay abbreviated — a basename, a clipped first line, a count; the
// full call/result lives in the row's tooltip and its own expansion
// (args/output). "ok" rows carry no status word (the icon suffices), errors
// keep theirs, and a call a hook blocked gets a small rubber stamp (stamped
// on, when it happens live) and starts expanded. `agent` calls are plain
// rows here: the AgentCard below the run is where a sub-agent shows.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import type { ToolRowInfo } from '../../lib/store'
import { openTaskDetail, state } from '../../lib/store'
import { cardById } from '../../lib/cards'
import type { ToolRowView } from '../../lib/toolrows'
import { argsOf, outputJson, toolRowTitle, toolRowView } from '../../lib/toolrows'
import { taskStatusWord } from '../../lib/tasks'
import { highlightLine } from '../../lib/markdown'
import { vEnter } from '../../lib/motion'
import Icon from '../ui/Icon.vue'
import Chip from '../ui/Chip.vue'
import CardChip from './CardChip.vue'
import CodeCalls from './CodeCalls.vue'
import BashView from './BashView.vue'
import FileDiff from './FileDiff.vue'
import FileView from './FileView.vue'
import ImageThumb from './ImageThumb.vue'
import JsonTree from './JsonTree.vue'

const props = defineProps<{ rows: ToolRowInfo[] }>()

const cardsMap = cardById

const views = computed<ToolRowView[]>(() => props.rows.map(toolRowView))

/** Rows already there when the box opened reveal with it; later ones slide in. */
const revealed = new Set(props.rows.map((r) => r.result.id))

const expanded = ref<Set<string>>(new Set())
/** Rows the user collapsed manually (blocked rows start expanded). */
const collapsed = ref<Set<string>>(new Set())

function isOpen(v: ToolRowView): boolean {
  const id = v.row.result.id
  if (collapsed.value.has(id)) return false
  return v.status === 'blocked' || expanded.value.has(id)
}

/** A string arg of the call: a `code` script, a `bash` command. */
function callArg(v: ToolRowView, key: string): string {
  const raw = argsOf(v.row.call)?.[key]
  return typeof raw === 'string' ? raw.trim() : ''
}

/** A bash row is its own prompt line: its intent (or the command, clipped
 * to one line) while closed, the whole command (highlighted) once open —
 * its body then says the exit code, so the row's result hides. */
function isShellOpen(v: ToolRowView): boolean {
  return v.name === 'bash' && isOpen(v)
}

/** A bash row's label: `$` when a command follows it, the tool name when
 * the row shows the call's intent instead. */
function nameLabel(v: ToolRowView): string {
  return v.name === 'bash' && (isShellOpen(v) || !v.description) ? '$' : v.name
}

/** The dedicated body an expanded row gets: a successful write/edit its
 * diff, a text read the file, any bash its terminal. Everything else (and
 * a failed file call) keeps the args/output view. */
function bodyOf(v: ToolRowView): 'diff' | 'file' | 'bash' | null {
  if (!v.row.call) return null
  if (v.name === 'bash') return 'bash'
  if (v.status !== 'ok') return null
  if (v.name === 'write' || v.name === 'edit') return 'diff'
  if (v.name === 'read' && !v.attachments.length) return 'file'
  return null
}

/** A background bash shows its task: live output while it runs, the final
 * result (with its exit code) once done. The row's own result is only the
 * "started task" line — all that is left after a reload drops the task. */
function bashOf(v: ToolRowView): { output: string; failed: boolean; running: boolean } {
  const task = v.taskId ? state.tasks[v.taskId] : undefined
  if (!task) return { output: v.output, failed: v.status !== 'ok', running: false }
  const live = liveTaskOutput(task.id)
  const running = task.status === 'running'
  return { output: running ? live : (task.result?.text ?? live), failed: task.is_error, running }
}

function toggle(v: ToolRowView): void {
  const id = v.row.result.id
  if (v.status === 'blocked' && !expanded.value.has(id)) collapsed.value.add(id)
  else if (expanded.value.has(id)) expanded.value.delete(id)
  else expanded.value.add(id)
}

/** Status color of a row's icon and name: failures danger, else tool. */
function statusClass(v: ToolRowView): string {
  return v.status === 'ok' ? 'text-tool' : 'text-danger'
}

function liveTaskOutput(id: string | null): string {
  return id ? (state.taskOutput[id] ?? '') : ''
}
</script>

<template>
  <div class="flex flex-col divide-y divide-line-soft overflow-hidden rounded-control border border-line">
    <div
      v-for="v in views"
      :key="v.row.result.id"
      v-enter="revealed.has(v.row.result.id) ? false : 'enter-slide'"
      :data-path-node="v.row.result.id"
      class="flex flex-col"
      :class="{ 'bg-danger-soft/40': v.status !== 'ok' }"
    >
      <button
        class="flex w-full min-w-0 gap-2 px-2.5 py-1 text-left transition-colors hover:bg-surface-2/70"
        :class="isShellOpen(v) ? 'items-start' : 'items-center'"
        :title="toolRowTitle(v)"
        :aria-expanded="isOpen(v)"
        @click="toggle(v)"
      >
        <Icon
          :name="v.status === 'ok' ? 'tool' : v.status === 'blocked' ? 'blocked' : 'error'"
          :size="12"
          :class="statusClass(v)"
        />
        <span class="max-w-[40%] flex-none truncate font-mono text-meta font-medium" :class="statusClass(v)">{{
          nameLabel(v)
        }}</span>
        <span
          v-if="isShellOpen(v)"
          class="shell max-h-40 min-w-0 flex-1 overflow-y-auto font-mono text-meta break-words whitespace-pre-wrap text-ink"
          v-html="highlightLine(callArg(v, 'command'), 'bash')"
        ></span>
        <span v-else-if="v.description" class="min-w-0 truncate text-meta text-ink-2">{{ v.description }}</span>
        <span v-else-if="v.callText" class="min-w-0 truncate font-mono text-meta text-ink-2">{{
          v.callText
        }}</span>
        <span v-if="v.diff" class="max-w-[40%] flex-none truncate text-micro text-ink-3">
          · <span class="font-medium text-pine">+{{ v.diff.added }}</span>
          <span v-if="v.diff.removed" class="font-medium text-danger">−{{ v.diff.removed }}</span>
        </span>
        <span
          v-else-if="v.resultText && !isShellOpen(v)"
          class="max-w-[40%] flex-none truncate text-micro text-ink-3"
        >
          · {{ v.resultText }}
        </span>
        <span class="ml-auto flex flex-none items-center gap-2">
          <Chip
            v-if="v.taskId"
            tone="tool"
            mono
            clickable
            :title="v.taskLabel ?? v.taskId"
            @click.stop="openTaskDetail(v.taskId)"
          >
            {{
              t('tools.row.task', {
                id: v.taskId,
                status: taskStatusWord(v.taskStatus ?? 'running'),
              })
            }}
          </Chip>
          <CardChip
            v-if="v.toolCardId && cardsMap.get(v.toolCardId)"
            :card="cardsMap.get(v.toolCardId)!"
            :use-ref="v.cardUseRef ?? undefined"
          />
          <span
            v-if="v.status === 'blocked'"
            v-enter="'enter-stamp'"
            class="rotate-[-4deg] rounded-xs border-[1.5px] border-danger/60 px-1.5 text-micro font-bold tracking-wide text-danger"
            :title="v.statusLabel"
            >{{ v.statusLabel }}</span
          >
          <span
            v-else-if="v.status === 'error'"
            class="text-micro font-semibold text-danger"
            :title="v.statusLabel"
            >{{ v.statusLabel }}</span
          >
          <Icon
            name="chevron"
            :size="12"
            class="text-ink-3/70 transition-transform duration-100"
            :class="{ 'rotate-90': isOpen(v) }"
          />
        </span>
      </button>

      <!-- dedicated bodies fill the expanded row edge to edge -->
      <FileDiff
        v-if="isOpen(v) && bodyOf(v) === 'diff'"
        class="reveal"
        :patch="v.patch"
        :path="v.toolCardId ? 'card.md' : v.callFull"
        :args="argsOf(v.row.call)"
      />
      <FileView
        v-else-if="isOpen(v) && bodyOf(v) === 'file'"
        class="reveal"
        :text="v.output"
        :path="v.toolCardId ? 'card.md' : v.callFull"
        :args="argsOf(v.row.call)"
      />
      <BashView v-else-if="isOpen(v) && bodyOf(v) === 'bash'" class="reveal" v-bind="bashOf(v)" />
      <div v-else-if="isOpen(v)" class="reveal flex flex-col gap-2 pt-1 pr-2.5 pb-2.5 pl-[30px]">
        <div v-if="v.row.call && v.name === 'code'">
          <div class="mb-1 text-micro font-semibold text-ink-3">{{ t('tools.detail.script') }}</div>
          <pre class="code">{{ callArg(v, 'code') }}</pre>
        </div>
        <div v-else-if="v.row.call">
          <div class="mb-1 text-micro font-semibold text-ink-3">{{ t('tools.detail.args') }}</div>
          <div class="code whitespace-normal"><JsonTree :value="v.args" bare /></div>
        </div>
        <div>
          <div class="mb-1 text-micro font-semibold text-ink-3">
            {{ v.status === 'ok' ? t('tools.detail.output') : t('tools.detail.outputError') }}
          </div>
          <div v-if="outputJson(v)" class="code whitespace-normal">
            <JsonTree :value="outputJson(v)" bare />
          </div>
          <pre v-else class="code" :class="{ 'border-danger/40 text-danger': v.status !== 'ok' }">{{
            v.output || t('common.empty')
          }}</pre>
        </div>
        <div v-if="v.attachments.length">
          <div class="mb-1 text-micro font-semibold text-ink-3">{{ t('tools.detail.images') }}</div>
          <div class="flex flex-wrap gap-2">
            <ImageThumb
              v-for="(a, i) in v.attachments"
              :key="`${a.hash}:${i}`"
              :attachment="a"
              :max-h="160"
              :max-w="240"
            />
          </div>
        </div>
        <div v-if="v.taskId && (v.taskStatus === 'running' || liveTaskOutput(v.taskId))">
          <div class="mb-1 text-micro font-semibold text-ink-3">
            {{ t('tools.detail.liveOutput') }}
            <span v-if="v.taskStatus === 'running'" class="text-brand-strong">{{
              t('tools.detail.runningTag')
            }}</span>
          </div>
          <pre class="code">{{ liveTaskOutput(v.taskId) || t('common.noOutput') }}</pre>
        </div>
        <CodeCalls v-if="v.name === 'code' && v.row.call" :parent-call="v.row.call.call_id" />
      </div>
    </div>
  </div>
</template>
