<script setup lang="ts">
// The workspace registry dialog (`workspaces.yaml`): add, edit and delete
// workspaces. A workspace added from the composer also joins the open
// branch. Names are keys: an existing workspace keeps its name.
import { computed, reactive, ref, watch } from 'vue'
import { t } from '../i18n'
import { closeWorkspaceSettings, layout } from '../lib/layout'
import {
  attachWorkspaces,
  deleteWorkspace,
  refreshAssistants,
  refreshWorkspaces,
  saveWorkspace,
  state,
} from '../lib/store'
import type { Workspace } from '../lib/types'
import FolderPicker from './FolderPicker.vue'
import Icon from './ui/Icon.vue'
import Modal from './ui/Modal.vue'

interface Form {
  name: string
  remote: boolean
  ssh: string
  root: string
  read_only: boolean
}

const editing = ref('')
const form = reactive<Form>(blank())
const pristine = ref(JSON.stringify(form))
const pendingDelete = ref(false)
const busy = ref(false)
const error = ref('')
/** The folder picker is open (local roots only). */
const browsing = ref(false)

const isNew = computed(() => !editing.value)
const dirty = computed(() => JSON.stringify(form) !== pristine.value)
const NAME = /^[a-z][a-z0-9.-]+$/
const nameInvalid = computed(() => isNew.value && !!form.name && !NAME.test(form.name))
const nameTaken = computed(() => isNew.value && state.workspaces.some((w) => w.name === form.name))
const canSave = computed(
  () =>
    !busy.value &&
    dirty.value &&
    NAME.test(form.name) &&
    !nameTaken.value &&
    !!form.root.trim() &&
    (!form.remote || !!form.ssh.trim()),
)
/** Assistants that lose the workspace when it is deleted. */
const users = computed(() =>
  state.assistants.filter((a) => a.workspaces.includes(editing.value)).map((a) => a.name),
)

function blank(): Form {
  return {
    name: '',
    remote: false,
    ssh: '',
    root: '',
    read_only: false,
  }
}

function fromWorkspace(w: Workspace): Form {
  return {
    name: w.name,
    remote: !!w.ssh,
    ssh: w.ssh ?? '',
    root: w.root,
    read_only: w.read_only,
  }
}

function choose(name: string): void {
  error.value = ''
  pendingDelete.value = false
  const w = state.workspaces.find((x) => x.name === name)
  editing.value = w ? name : ''
  Object.assign(form, w ? fromWorkspace(w) : blank())
  pristine.value = JSON.stringify(form)
  browsing.value = !w
}

function where(w: Workspace): string {
  return w.ssh ? `${w.ssh}:${w.root}` : w.root
}

/** A folder picked in the picker; a new workspace untouched by hand takes
 *  its name from the folder. */
function pickRoot(path: string): void {
  if (isNew.value && (!form.name || form.name === folderName(form.root))) form.name = folderName(path)
  form.root = path
}

/** The workspace name a folder suggests (its basename, folded to the name
 *  rule), or '' when it suggests none. */
function folderName(path: string): string {
  const base = path.split('/').filter(Boolean).pop() ?? ''
  const name = base.toLowerCase().replace(/[^a-z0-9.-]+/g, '-')
  return NAME.test(name) ? name : ''
}

async function save(): Promise<void> {
  if (!canSave.value) return
  busy.value = true
  error.value = ''
  const created = isNew.value
  try {
    const ws = await saveWorkspace({
      name: form.name,
      root: form.root.trim(),
      ssh: form.remote ? form.ssh.trim() : undefined,
      read_only: form.read_only,
    })
    if (created && layout.workspaceSettingsAttach && state.pathState) {
      await attachWorkspaces([...state.pathState.attached, ws.name])
      closeWorkspaceSettings()
      return
    }
    choose(ws.name)
  } catch (err) {
    error.value = `${t('workspaces.saveError')}: ${(err as Error).message}`
  } finally {
    busy.value = false
  }
}

async function remove(): Promise<void> {
  busy.value = true
  error.value = ''
  try {
    await deleteWorkspace(editing.value)
    await refreshAssistants()
    choose(state.workspaces[0]?.name ?? '')
  } catch (err) {
    error.value = `${t('workspaces.deleteError')}: ${(err as Error).message}`
  } finally {
    busy.value = false
  }
}

watch(
  () => layout.workspaceSettingsOpen,
  async (open) => {
    if (!open) return
    choose(layout.workspaceSettingsFor)
    await Promise.all([refreshWorkspaces(), refreshAssistants()])
    if (!dirty.value) choose(layout.workspaceSettingsFor)
  },
)
</script>

<template>
  <Modal
    :open="layout.workspaceSettingsOpen"
    :label="t('workspaces.title')"
    :z="55"
    wrapper-class="p-3 sm:p-6"
    panel-class="h-[min(84dvh,560px)] w-full max-w-[760px] rounded-sheet"
    @close="closeWorkspaceSettings"
  >
    <div class="flex min-h-0 flex-1 flex-col sm:flex-row">
      <nav
        class="flex flex-none flex-col border-b border-line-soft bg-canvas/40 sm:w-60 sm:border-r sm:border-b-0"
      >
        <header class="flex items-center gap-2 px-5 pt-5 pb-3">
          <h2 class="text-title font-bold">{{ t('workspaces.title') }}</h2>
          <span class="text-meta font-semibold text-ink-3 tabular-nums">{{ state.workspaces.length }}</span>
        </header>
        <div class="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-3 pb-3">
          <button
            v-for="w in state.workspaces"
            :key="w.name"
            type="button"
            class="flex flex-col rounded-control px-2.5 py-2 text-left transition-colors duration-150"
            :class="
              w.name === editing
                ? 'bg-surface font-semibold text-ink shadow-soft'
                : 'text-ink-2 hover:bg-surface/70 hover:text-ink'
            "
            @click="choose(w.name)"
          >
            <span class="truncate font-card text-secondary">{{ w.name }}</span>
            <span class="truncate font-card text-micro font-normal text-ink-3">{{ where(w) }}</span>
          </button>
          <button
            type="button"
            class="mt-1 flex items-center gap-2.5 rounded-control border border-dashed px-2.5 py-2 text-left text-secondary transition-colors duration-150"
            :class="
              isNew
                ? 'border-brand/50 bg-brand-soft/50 font-semibold text-ink'
                : 'border-line text-ink-3 hover:border-ink-3 hover:text-ink'
            "
            @click="choose('')"
          >
            <Icon name="plus" :size="14" />
            {{ t('workspaces.add') }}
          </button>
        </div>
      </nav>

      <form class="flex min-h-0 min-w-0 flex-1 flex-col" @submit.prevent="save">
        <header class="flex flex-none items-center gap-3 px-6 pt-5 pb-1">
          <h3 class="min-w-0 truncate text-title font-bold" :class="{ 'font-card': !isNew }">
            {{ isNew ? t('workspaces.add') : editing }}
          </h3>
          <button
            type="button"
            class="icon-btn ml-auto size-8 flex-none"
            :title="t('common.close')"
            :aria-label="t('common.close')"
            @click="closeWorkspaceSettings"
          >
            <Icon name="close" :size="14" />
          </button>
        </header>

        <div class="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-6 pt-3 pb-6">
          <label v-if="isNew" class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('workspaces.name') }}</span>
            <input
              v-model.trim="form.name"
              type="text"
              class="w-full font-card"
              autocomplete="off"
              spellcheck="false"
            />
            <span v-if="nameInvalid" class="field-hint text-danger">{{ t('workspaces.nameHint') }}</span>
            <span v-else-if="nameTaken" class="field-hint text-danger">{{ t('workspaces.nameTaken') }}</span>
          </label>

          <div class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('workspaces.location') }}</span>
            <div class="flex items-center gap-2">
              <div class="flex flex-none rounded-control bg-surface-2 p-0.5" role="radiogroup">
                <button
                  v-for="remote in [false, true]"
                  :key="String(remote)"
                  type="button"
                  role="radio"
                  :aria-checked="form.remote === remote"
                  class="rounded-xs px-2.5 py-1 text-meta font-semibold transition-colors duration-150"
                  :class="
                    form.remote === remote ? 'bg-surface text-ink shadow-soft' : 'text-ink-3 hover:text-ink'
                  "
                  @click="form.remote = remote"
                >
                  {{ remote ? t('workspaces.ssh') : t('workspaces.local') }}
                </button>
              </div>
              <input
                v-if="form.remote"
                v-model="form.ssh"
                type="text"
                class="min-w-0 flex-1 font-card"
                placeholder="user@host"
                :aria-label="t('workspaces.host')"
                autocomplete="off"
                spellcheck="false"
              />
            </div>
          </div>

          <div class="flex flex-col gap-1.5">
            <label for="workspace-root" class="field-label">{{ t('workspaces.root') }}</label>
            <div class="flex items-center gap-2">
              <input
                id="workspace-root"
                v-model="form.root"
                type="text"
                class="min-w-0 flex-1 font-card"
                :placeholder="form.remote ? '/srv/app' : '~/code/proj'"
                autocomplete="off"
                spellcheck="false"
              />
              <button
                v-if="!form.remote"
                type="button"
                class="icon-btn size-8 flex-none"
                :class="{ 'bg-surface-2 text-ink': browsing }"
                :title="t('workspaces.browse')"
                :aria-label="t('workspaces.browse')"
                :aria-pressed="browsing"
                @click="browsing = !browsing"
              >
                <Icon name="mount" :size="15" />
              </button>
            </div>
            <FolderPicker
              v-if="browsing && !form.remote"
              :key="editing"
              :start="form.root"
              @pick="pickRoot"
            />
          </div>

          <label class="flex items-center gap-2 text-secondary">
            <input v-model="form.read_only" type="checkbox" />
            {{ t('workspaces.readOnly') }}
          </label>
        </div>

        <footer class="flex flex-none flex-col gap-2 border-t border-line-soft px-6 py-3">
          <p v-if="error" class="text-meta text-danger">{{ error }}</p>
          <div v-if="pendingDelete" class="flex items-center gap-3">
            <Icon name="trash" :size="14" class="flex-none text-danger" />
            <div class="flex min-w-0 flex-1 flex-col">
              <p class="text-meta font-semibold text-ink">
                {{ t('workspaces.deletePrompt', { name: editing }) }}
              </p>
              <p v-if="users.length" class="truncate text-micro text-ink-3">
                {{ t('workspaces.deleteAssistants', { names: users.join(', ') }) }}
              </p>
            </div>
            <button type="button" class="btn" @click="pendingDelete = false">
              {{ t('common.cancel') }}
            </button>
            <button type="button" class="btn btn-danger" :disabled="busy" @click="remove">
              {{ t('workspaces.deleteConfirm') }}
            </button>
          </div>
          <div v-else class="flex items-center gap-2">
            <button
              v-if="!isNew"
              type="button"
              class="-ml-2 flex items-center gap-1.5 rounded-control px-2 py-1.5 text-meta font-semibold text-ink-3 transition-colors duration-150 hover:bg-danger-soft hover:text-danger"
              @click="pendingDelete = true"
            >
              <Icon name="trash" :size="14" />
              {{ t('workspaces.delete') }}
            </button>
            <button v-if="dirty && !isNew" type="button" class="btn ml-auto" @click="choose(editing)">
              {{ t('workspaces.discard') }}
            </button>
            <button
              type="submit"
              class="btn btn-primary"
              :class="{ 'ml-auto': !dirty || isNew }"
              :disabled="!canSave"
            >
              {{ isNew ? t('workspaces.create') : t('workspaces.save') }}
            </button>
          </div>
        </footer>
      </form>
    </div>
  </Modal>
</template>
