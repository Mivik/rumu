<script lang="ts">
// One side note (a context notice other than recalled cards; see
// ContextPills), as a sentence that reads on its own: the default workspace
// set, the session moved into or out of a project, a CLAUDE.md loaded, a card that changed, a finished background task,
// the session's MCP servers or skills, a watch event, a sub-agent's reply,
// files the user reverted, a plain note in full. The model-facing text
// stays in the hover title.
import { t } from '../../i18n'
import type { BranchId, Tagged, TaskStatus } from '../../lib/types'
import type { McpStatus } from '../../lib/generated/McpStatus'

export interface McpServerLine {
  name: string
  status?: McpStatus
  tools?: number
  error?: string
}

export type Notice =
  | { kind: 'workspace' | 'agents'; key: string; name: string }
  | { kind: 'project'; key: string; name: string | null; text: string }
  | { kind: 'update'; key: string; id: string; label: string; text: string }
  | { kind: 'task'; key: string; id: string; label: string; status: TaskStatus; text: string }
  | { kind: 'mcp'; key: string; servers: McpServerLine[]; text: string }
  | { kind: 'app'; key: string; tool: string; text: string }
  | { kind: 'skills'; key: string; names: string[]; text: string }
  | { kind: 'watch'; key: string; id: string; label: string; count: number; matching: boolean; text: string }
  | { kind: 'silence'; key: string; id: string; label: string; secs: number; matching: boolean }
  | { kind: 'reply'; key: string; branch: BranchId; label: string; text: string }
  | { kind: 'note'; key: string; source?: string; text: string }
  | { kind: 'revert'; key: string; files: string[]; text: string }
  | { kind: 'plugin'; key: string; data: Tagged; text: string }

const ICON: Record<Notice['kind'], string> = {
  workspace: 'mount',
  project: 'mount',
  agents: 'file',
  update: 'note',
  task: 'tool',
  mcp: 'mcp',
  app: 'mcp',
  skills: 'document',
  watch: 'bash',
  silence: 'bash',
  reply: 'agent',
  note: 'note',
  revert: 'undo',
  plugin: 'plugin',
}

/** `telegram (61 tools)`; a server not ready says so. */
function serverWord(s: McpServerLine): string {
  if (s.status === 'connecting') return t('cards.notes.mcpConnecting', { name: s.name })
  if (s.status === 'failed') return t('cards.notes.mcpFailed', { name: s.name })
  if (s.status === 'ready') return t('cards.notes.mcpTools', { name: s.name, count: s.tools ?? 0 })
  return s.name
}

/** Names in a sentence: `a、b、c` / `a, b, c`. */
function joinList(xs: string[]): string {
  return xs.join(t('cards.notes.sep'))
}
</script>

<script setup lang="ts">
import { openAgentView, openDrawer, openTaskDetail } from '../../lib/store'
import { rendererOf } from '../../lib/plugins'
import PluginView from '../plugins/PluginView.vue'
import { taskStatusWord } from '../../lib/tasks'
import { workspaceLabel } from '../../lib/util'
import Icon from '../ui/Icon.vue'

defineProps<{ notice: Notice }>()
</script>

<template>
  <div class="flex max-w-full min-w-0 items-baseline gap-1.5 text-meta text-ink-3">
    <Icon :name="ICON[notice.kind]" :size="12" class="flex-none text-ink-3" />
    <span v-if="notice.kind === 'workspace'" class="min-w-0 truncate">
      {{ t('cards.notes.workspace', { name: workspaceLabel(notice.name) }) }}
    </span>
    <span v-else-if="notice.kind === 'project'" class="min-w-0 truncate" :title="notice.text">
      {{
        notice.name ? t('cards.notes.projectEntered', { name: notice.name }) : t('cards.notes.projectLeft')
      }}
    </span>
    <span v-else-if="notice.kind === 'agents'" class="min-w-0 truncate">
      {{ t('cards.notes.loaded', { name: notice.name }) }}
    </span>
    <span v-else-if="notice.kind === 'update'" class="min-w-0" :title="notice.text">
      {{ t('cards.notes.cardBefore') }}
      <button class="card-ref" type="button" @click="openDrawer(notice.id)">{{ notice.label }}</button>
      {{ t('cards.notes.cardAfter') }}
    </span>
    <button
      v-else-if="notice.kind === 'task'"
      type="button"
      class="min-w-0 truncate text-left hover:text-ink"
      :title="notice.text"
      @click="openTaskDetail(notice.id)"
    >
      {{ t('cards.notes.task', { label: notice.label, status: taskStatusWord(notice.status) }) }}
    </button>
    <span v-else-if="notice.kind === 'mcp'" class="min-w-0 truncate" :title="notice.text">
      {{
        notice.servers.length
          ? t('cards.notes.mcp', { list: joinList(notice.servers.map(serverWord)) })
          : t('cards.notes.mcpNone')
      }}
    </span>
    <span v-else-if="notice.kind === 'app'" class="min-w-0 truncate" :title="notice.text">
      {{ t('cards.notes.appContext', { tool: notice.tool }) }}
    </span>
    <span v-else-if="notice.kind === 'skills'" class="min-w-0 truncate" :title="notice.text">
      {{
        notice.names.length
          ? t('cards.notes.skills', { list: joinList(notice.names) })
          : t('cards.notes.skillsNone')
      }}
    </span>
    <button
      v-else-if="notice.kind === 'watch'"
      type="button"
      class="min-w-0 truncate text-left hover:text-ink"
      :title="notice.text"
      @click="openTaskDetail(notice.id)"
    >
      {{
        notice.count
          ? t(notice.matching ? 'cards.notes.watchMatch' : 'cards.notes.watch', {
              label: notice.label,
              count: notice.count,
            })
          : t('cards.notes.watchSome', { label: notice.label })
      }}
    </button>
    <button
      v-else-if="notice.kind === 'silence'"
      type="button"
      class="min-w-0 truncate text-left hover:text-ink"
      @click="openTaskDetail(notice.id)"
    >
      {{
        t(notice.matching ? 'cards.notes.silenceMatch' : 'cards.notes.silence', {
          label: notice.label,
          secs: notice.secs,
        })
      }}
    </button>
    <button
      v-else-if="notice.kind === 'reply'"
      type="button"
      class="min-w-0 truncate text-left hover:text-ink"
      :title="notice.text"
      @click="openAgentView(notice.branch)"
    >
      {{ t('cards.notes.reply', { label: notice.label }) }}
    </button>
    <span v-else-if="notice.kind === 'revert'" class="min-w-0 truncate" :title="notice.text">
      {{
        notice.files.length
          ? t('cards.notes.revert', { count: notice.files.length })
          : t('cards.notes.revertNone')
      }}
    </span>
    <PluginView
      v-else-if="notice.kind === 'plugin' && rendererOf(notice.data)"
      class="min-w-0"
      :plugin="rendererOf(notice.data)!.plugin"
      :is="rendererOf(notice.data)!.component"
      :props="{ data: notice.data.data, v: notice.data.v, text: notice.text }"
      :title="notice.text"
    />
    <span v-else-if="notice.kind === 'note'" class="min-w-0 whitespace-pre-wrap"
      ><span v-if="notice.source" class="font-semibold text-ink-2">{{ notice.source }} </span
      >{{ notice.text }}</span
    >
  </div>
</template>
