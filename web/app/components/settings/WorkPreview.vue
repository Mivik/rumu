<script setup lang="ts">
// The appearance settings' preview: a made-up turn — adding a parameter to
// a small function and to the half-dozen files that use it, a failing test
// and its fix — drawn by the
// conversation's own ActivityRun under the work view being edited (`work`,
// not yet applied), so it shows what the settings above it would do. The
// turn can be seen running (its last thought still streaming) or done. Any
// change starts it over: fresh node ids drop the rows' clicks, the run's
// key its own.
import { computed, ref, watch } from 'vue'
import { t } from '../../i18n'
import type { ActivityStep, BranchView, DeltaBuffer, ToolRowInfo } from '../../lib/store'
import { provideBranchView } from '../../lib/store'
import { provideWorkView } from '../../lib/state/appearance'
import { makeAside } from '../../lib/state/side'
import type { WorkView } from '../../lib/generated/WorkView'
import type { Node, NodeBody, ToolCall } from '../../lib/types'
import ActivityRun from '../chat/ActivityRun.vue'

const props = defineProps<{ work: WorkView }>()
provideWorkView(computed(() => props.work))

const running = ref(false)
const round = ref(0)
watch([() => props.work, running], () => round.value++)

const delta = computed<DeltaBuffer | null>(() =>
  running.value
    ? {
        text: '',
        reasoning: t('settings.previewThoughtLive'),
        hasReasoning: true,
        hasText: false,
        reasonStart: Date.now(),
        seq: 0,
        tokens: 0,
      }
    : null,
)
const view: BranchView = {
  branch: computed(() => null),
  nodes: computed(() => []),
  timeline: computed(() => []),
  delta,
  busy: computed(() => running.value),
  owed: computed(() => null),
  queued: computed(() => []),
  tasks: computed(() => []),
  nestedTasks: computed(() => 0),
  model: computed(() => null),
  pick: () => {},
  context: computed(() => null),
  aside: makeAside(
    computed(() => null),
    computed(() => []),
  ),
  sub: true,
}
provideBranchView(view)

/** The preview scrolls in a box of its own, its wheel going on to the page
 *  at either end. Chromium only lands on whole device pixels, so at a
 *  fractional zoom the box's end can stay a hair out of reach: it still
 *  counts as scrollable, keeps the wheel and the page never moves. Within a
 *  pixel of the end, the wheel is the page's, smoothly, its targets adding
 *  up while a spin lasts. */
const body = ref<HTMLElement | null>(null)
let target: number | null = null
let lastWheel = 0
function onWheel(e: WheelEvent): void {
  const el = body.value
  if (!el || e.ctrlKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) return
  const dy = e.deltaY * (e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? el.clientHeight : 1)
  const room = dy > 0 ? el.scrollHeight - el.clientHeight - el.scrollTop : el.scrollTop
  if (room >= 1) return
  let page = el.parentElement
  while (
    page &&
    !(/(auto|scroll)/.test(getComputedStyle(page).overflowY) && page.scrollHeight > page.clientHeight)
  )
    page = page.parentElement
  if (!page) return
  e.preventDefault()
  if (e.timeStamp - lastWheel > 250) target = null
  lastWheel = e.timeStamp
  const max = page.scrollHeight - page.clientHeight
  target = Math.max(0, Math.min(max, (target ?? page.scrollTop) + dy))
  page.scrollTo({ top: target, behavior: 'smooth' })
}

const OLD = 'def greet():\n    print("Hello")'
const NEW = 'def greet(name):\n    print(f"Hello, {name}")'
const CALLS: Record<string, { name: string; args: ToolCall['args']; out: string; error?: boolean }> = {
  read: { name: 'read', args: { path: 'greet.py' }, out: OLD },
  grep: {
    name: 'bash',
    args: { command: 'grep -rl greet --exclude=greet.py .' },
    out: './cli.py\n./tests/test_greet.py\n./README.md\n./docs/usage.md',
  },
  edit: { name: 'edit', args: { path: 'greet.py', old_string: OLD, new_string: NEW }, out: 'edited' },
  cli: {
    name: 'edit',
    args: { path: 'cli.py', old_string: '    greet()', new_string: '    greet(args.name)' },
    out: 'edited',
  },
  tests: {
    name: 'edit',
    args: {
      path: 'tests/test_greet.py',
      old_string: 'assert run(greet) == "Hello"',
      new_string: 'assert run(greet, "Ada") == "Hello, Ada"',
    },
    out: 'edited',
  },
  readme: {
    name: 'edit',
    args: { path: 'README.md', old_string: 'greet()  # Hello', new_string: 'greet("Ada")  # Hello, Ada' },
    out: 'edited',
  },
  docs: {
    name: 'edit',
    args: { path: 'docs/usage.md', old_string: 'python cli.py', new_string: 'python cli.py --name Ada' },
    out: 'edited',
  },
  changelog: {
    name: 'write',
    args: { path: 'CHANGELOG.md', content: '## 0.2\n\n- greet takes a name\n' },
    out: 'wrote CHANGELOG.md',
  },
  test: {
    name: 'bash',
    args: { command: 'pytest' },
    out: "exit code: 1\nFAILED tests/test_cli.py::test_no_name - TypeError: greet() missing 1 required positional argument: 'name'",
    error: true,
  },
  fix: {
    name: 'edit',
    args: { path: 'greet.py', old_string: 'def greet(name):', new_string: 'def greet(name="world"):' },
    out: 'edited',
  },
  retest: { name: 'bash', args: { command: 'pytest' }, out: 'exit code: 0\n4 passed' },
}

const T0 = Date.now() - 40_000

/** The turn, as the timeline would group it. */
const steps = computed<ActivityStep[]>(() => {
  const r = round.value
  let at = T0
  const node = (id: string, dur: number, body: NodeBody): Node => {
    at += dur
    return { id: `preview-${r}-${id}`, parent: null, session: 'preview', at, dur_ms: dur, body }
  }
  /** A step: its reasoning (if any), then its calls and their results. */
  const step = (id: string, thought: string | null, reasonMs: number, calls: string[]): ActivityStep[] => {
    const toolCalls: ToolCall[] = calls.map((c) => ({
      call_id: `${r}-${c}`,
      name: CALLS[c]!.name,
      args: CALLS[c]!.args,
    }))
    const assistant = node(id, reasonMs + 1500, {
      type: 'assistant',
      content: { text: '' },
      reasoning: thought ? [{ type: 'thinking', text: thought }] : [],
      tool_calls: toolCalls,
      model: 'preview',
      usage: { prompt_tokens: 0, completion_tokens: 0, cached_tokens: 0, cache_creation_tokens: 0 },
      reason_ms: reasonMs,
    })
    const rows: ToolRowInfo[] = calls.map((c, i) => ({
      result: node(`${c}-result`, 2000, {
        type: 'tool_result',
        call_id: toolCalls[i]!.call_id,
        content: { text: CALLS[c]!.out },
        is_error: !!CALLS[c]!.error,
      }),
      call: toolCalls[i]!,
      assistant,
      invokes: [],
    }))
    const tools: ActivityStep = { kind: 'tools', rows }
    return thought ? [{ kind: 'thought', node: assistant }, tools] : [tools]
  }
  const done = [
    ...step('a1', t('settings.previewThought'), 3000, ['read', 'grep']),
    ...step('a2', null, 0, ['edit', 'cli', 'tests', 'readme', 'docs', 'changelog', 'test']),
  ]
  // consecutive results are one step, as in the timeline
  done.splice(1, 2, {
    kind: 'tools',
    rows: done.slice(1, 3).flatMap((s) => (s.kind === 'tools' ? s.rows : [])),
  })
  if (running.value) return [...done, { kind: 'thought', node: null }]
  return [...done, ...step('a3', t('settings.previewThoughtFix'), 2000, ['fix', 'retest'])]
})
</script>

<template>
  <div class="mt-3 overflow-hidden rounded-card border border-line bg-canvas">
    <div class="flex items-center justify-between gap-3 border-b border-line-soft px-3 py-2">
      <span class="field-label">{{ t('settings.preview') }}</span>
      <div
        class="flex rounded-control bg-surface-2 p-0.5"
        role="radiogroup"
        :aria-label="t('settings.preview')"
      >
        <button
          v-for="x in [true, false]"
          :key="String(x)"
          type="button"
          role="radio"
          :aria-checked="running === x"
          class="rounded-xs px-2 py-0.5 text-micro font-semibold transition-colors duration-150"
          :class="running === x ? 'bg-surface text-ink shadow-soft' : 'text-ink-3 hover:text-ink'"
          @click="running = x"
        >
          {{ t(x ? 'settings.preview_running' : 'settings.preview_done') }}
        </button>
      </div>
    </div>
    <div
      ref="body"
      class="flex max-h-[28rem] min-w-0 flex-col gap-2.5 overflow-y-auto px-4 py-3 text-message"
      @wheel="onWheel"
    >
      <p class="max-w-[85%] self-end rounded-sheet rounded-br-xs bg-brand-soft px-4 py-2 text-ink">
        {{ t('settings.previewPrompt') }}
      </p>
      <ActivityRun :key="round" :steps="steps" :live="running" :current="running" />
      <p v-if="!running">{{ t('settings.previewAnswer') }}</p>
    </div>
  </div>
</template>
