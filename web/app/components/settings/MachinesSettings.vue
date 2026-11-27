<script setup lang="ts">
// Machines: other machines' rumu this one forwards to (`/m/<name>/…`), each
// a URL and the token it takes. The sidebar lists their sessions with this
// one's. The token never comes back, so a saved one is kept unless retyped.
// They are the serving machine's settings, whichever machine the page shows.
import { computed, onBeforeUnmount, reactive, ref, watchEffect } from 'vue'
import { t } from '../../i18n'
import { layout } from '../../lib/layout'
import { compatible, homeRequest, machineState, machines } from '../../lib/machines'
import { SWAP } from '../../lib/motion'
import { models, onSettingsUpdated } from '../../lib/state/models'
import { toast } from '../../lib/state/ui'
import type { MachineInfo } from '../../lib/generated/MachineInfo'
import type { SettingsInfo } from '../../lib/generated/SettingsInfo'
import Icon from '../ui/Icon.vue'

interface Form {
  name: string
  url: string
  token: string
}

/** The serving machine's list: its settings while it is shown, else the
 *  menu's (the same list). */
const list = computed<MachineInfo[]>(() =>
  machines.current
    ? machines.all.filter((m) => m.name).map((m) => ({ name: m.name, url: m.url }))
    : (models.settings?.machines ?? []),
)

/** The open machine's name; `''` adds one. */
const editing = ref<string | null>(null)
const form = reactive<Form>({ name: '', url: '', token: '' })
const pristine = ref('')
const saved = computed(() => list.value.find((m) => m.name === editing.value) ?? null)
const isNew = computed(() => editing.value === '')
const dirty = computed(() => JSON.stringify(form) !== pristine.value)
const busy = ref(false)
const pendingDelete = ref(false)

watchEffect(() => (layout.settingsBusy = busy.value))
onBeforeUnmount(() => (layout.settingsBusy = false))

watchEffect(() => {
  const root = { label: t('settings.machines'), go: () => (editing.value = null) }
  layout.settingsTrail =
    editing.value === null
      ? []
      : [root, { label: isNew.value ? t('settings.addMachine') : (editing.value ?? '') }]
})
onBeforeUnmount(() => (layout.settingsTrail = []))

function open(m: MachineInfo | null): void {
  pendingDelete.value = false
  editing.value = m?.name ?? ''
  Object.assign(form, { name: m?.name ?? '', url: m?.url ?? '', token: '' })
  pristine.value = JSON.stringify(form)
}

const nameValid = computed(() => /^[A-Za-z0-9_-]{1,64}$/.test(form.name.trim()))
const nameTaken = computed(() => isNew.value && list.value.some((m) => m.name === form.name.trim()))
const urlValid = computed(() => /^https?:\/\/\S+$/.test(form.url.trim()))
const canSave = computed(
  () =>
    !busy.value &&
    nameValid.value &&
    !nameTaken.value &&
    urlValid.value &&
    (!isNew.value || !!form.token.trim()) &&
    (isNew.value || dirty.value),
)

/** Edits go to the serving machine; its settings are the page's own only
 *  while it is the one shown. */
async function edit(method: string, params: Record<string, unknown>): Promise<boolean> {
  try {
    const info = await homeRequest<SettingsInfo>(method, params)
    if (!machines.current) onSettingsUpdated({ settings: info })
    return true
  } catch (err) {
    toast('error', t('settings.saveError'), (err as Error).message)
    return false
  }
}

async function save(): Promise<void> {
  if (!canSave.value) return
  busy.value = true
  const name = form.name.trim()
  const ok = await edit('settings.set_machine', { name, url: form.url.trim(), token: form.token.trim() })
  busy.value = false
  if (ok) editing.value = null
}

async function remove(): Promise<void> {
  if (!saved.value) return
  busy.value = true
  const ok = await edit('settings.remove_machine', { name: saved.value.name })
  busy.value = false
  if (ok) editing.value = null
}

/** A machine's state in a word, from its light connection. */
function status(name: string): { text: string; bad: boolean } {
  const m = machineState(name)
  if (!m || !m.connected) return { text: t('settings.machine_offline'), bad: true }
  if (m.protocol === null) return { text: t('settings.machine_connecting'), bad: false }
  if (!compatible(m)) return { text: t('settings.machine_mismatch'), bad: true }
  return { text: t('settings.machine_ready', { n: m.sessions.length }), bad: false }
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div class="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-4 pt-5 pb-6 sm:px-6">
      <Transition v-bind="SWAP" mode="out-in">
        <!-- the list -->
        <div v-if="editing === null" class="flex flex-col gap-2">
          <button
            v-for="m in list"
            :key="m.name"
            type="button"
            class="flex items-center gap-3 rounded-card border border-line-soft bg-surface px-3 py-2.5 text-left transition-colors hover:bg-surface-2"
            @click="open(m)"
          >
            <Icon name="machine" :size="14" class="flex-none text-ink-3" />
            <span class="font-mono font-semibold text-ink">{{ m.name }}</span>
            <span class="ml-auto text-meta" :class="status(m.name).bad ? 'text-danger' : 'text-ink-3'">{{
              status(m.name).text
            }}</span>
            <Icon name="chevron" :size="12" class="text-ink-3" />
          </button>
          <div class="flex items-center gap-2 pt-1">
            <button type="button" class="btn" @click="open(null)">
              <Icon name="plus" :size="13" />
              {{ t('settings.addMachine') }}
            </button>
          </div>
        </div>

        <!-- the machine's form -->
        <form v-else class="flex flex-col gap-5" @submit.prevent="save">
          <label v-if="isNew" class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('settings.mcpName') }}</span>
            <input
              v-model="form.name"
              type="text"
              class="w-full font-mono text-meta"
              placeholder="devbox"
              autocomplete="off"
              spellcheck="false"
            />
            <span v-if="form.name.trim() && !nameValid" class="text-meta text-danger">{{
              t('settings.mcpNameInvalid')
            }}</span>
            <span v-else-if="nameTaken" class="text-meta text-danger">{{ t('settings.mcpNameTaken') }}</span>
          </label>
          <label class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('settings.machineUrl') }}</span>
            <input
              v-model="form.url"
              type="url"
              class="w-full font-mono text-meta"
              placeholder="http://devbox:7878"
              autocomplete="off"
              spellcheck="false"
            />
          </label>
          <label class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('settings.machineToken') }}</span>
            <input
              v-model="form.token"
              type="password"
              class="w-full font-mono text-meta"
              :placeholder="saved ? `•••••••• ${t('settings.apiKeySaved')}` : '~/.rumu/token'"
              autocomplete="off"
              spellcheck="false"
            />
          </label>
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
