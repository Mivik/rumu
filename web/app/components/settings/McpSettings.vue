<script setup lang="ts">
// MCP servers: each a command (stdio) or a URL (streamable HTTP), with its
// environment or headers. Stored values never come back from the server, so
// a saved one shows as a placeholder and is kept unless retyped. A saved
// server shows its status, its tools and the error it failed with.
import { computed, onBeforeUnmount, reactive, ref, watchEffect } from 'vue'
import { t } from '../../i18n'
import { rumu } from '../../lib/api'
import { layout } from '../../lib/layout'
import { SWAP } from '../../lib/motion'
import { editSettings, models } from '../../lib/state/models'
import type { McpServerInfo } from '../../lib/generated/McpServerInfo'
import Icon from '../ui/Icon.vue'

type Transport = 'command' | 'url'

interface Pair {
  key: string
  /** Typed value; empty keeps a saved one. */
  value: string
  saved: boolean
}

interface Form {
  name: string
  transport: Transport
  command: string
  url: string
  pairs: Pair[]
}

const servers = computed(() => models.settings?.mcp ?? [])

/** The open server's name; `''` adds one. */
const editing = ref<string | null>(null)
const form = reactive<Form>(blank())
const pristine = ref('')
const saved = computed(() => servers.value.find((s) => s.name === editing.value) ?? null)
const isNew = computed(() => editing.value === '')
const dirty = computed(() => JSON.stringify(form) !== pristine.value)
const busy = ref(false)
const pendingDelete = ref(false)

watchEffect(() => (layout.settingsBusy = busy.value))
onBeforeUnmount(() => (layout.settingsBusy = false))

watchEffect(() => {
  const root = { label: t('settings.mcp'), go: () => (editing.value = null) }
  layout.settingsTrail =
    editing.value === null
      ? []
      : [root, { label: isNew.value ? t('settings.addMcp') : (editing.value ?? '') }]
})
onBeforeUnmount(() => (layout.settingsTrail = []))

function blank(): Form {
  return { name: '', transport: 'command', command: '', url: '', pairs: [] }
}

function fill(s: McpServerInfo | null): void {
  Object.assign(
    form,
    s
      ? {
          name: s.name,
          transport: s.url ? 'url' : 'command',
          command: joinArgs(s.command),
          url: s.url ?? '',
          pairs: (s.url ? s.headers : s.env).map((key) => ({ key, value: '', saved: true })),
        }
      : blank(),
  )
  pristine.value = JSON.stringify(form)
}

function open(s: McpServerInfo | null): void {
  pendingDelete.value = false
  editing.value = s?.name ?? ''
  fill(s)
}

/** Split a command line into arguments: whitespace separates, quotes group. */
function splitArgs(line: string): string[] {
  const args: string[] = []
  const re = /"((?:\\.|[^"\\])*)"|'([^']*)'|(\S+)/g
  for (const m of line.matchAll(re)) args.push(m[1]?.replace(/\\(.)/g, '$1') ?? m[2] ?? m[3] ?? '')
  return args
}

function joinArgs(args: string[]): string {
  return args.map((a) => (/^[\w@%+=:,./-]+$/.test(a) ? a : `"${a.replace(/(["\\])/g, '\\$1')}"`)).join(' ')
}

const nameValid = computed(() => /^[A-Za-z0-9_-]{1,64}$/.test(form.name.trim()))
const nameTaken = computed(() => isNew.value && servers.value.some((s) => s.name === form.name.trim()))
const canSave = computed(
  () =>
    !busy.value &&
    nameValid.value &&
    !nameTaken.value &&
    (form.transport === 'command' ? splitArgs(form.command).length > 0 : !!form.url.trim()) &&
    form.pairs.every((p) => !p.key.trim() || p.saved || p.value) &&
    (isNew.value || dirty.value),
)

function addPair(): void {
  form.pairs.push({ key: '', value: '', saved: false })
}

async function save(): Promise<void> {
  if (!canSave.value) return
  busy.value = true
  const name = form.name.trim()
  // A saved value left blank is sent as null: kept.
  const pairs = Object.fromEntries(
    form.pairs.filter((p) => p.key.trim()).map((p) => [p.key.trim(), p.value || (p.saved ? null : '')]),
  )
  const params =
    form.transport === 'command'
      ? { name, command: splitArgs(form.command), env: pairs }
      : { name, url: form.url.trim(), headers: pairs }
  const ok = await editSettings('settings.set_mcp', params)
  const s = ok ? servers.value.find((x) => x.name === name) : undefined
  if (s) open(s)
  busy.value = false
}

async function remove(): Promise<void> {
  if (!saved.value) return
  busy.value = true
  const ok = await editSettings('settings.remove_mcp', { name: saved.value.name })
  busy.value = false
  if (ok) editing.value = null
}

function reconnect(s: McpServerInfo): void {
  void rumu.request('settings.reconnect_mcp', { name: s.name })
}

function firstLine(text?: string): string {
  return (
    text
      ?.split('\n')
      .find((l) => l.trim())
      ?.trim() ?? ''
  )
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div class="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-4 pt-5 pb-6 sm:px-6">
      <Transition v-bind="SWAP" mode="out-in">
        <!-- the list -->
        <div v-if="editing === null" class="flex flex-col gap-2">
          <button
            v-for="s in servers"
            :key="s.name"
            type="button"
            class="flex items-center gap-3 rounded-card border border-line-soft bg-surface px-3 py-2.5 text-left transition-colors hover:bg-surface-2"
            @click="open(s)"
          >
            <span class="font-mono font-semibold text-ink">{{ s.name }}</span>
            <span class="rounded-xs border border-line px-1 text-micro text-ink-3">{{
              s.url ? 'HTTP' : 'stdio'
            }}</span>
            <span class="ml-auto text-meta" :class="s.status === 'failed' ? 'text-danger' : 'text-ink-3'">
              <template v-if="s.status === 'ready'">{{
                t('settings.toolCount', { n: s.tools.length })
              }}</template>
              <template v-else>{{ t(`settings.mcp_${s.status}`) }}</template>
            </span>
            <Icon name="chevron" :size="12" class="text-ink-3" />
          </button>
          <div class="flex items-center gap-2 pt-1">
            <button type="button" class="btn" @click="open(null)">
              <Icon name="plus" :size="13" />
              {{ t('settings.addMcp') }}
            </button>
          </div>
        </div>

        <!-- the server's form -->
        <form v-else class="flex flex-col gap-5" @submit.prevent="save">
          <label v-if="isNew" class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('settings.mcpName') }}</span>
            <input
              v-model="form.name"
              type="text"
              class="w-full font-mono text-meta"
              placeholder="github"
              autocomplete="off"
              spellcheck="false"
            />
            <span v-if="form.name.trim() && !nameValid" class="text-meta text-danger">{{
              t('settings.mcpNameInvalid')
            }}</span>
            <span v-else-if="nameTaken" class="text-meta text-danger">{{ t('settings.mcpNameTaken') }}</span>
          </label>

          <div class="flex flex-col gap-1.5">
            <div class="flex items-center gap-2">
              <div class="flex flex-none rounded-control bg-surface-2 p-0.5" role="radiogroup">
                <button
                  v-for="tr in ['command', 'url'] as const"
                  :key="tr"
                  type="button"
                  role="radio"
                  :aria-checked="form.transport === tr"
                  class="rounded-xs px-2 py-0.5 text-meta font-semibold transition-colors duration-150"
                  :class="
                    form.transport === tr ? 'bg-surface text-ink shadow-soft' : 'text-ink-3 hover:text-ink'
                  "
                  @click="form.transport !== tr && ((form.transport = tr), (form.pairs = []))"
                >
                  {{ t(`settings.mcp_${tr}`) }}
                </button>
              </div>
            </div>
            <input
              v-if="form.transport === 'command'"
              v-model="form.command"
              type="text"
              class="w-full font-mono text-meta"
              placeholder="npx -y @modelcontextprotocol/server-everything"
              autocomplete="off"
              spellcheck="false"
            />
            <input
              v-else
              v-model="form.url"
              type="url"
              class="w-full font-mono text-meta"
              placeholder="https://mcp.example/mcp"
              autocomplete="off"
              spellcheck="false"
            />
          </div>

          <div class="flex flex-col gap-2">
            <span class="field-label">{{
              form.transport === 'command' ? t('settings.mcpEnv') : t('settings.mcpHeaders')
            }}</span>
            <div
              v-for="(p, i) in form.pairs"
              :key="i"
              class="grid grid-cols-[minmax(0,2fr)_minmax(0,3fr)_auto] items-center gap-2"
            >
              <input
                v-model="p.key"
                type="text"
                class="w-full font-mono text-meta"
                :placeholder="form.transport === 'command' ? 'GITHUB_TOKEN' : 'Authorization'"
                :readonly="p.saved"
                autocomplete="off"
                spellcheck="false"
              />
              <input
                v-model="p.value"
                type="text"
                class="w-full font-mono text-meta"
                :placeholder="p.saved ? `•••••••• ${t('settings.apiKeySaved')}` : ''"
                autocomplete="off"
                spellcheck="false"
              />
              <button
                type="button"
                class="icon-btn size-7"
                :title="t('settings.remove')"
                :aria-label="t('settings.remove')"
                @click="form.pairs.splice(i, 1)"
              >
                <Icon name="trash" :size="13" />
              </button>
            </div>
            <button
              type="button"
              class="-mx-2.5 flex items-center gap-2 self-start rounded-control px-2.5 py-1.5 text-secondary text-ink-2 transition-colors hover:bg-surface-2"
              @click="addPair"
            >
              <Icon name="plus" :size="13" class="text-ink-3" />
              {{ form.transport === 'command' ? t('settings.addEnv') : t('settings.addHeader') }}
            </button>
          </div>

          <!-- the saved server's connection -->
          <div v-if="saved" class="flex flex-col gap-2">
            <div class="flex items-center gap-2">
              <span class="field-label">{{ t('settings.mcpStatus') }}</span>
              <span
                class="text-meta"
                :class="
                  saved.status === 'failed'
                    ? 'text-danger'
                    : saved.status === 'ready'
                      ? 'text-ink-2'
                      : 'text-ink-3'
                "
              >
                <template v-if="saved.status === 'ready'">{{
                  t('settings.toolCount', { n: saved.tools.length })
                }}</template>
                <template v-else>{{ t(`settings.mcp_${saved.status}`) }}</template>
              </span>
              <button
                type="button"
                class="icon-btn ml-auto size-7"
                :title="t('settings.reconnect')"
                :aria-label="t('settings.reconnect')"
                :disabled="saved.status === 'connecting'"
                @click="reconnect(saved)"
              >
                <Icon
                  name="regenerate"
                  :size="13"
                  :class="{ 'animate-spin': saved.status === 'connecting' }"
                />
              </button>
            </div>
            <pre
              v-if="saved.error"
              class="max-h-48 overflow-auto rounded-control border border-danger/30 bg-danger-soft px-3 py-2 font-mono text-micro whitespace-pre-wrap text-danger"
              >{{ saved.error }}</pre>
            <ul
              v-if="saved.tools.length"
              class="flex max-h-96 flex-col overflow-y-auto rounded-control border border-line-soft"
            >
              <li
                v-for="tool in saved.tools"
                :key="tool.name"
                class="flex items-baseline gap-3 border-b border-line-soft px-3 py-1.5 last:border-b-0"
              >
                <span class="flex-none font-mono text-meta text-ink">{{ tool.name }}</span>
                <span class="min-w-0 truncate text-meta text-ink-3" :title="tool.description">{{
                  firstLine(tool.description)
                }}</span>
              </li>
            </ul>
          </div>
        </form>
      </Transition>
    </div>

    <Transition
      enter-active-class="transition-opacity duration-150 ease-soft"
      enter-from-class="opacity-0"
      leave-active-class="transition-opacity duration-150 ease-soft"
      leave-to-class="opacity-0"
    >
      <footer
        v-if="editing !== null"
        class="flex flex-none items-center gap-2 border-t border-line-soft px-4 py-3 sm:px-6"
      >
        <template v-if="saved">
          <div v-if="pendingDelete" class="flex items-center gap-2">
            <button type="button" class="btn" :disabled="busy" @click="pendingDelete = false">
              {{ t('common.cancel') }}
            </button>
            <button type="button" class="btn btn-danger" :disabled="busy" @click="remove">
              {{ t('settings.deleteProvider', { name: saved.name }) }}
            </button>
          </div>
          <button
            v-else
            type="button"
            class="inline-flex items-center gap-1.5 rounded-control border border-danger/30 bg-danger-soft px-2.5 py-1 text-meta font-semibold text-danger transition-colors hover:border-danger/50 hover:bg-danger-soft/70"
            :disabled="busy"
            @click="pendingDelete = true"
          >
            <Icon name="trash" :size="13" />
            {{ t('settings.delete') }}
          </button>
        </template>
        <span class="flex-1" />
        <button v-if="dirty && saved" type="button" class="btn" :disabled="busy" @click="open(saved)">
          {{ t('settings.discard') }}
        </button>
        <button type="button" class="btn btn-primary" :disabled="!canSave" @click="save">
          <span
            v-if="busy"
            class="size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent"
          />
          {{ isNew ? t('settings.add') : t('settings.save') }}
        </button>
      </footer>
    </Transition>
  </div>
</template>
