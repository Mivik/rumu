<script setup lang="ts">
// External plugins: each a command run in RUMU_HOME (or a directory under
// it) that speaks rumu's stdio protocol. A saved plugin shows its status,
// what it registers and the error it stopped with, and restarts from here.
// A running plugin that gives a schema for its settings gets a form of them
// here (one row per property); a change of settings alone reaches it without
// a restart.
import { computed, onBeforeUnmount, reactive, ref, watchEffect } from 'vue'
import { t } from '../../i18n'
import { rumu } from '../../lib/api'
import { layout } from '../../lib/layout'
import { SWAP } from '../../lib/motion'
import { editSettings, models } from '../../lib/state/models'
import { joinArgs, splitArgs } from '../../lib/util'
import type { PluginInfo } from '../../lib/generated/PluginInfo'
import Icon from '../ui/Icon.vue'
import Select from '../ui/Select.vue'
import Switch from '../ui/Switch.vue'

type Value = string | number | boolean

interface Form {
  name: string
  command: string
  cwd: string
  settings: Record<string, Value>
}

/** One property of a plugin's settings schema. */
interface Field {
  key: string
  type: 'string' | 'integer' | 'number' | 'boolean'
  title: string
  description?: string
  default?: Value
  enum?: string[]
  minimum?: number
  maximum?: number
}

const plugins = computed(() => models.settings?.plugins ?? [])

/** The open plugin's name; `''` adds one. */
const editing = ref<string | null>(null)
const form = reactive<Form>(blank())
const pristine = ref('')
const saved = computed(() => plugins.value.find((p) => p.name === editing.value) ?? null)
const isNew = computed(() => editing.value === '')
const dirty = computed(() => JSON.stringify(form) !== pristine.value)
const busy = ref(false)
const pendingDelete = ref(false)

watchEffect(() => (layout.settingsBusy = busy.value))
onBeforeUnmount(() => (layout.settingsBusy = false))

watchEffect(() => {
  const root = { label: t('settings.plugins'), go: () => (editing.value = null) }
  layout.settingsTrail =
    editing.value === null
      ? []
      : [root, { label: isNew.value ? t('settings.addPlugin') : (editing.value ?? '') }]
})
onBeforeUnmount(() => (layout.settingsTrail = []))

function blank(): Form {
  return { name: '', command: '', cwd: '', settings: {} }
}

/** The fields the open plugin's schema describes. */
const fields = computed<Field[]>(() => {
  const props = (saved.value?.schema?.properties ?? {}) as Record<string, Partial<Field>>
  return Object.entries(props).flatMap(([key, f]) =>
    f && ['string', 'integer', 'number', 'boolean'].includes(f.type ?? '')
      ? [{ ...f, key, type: f.type!, title: f.title ?? key }]
      : [],
  )
})

/** What the form holds for each field: the stored value, else the default. */
function settingsOf(p: PluginInfo): Record<string, Value> {
  const out: Record<string, Value> = {}
  const props = (p.schema?.properties ?? {}) as Record<string, Partial<Field>>
  for (const [key, f] of Object.entries(props)) {
    const v = (p.settings[key] ?? f?.default) as Value | undefined
    if (v !== undefined) out[key] = v
  }
  return out
}

function open(p: PluginInfo | null): void {
  pendingDelete.value = false
  editing.value = p?.name ?? ''
  Object.assign(
    form,
    p ? { name: p.name, command: joinArgs(p.command), cwd: p.cwd ?? '', settings: settingsOf(p) } : blank(),
  )
  pristine.value = JSON.stringify(form)
}

function setNumber(f: Field, raw: string): void {
  const n = f.type === 'integer' ? parseInt(raw, 10) : parseFloat(raw)
  if (Number.isNaN(n)) delete form.settings[f.key]
  else form.settings[f.key] = n
}

const nameValid = computed(() => /^[A-Za-z0-9_-]{1,64}$/.test(form.name.trim()))
const nameTaken = computed(() => isNew.value && plugins.value.some((p) => p.name === form.name.trim()))
const canSave = computed(
  () =>
    !busy.value &&
    nameValid.value &&
    !nameTaken.value &&
    splitArgs(form.command).length > 0 &&
    (isNew.value || dirty.value),
)

async function save(): Promise<void> {
  if (!canSave.value) return
  busy.value = true
  const name = form.name.trim()
  const ok = await editSettings('settings.set_plugin', {
    name,
    command: splitArgs(form.command),
    cwd: form.cwd.trim() || null,
    ...(fields.value.length && { settings: form.settings }),
  })
  const p = ok ? plugins.value.find((x) => x.name === name) : undefined
  if (p) open(p)
  busy.value = false
}

async function remove(): Promise<void> {
  if (!saved.value) return
  busy.value = true
  const ok = await editSettings('settings.remove_plugin', { name: saved.value.name })
  busy.value = false
  if (ok) editing.value = null
}

function restart(p: PluginInfo): void {
  void rumu.request('settings.restart_plugin', { name: p.name })
}

function statusClass(p: PluginInfo): string {
  return p.status === 'failed' ? 'text-danger' : p.status === 'running' ? 'text-ink-2' : 'text-ink-3'
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div class="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-4 pt-5 pb-6 sm:px-6">
      <Transition v-bind="SWAP" mode="out-in">
        <!-- the list -->
        <div v-if="editing === null" class="flex flex-col gap-2">
          <p v-if="!plugins.length" class="text-meta text-ink-3">{{ t('settings.noPlugins') }}</p>
          <button
            v-for="p in plugins"
            :key="p.name"
            type="button"
            class="flex items-center gap-3 rounded-card border border-line-soft bg-surface px-3 py-2.5 text-left transition-colors hover:bg-surface-2"
            @click="open(p)"
          >
            <span class="font-mono font-semibold text-ink">{{ p.name }}</span>
            <span class="min-w-0 flex-1 truncate font-mono text-micro text-ink-3">{{
              p.command.join(' ')
            }}</span>
            <span class="flex-none text-meta" :class="statusClass(p)">{{
              t(`settings.plugin_${p.status}`)
            }}</span>
            <Icon name="chevron" :size="12" class="text-ink-3" />
          </button>
          <div class="flex items-center gap-2 pt-1">
            <button type="button" class="btn" @click="open(null)">
              <Icon name="plus" :size="13" />
              {{ t('settings.addPlugin') }}
            </button>
          </div>
        </div>

        <!-- the plugin's form -->
        <form v-else class="flex flex-col gap-5" @submit.prevent="save">
          <label v-if="isNew" class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('settings.mcpName') }}</span>
            <input
              v-model="form.name"
              type="text"
              class="w-full font-mono text-meta"
              placeholder="scratch-path"
              autocomplete="off"
              spellcheck="false"
            />
            <span v-if="form.name.trim() && !nameValid" class="text-meta text-danger">{{
              t('settings.mcpNameInvalid')
            }}</span>
            <span v-else-if="nameTaken" class="text-meta text-danger">{{ t('settings.mcpNameTaken') }}</span>
          </label>

          <label class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('settings.mcp_command') }}</span>
            <input
              v-model="form.command"
              type="text"
              class="w-full font-mono text-meta"
              placeholder="node plugins/dice/dist/index.js"
              autocomplete="off"
              spellcheck="false"
            />
          </label>

          <label class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('settings.pluginCwd') }}</span>
            <input
              v-model="form.cwd"
              type="text"
              class="w-full font-mono text-meta"
              placeholder="RUMU_HOME"
              autocomplete="off"
              spellcheck="false"
            />
          </label>

          <!-- the settings its schema describes -->
          <div v-if="fields.length" class="flex flex-col gap-4">
            <span class="field-label">{{ t('settings.pluginSettings') }}</span>
            <div v-for="f in fields" :key="f.key" class="flex flex-col gap-1.5">
              <span class="flex items-center gap-3">
                <span class="min-w-0 flex-1 text-meta text-ink-2" :title="f.description">{{ f.title }}</span>
                <Switch
                  v-if="f.type === 'boolean'"
                  :model-value="!!form.settings[f.key]"
                  :label="f.title"
                  @update:model-value="form.settings[f.key] = $event"
                />
                <Select
                  v-else-if="f.enum"
                  :model-value="String(form.settings[f.key] ?? '')"
                  :options="f.enum.map((v) => ({ value: v, label: v }))"
                  :label="f.title"
                  @update:model-value="form.settings[f.key] = $event"
                />
                <input
                  v-else-if="f.type !== 'string'"
                  type="number"
                  class="w-32 font-mono text-meta"
                  :value="form.settings[f.key]"
                  :min="f.minimum"
                  :max="f.maximum"
                  :step="f.type === 'integer' ? 1 : 'any'"
                  @input="setNumber(f, ($event.target as HTMLInputElement).value)"
                />
              </span>
              <input
                v-if="f.type === 'string' && !f.enum"
                v-model="form.settings[f.key]"
                type="text"
                class="w-full text-meta"
                autocomplete="off"
                spellcheck="false"
              />
            </div>
          </div>

          <!-- the saved plugin's process -->
          <div v-if="saved" class="flex flex-col gap-2">
            <div class="flex items-center gap-2">
              <span class="field-label">{{ t('settings.pluginStatus') }}</span>
              <span class="text-meta" :class="statusClass(saved)">{{
                t(`settings.plugin_${saved.status}`)
              }}</span>
              <button
                type="button"
                class="icon-btn ml-auto size-7"
                :title="t('settings.restart')"
                :aria-label="t('settings.restart')"
                :disabled="saved.status === 'starting'"
                @click="restart(saved)"
              >
                <Icon name="regenerate" :size="13" :class="{ 'animate-spin': saved.status === 'starting' }" />
              </button>
            </div>
            <pre
              v-if="saved.error"
              class="max-h-48 overflow-auto rounded-control border border-danger/30 bg-danger-soft px-3 py-2 font-mono text-micro whitespace-pre-wrap text-danger"
              >{{ saved.error }}</pre>
            <div
              v-if="saved.tools.length || saved.hooks.length"
              class="flex flex-wrap gap-1.5 font-mono text-micro"
            >
              <span
                v-for="tool in saved.tools"
                :key="`t:${tool}`"
                class="rounded-xs border border-line px-1 text-ink-2"
                >{{ tool }}</span
              >
              <span
                v-for="hook in saved.hooks"
                :key="`h:${hook}`"
                class="rounded-xs border border-dashed border-line px-1 text-ink-3"
                :title="t('settings.pluginHook')"
                >{{ hook }}</span
              >
            </div>
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
