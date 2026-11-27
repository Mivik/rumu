// The web side of external plugins. A running plugin whose `initialize`
// names a web entry (`PluginInfo.web`) is an ES module the page imports; its
// default export is a setup function handed a context (`PluginContext`) to
// register what the plugin adds: result renderers by data tag, status items
// above the composer, slash commands and side panels. It may return a
// cleanup. A new URL (the file changed, the plugin restarted) undoes what the
// old module registered and sets the new one up.
//
// Plugins run in the page and share its Vue: `globalThis.__rumu.vue` is the
// instance their build (@rumu/plugin's Vite preset) imports `vue` from.
// Whatever they register is mounted inside `PluginView`, an error boundary: a
// plugin that throws breaks only its own part, and says so there.
//
// `ctx.call` reaches the plugin's process through the server (`plugin.call`);
// the events it pushes (`host/emit`, the `plugin.event` push) come only for
// the session shown, like every other session push.

import * as Vue from 'vue'
import { markRaw, reactive, watch, type Component } from 'vue'
import { t } from '../i18n'
import { rumu } from './api'
import { models } from './state/models'
import { sendPrompt } from './state/sessions'
import { state } from './state/state'
import { toast } from './state/ui'
import Icon from '../components/ui/Icon.vue'
import Chip from '../components/ui/Chip.vue'

/** Renders a tool result or note whose data carries the tag it was
 *  registered for. Its props: the payload (`data`), its version (`v`), and
 *  the text the model saw (`text`). */
export interface ResultRenderer {
  plugin: string
  component: Component
}

/** A small label above the composer: `render` gives its content (text or a
 *  vnode; `null` hides it). */
export interface StatusItem {
  plugin: string
  id: string
  render: () => unknown
  title?: () => string
  onClick?: () => void
}

/** `/name [arg]` in the composer runs `run(arg)` instead of sending. */
export interface PluginCommand {
  plugin: string
  name: string
  description?: string
  run: (arg: string) => unknown
}

/** A sheet beside the chat, like a sub-agent's or an artifact's. */
export interface PluginPanel {
  plugin: string
  id: string
  title: string
  component: Component
}

type Listener = (data: unknown, branch: string | null) => void

export const plugins = reactive({
  /** By data tag; the first plugin to claim a tag keeps it. */
  renderers: {} as Record<string, ResultRenderer>,
  statuses: [] as StatusItem[],
  commands: [] as PluginCommand[],
  panels: [] as PluginPanel[],
  /** Why a plugin's web side did not load or set up. */
  errors: {} as Record<string, string>,
  /** The panel open beside the chat. */
  panel: null as { plugin: string; id: string } | null,
})

interface Loaded {
  url: string
  dispose?: () => void
  listeners: Map<string, Set<Listener>>
}

const loaded = new Map<string, Loaded>()

/** The panel open beside the chat, while its plugin still offers it. */
export function openPanelOf(): PluginPanel | null {
  const open = plugins.panel
  return open ? (plugins.panels.find((p) => p.plugin === open.plugin && p.id === open.id) ?? null) : null
}

export function openPluginPanel(plugin: string, id: string | null): void {
  if (id) {
    state.artifact = null
    state.agentView = null
    state.taskDetail = null
  }
  plugins.panel = id ? { plugin, id } : null
}

/** The renderer for a node's data, if a plugin claimed its tag. */
export function rendererOf(data: { tag: string } | null | undefined): ResultRenderer | null {
  return (data && plugins.renderers[data.tag]) || null
}

/** Run `/name arg` if a plugin registered `name`; `null` when none did. */
export function runCommand(text: string): Promise<void> | null {
  const m = /^\/([\w-]+)(?:\s+([\s\S]*))?$/.exec(text)
  const cmd = m && plugins.commands.find((c) => c.name === m[1])
  if (!m || !cmd) return null
  return (async () => {
    try {
      await cmd.run(m[2]?.trim() ?? '')
    } catch (err) {
      toast('error', t('common.pluginFailed', { name: cmd.plugin }), (err as Error).message)
    }
  })()
}

function unload(name: string): void {
  const l = loaded.get(name)
  loaded.delete(name)
  for (const [tag, r] of Object.entries(plugins.renderers))
    if (r.plugin === name) delete plugins.renderers[tag]
  plugins.statuses = plugins.statuses.filter((s) => s.plugin !== name)
  plugins.commands = plugins.commands.filter((c) => c.plugin !== name)
  plugins.panels = plugins.panels.filter((p) => p.plugin !== name)
  delete plugins.errors[name]
  try {
    l?.dispose?.()
  } catch (err) {
    console.error(`plugin ${name}: cleanup failed`, err)
  }
}

function fail(name: string, err: unknown): void {
  const message = err instanceof Error ? err.message : String(err)
  console.error(`plugin ${name}:`, err)
  plugins.errors[name] = message
  toast('error', t('common.pluginFailed', { name }), message)
}

/** What a plugin's setup is handed. */
function context(name: string, l: Loaded) {
  const info = () => models.settings?.plugins.find((p) => p.name === name)
  return {
    name,
    /** The values of its settings (reactive). */
    get settings(): Record<string, unknown> {
      return info()?.settings ?? {}
    },
    /** Ask the plugin's process: its `call` handler, for the session shown. */
    call<T = unknown>(method: string, params: unknown = null): Promise<T> {
      return rumu.request<T>('plugin.call', { plugin: name, method, params, branch: state.mainBranch })
    },
    /** Take the events the plugin's process pushes (`ctx.emit`). */
    on(event: string, fn: Listener): () => void {
      let set = l.listeners.get(event)
      if (!set) l.listeners.set(event, (set = new Set()))
      set.add(fn)
      return () => set.delete(fn)
    },
    renderResult(tag: string, component: Component): void {
      if (plugins.renderers[tag]) return
      plugins.renderers[tag] = { plugin: name, component: markRaw(component) }
    },
    addStatus(item: Omit<StatusItem, 'plugin'>): void {
      plugins.statuses.push({ ...item, plugin: name })
    },
    addCommand(cmd: Omit<PluginCommand, 'plugin'>): void {
      plugins.commands.push({ ...cmd, plugin: name })
    },
    addPanel(panel: Omit<PluginPanel, 'plugin'>): void {
      plugins.panels.push({ ...panel, component: markRaw(panel.component), plugin: name })
    },
    /** Open one of its panels beside the chat (`null` closes it). */
    openPanel(id: string | null): void {
      openPluginPanel(name, id)
    },
    session: {
      /** The session shown, and its main line. */
      get id(): string | null {
        return state.sessionId
      },
      get branch(): string | null {
        return state.mainBranch
      },
      /** Send a message to the session shown (a new one on the landing page). */
      send: (text: string): Promise<boolean> => sendPrompt(text),
    },
    /** May change or go in any release. */
    experimental: {
      ui: { Icon, Chip },
    },
  }
}

export type PluginContext = ReturnType<typeof context>

async function load(name: string, url: string): Promise<void> {
  const l: Loaded = { url, listeners: new Map() }
  loaded.set(name, l)
  try {
    const mod = (await import(/* @vite-ignore */ url)) as { default?: unknown }
    if (loaded.get(name) !== l) return
    const setup = mod.default
    if (typeof setup !== 'function') throw new Error('its web entry has no default export to set up')
    const dispose: unknown = await setup(context(name, l))
    if (typeof dispose === 'function') l.dispose = dispose as () => void
  } catch (err) {
    if (loaded.get(name) === l) fail(name, err)
  }
}

/** Load the running plugins' web sides, and follow them. */
export function startPlugins(): void {
  ;(globalThis as { __rumu?: unknown }).__rumu = { vue: Vue }
  rumu.onPush('plugin.event', (p) => {
    const set = loaded.get(String(p.plugin))?.listeners.get(String(p.event))
    for (const fn of set ?? []) {
      try {
        fn(p.data, (p.branch as string | null) ?? null)
      } catch (err) {
        console.error(`plugin ${String(p.plugin)}: event handler failed`, err)
      }
    }
  })
  watch(
    () =>
      (models.settings?.plugins ?? []).flatMap((p) =>
        p.status === 'running' && p.web ? [[p.name, rumu.base + p.web]] : [],
      ),
    (now) => {
      const urls = new Map(now as [string, string][])
      for (const [name, l] of [...loaded]) if (urls.get(name) !== l.url) unload(name)
      for (const [name, url] of urls) if (!loaded.has(name)) void load(name, url)
    },
    { immediate: true },
  )
}
