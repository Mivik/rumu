// Task helpers shared by the tool rows and the tasks panel. Tasks are
// runtime-only kernel rows (`task.started/progress/done`, `task.list`).

import type { TaskInfo, TaskStatus } from './types'
import { t } from '../i18n'
import { callById } from './state/derived'

/** `bash: rm -rf dist`, `agent: find the config`. */
export function taskLabel(t: TaskInfo): string {
  if (t.kind.type === 'tool') {
    return t.kind.args_summary ? `${t.kind.name}: ${t.kind.args_summary}` : t.kind.name
  }
  return `agent: ${t.summary}`
}

/** A bash task's command, read from its call (the task keeps only a summary);
 * empty until the call's node is loaded. */
export function taskCommand(t: TaskInfo): string {
  if (t.kind.type !== 'tool' || t.kind.name !== 'bash') return ''
  const command = (callById(t.call_id)?.args as { command?: unknown } | null)?.command
  return typeof command === 'string' ? command.trimEnd() : ''
}

/** The word shown in notes and chips (`ok` / `error` / `cancelled`). */
export function taskStatusWord(status: TaskStatus): string {
  const key = status === 'done' ? 'ok' : status === 'failed' ? 'error' : status
  return t(`tools.status.${key}`)
}

/** The task id of a background call's result: `started task t-1a2b3c4d (…)`,
 * or `still running after 60s, so it moved to the background as task t-1a2b3c4d (…)`. */
export function taskIdOf(output: string): string | null {
  return (
    /^(?:started task|still running after [\d.]+s, so it moved to the background as task) (t-[0-9a-f]{8})/.exec(
      output,
    )?.[1] ?? null
  )
}

/** Compact elapsed time for a running/finished task: `4s`, `1m 05s`. */
export function taskElapsed(t: TaskInfo, now = Date.now()): string {
  const ms = Math.max(1, (t.ended_at ?? now) - t.started_at)
  if (ms < 1000) return `${Math.round(ms)}ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1).replace(/\.0$/, '')}s`
  const s = Math.round(ms / 1000)
  return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, '0')}s`
}
