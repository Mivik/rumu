<script setup lang="ts">
// A project's dialog: a new one (`projectSettingsFor` empty), or an existing
// one's settings. Its name, its charter, its mounted workspaces (add a
// registered one or a folder, reorder — the first is the default — remove)
// and deleting it. A folder added here is registered only when the dialog
// is submitted.
import { computed, reactive, ref, watch } from 'vue'
import { t } from '../i18n'
import { closeProjectSettings, layout } from '../lib/layout'
import {
  chooseLandingProject,
  createProject,
  deleteProject,
  folderProjectName,
  folderWorkspaceName,
  openCardWindow,
  saveWorkspace,
  state,
  updateProject,
} from '../lib/store'
import type { Workspace } from '../lib/types'
import FolderPicker from './FolderPicker.vue'
import Icon from './ui/Icon.vue'
import Modal from './ui/Modal.vue'
import Popover from './ui/Popover.vue'

/** `folders`: the workspaces to register on submit, name → root. */
const form = reactive<{ name: string; workspaces: string[]; folders: Record<string, string> }>({
  name: '',
  workspaces: [],
  folders: {},
})
const pristine = ref(JSON.stringify(form))
const busy = ref(false)
const error = ref('')
const pendingDelete = ref(false)
const picker = ref<InstanceType<typeof FolderPicker> | null>(null)

const project = computed(() => state.projects.find((p) => p.id === layout.projectSettingsFor) ?? null)
const creating = computed(() => !layout.projectSettingsFor)
const dirty = computed(() => JSON.stringify(form) !== pristine.value)
const nameInvalid = computed(() => !!form.name && !/^[a-z][a-z0-9-]*$/.test(form.name))
const canSave = computed(
  () => !busy.value && (creating.value || dirty.value) && !!form.name.trim() && !nameInvalid.value,
)

/** The registered workspaces not in the project yet. */
const available = computed(() => state.workspaces.filter((w) => !form.workspaces.includes(w.name)))

function where(w: Workspace): string {
  return w.ssh ? `${w.ssh}:${w.root}` : w.root
}

function location(name: string): string {
  const w = state.workspaces.find((x) => x.name === name)
  return w ? where(w) : (form.folders[name] ?? '')
}

function choose(id: string): void {
  error.value = ''
  pendingDelete.value = false
  const p = state.projects.find((x) => x.id === id)
  form.name = p?.name ?? ''
  form.workspaces = [...(p?.workspaces ?? [])]
  form.folders = {}
  pristine.value = JSON.stringify(form)
}

function add(name: string): void {
  if (!form.workspaces.includes(name)) form.workspaces.push(name)
}

/** Add a registered workspace; a new project with no name yet takes its. */
function addExisting(name: string, close: () => void): void {
  add(name)
  if (creating.value && !form.name) form.name = folderProjectName(name)
  close()
}

/** Add the picked folder as a workspace named after it; a new project with
 *  no name yet takes the folder's. */
function addFolder(): void {
  const root = picker.value?.path
  if (!root) return
  const existing = state.workspaces.find((w) => !w.ssh && w.root === root)
  if (existing) add(existing.name)
  else {
    const taken = [...state.workspaces.map((w) => w.name), ...Object.keys(form.folders)]
    const name = folderWorkspaceName(root, taken)
    form.folders[name] = root
    form.workspaces.push(name)
  }
  if (creating.value && !form.name) form.name = folderProjectName(root)
  layout.projectFolderOpen = false
}

function removeAt(index: number): void {
  const [name] = form.workspaces.splice(index, 1)
  if (name) delete form.folders[name]
}

/** Move one step; the first is the default. */
function move(index: number, delta: number): void {
  const to = index + delta
  if (to < 0 || to >= form.workspaces.length) return
  const [name] = form.workspaces.splice(index, 1)
  form.workspaces.splice(to, 0, name!)
}

function charter(): void {
  if (project.value) openCardWindow(project.value.id, 'about')
}

async function save(): Promise<void> {
  if (!canSave.value) return
  busy.value = true
  error.value = ''
  try {
    for (const [name, root] of Object.entries(form.folders))
      await saveWorkspace({ name, root, read_only: false })
    form.folders = {}
    const changes = { name: form.name.trim(), workspaces: form.workspaces }
    const p = project.value
    if (p) {
      await updateProject(p.id, changes)
      choose(p.id)
    } else {
      chooseLandingProject((await createProject(changes.name, changes.workspaces)).id)
      closeProjectSettings()
    }
  } catch (err) {
    error.value = `${t(creating.value ? 'projects.createError' : 'projects.saveError')}: ${(err as Error).message}`
  } finally {
    busy.value = false
  }
}

async function remove(): Promise<void> {
  const p = project.value
  if (!p) return
  busy.value = true
  error.value = ''
  try {
    await deleteProject(p.id)
    closeProjectSettings()
  } catch (err) {
    error.value = `${t('projects.deleteError')}: ${(err as Error).message}`
  } finally {
    busy.value = false
  }
}

watch(
  () => layout.projectSettingsOpen,
  (open) => {
    if (open) choose(layout.projectSettingsFor)
    else {
      pendingDelete.value = false
      error.value = ''
    }
  },
)
</script>

<template>
  <Modal
    :open="layout.projectSettingsOpen"
    :label="creating ? t('projects.add') : t('projects.title')"
    :z="55"
    wrapper-class="p-3 sm:p-6"
    panel-class="h-[min(84dvh,560px)] w-full max-w-[680px] rounded-sheet"
    @close="closeProjectSettings"
  >
    <form class="flex min-h-0 flex-1 flex-col" @submit.prevent="save">
      <header class="flex flex-none items-center gap-3 px-6 pt-5 pb-1">
        <h2 class="min-w-0 truncate text-title font-bold">
          {{ creating ? t('projects.add') : (project?.name ?? t('projects.title')) }}
        </h2>
        <button
          type="button"
          class="icon-btn ml-auto size-8 flex-none"
          :title="t('common.close')"
          :aria-label="t('common.close')"
          @click="closeProjectSettings"
        >
          <Icon name="close" :size="14" />
        </button>
      </header>

      <div class="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-6 pt-3 pb-6">
        <div class="flex flex-col gap-1.5">
          <div class="flex items-center gap-2">
            <label for="project-name" class="field-label">{{ t('projects.name') }}</label>
            <button
              v-if="project"
              type="button"
              class="-mr-1.5 ml-auto flex items-center gap-1 rounded-control px-1.5 py-0.5 text-meta font-semibold text-ink-3 transition-colors duration-150 hover:bg-surface-2 hover:text-ink"
              @click="charter"
            >
              <Icon name="note" :size="12" />
              {{ t('projects.charter') }}
            </button>
          </div>
          <input
            id="project-name"
            v-model.trim="form.name"
            type="text"
            class="w-full"
            autocomplete="off"
            spellcheck="false"
          />
          <span v-if="nameInvalid" class="field-hint text-danger">{{ t('projects.nameHint') }}</span>
        </div>

        <div class="flex flex-col gap-1.5">
          <span class="field-label">{{ t('projects.workspaces') }}</span>

          <div class="flex flex-col gap-0.5">
            <div
              v-for="(name, i) in form.workspaces"
              :key="name"
              class="group flex items-center gap-2 rounded-control border border-line-soft px-2.5 py-1.5"
            >
              <span
                v-if="i === 0"
                class="flex-none rounded-xs bg-surface-2 px-1 text-micro font-semibold text-ink-3"
                >{{ t('projects.default') }}</span
              >
              <span class="flex-none font-card text-secondary">{{ name }}</span>
              <span class="min-w-0 flex-1 truncate text-micro text-ink-3">{{ location(name) }}</span>
              <button
                type="button"
                class="icon-btn size-6 flex-none text-ink-3 disabled:opacity-30"
                :title="t('projects.moveUp')"
                :aria-label="t('projects.moveUp')"
                :disabled="i === 0"
                @click="move(i, -1)"
              >
                <Icon name="up" :size="12" />
              </button>
              <button
                type="button"
                class="icon-btn size-6 flex-none text-ink-3 disabled:opacity-30"
                :title="t('projects.moveDown')"
                :aria-label="t('projects.moveDown')"
                :disabled="i === form.workspaces.length - 1"
                @click="move(i, 1)"
              >
                <Icon name="up" :size="12" class="rotate-180" />
              </button>
              <button
                type="button"
                class="icon-btn size-6 flex-none text-ink-3 hover:text-danger"
                :title="t('projects.removeWorkspace')"
                :aria-label="t('projects.removeWorkspace')"
                @click="removeAt(i)"
              >
                <Icon name="close" :size="12" />
              </button>
            </div>
            <p v-if="!form.workspaces.length" class="px-1 text-meta text-ink-3">
              {{ t('projects.noWorkspaces') }}
            </p>
          </div>

          <div class="flex items-center gap-2">
            <button type="button" class="btn flex-none" @click="layout.projectFolderOpen = true">
              <Icon name="plus" :size="13" />
              {{ t('projects.addFolder') }}
            </button>
            <Popover
              v-if="available.length"
              role="menu"
              :label="t('projects.addWorkspace')"
              placement="bottom-start"
            >
              <template #trigger="{ open, toggle }">
                <button
                  type="button"
                  class="-mr-1.5 ml-auto flex items-center gap-1 rounded-control px-1.5 py-0.5 text-meta font-semibold text-ink-3 transition-colors duration-150 hover:bg-surface-2 hover:text-ink"
                  aria-haspopup="menu"
                  :aria-expanded="open"
                  @click="toggle"
                >
                  {{ t('projects.addWorkspace') }}
                  <Icon name="down" :size="12" />
                </button>
              </template>
              <template #default="{ close }">
                <div class="-m-1.5">
                  <button
                    v-for="w in available"
                    :key="w.name"
                    type="button"
                    role="menuitem"
                    class="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left transition-colors duration-150 hover:bg-surface-2"
                    @click="addExisting(w.name, close)"
                  >
                    <span class="flex-none font-card text-secondary text-ink-2">{{ w.name }}</span>
                    <span class="min-w-0 flex-1 truncate text-micro text-ink-3">{{ where(w) }}</span>
                  </button>
                </div>
              </template>
            </Popover>
          </div>
        </div>
      </div>

      <footer class="flex flex-none flex-col gap-2 border-t border-line-soft px-6 py-3">
        <p v-if="error" class="text-meta text-danger">{{ error }}</p>
        <div v-if="pendingDelete" class="flex items-center gap-3">
          <Icon name="trash" :size="14" class="flex-none text-danger" />
          <p class="min-w-0 flex-1 text-meta font-semibold text-ink">
            {{ t('projects.deletePrompt', { name: project?.name ?? '' }) }}
          </p>
          <button type="button" class="btn" @click="pendingDelete = false">
            {{ t('common.cancel') }}
          </button>
          <button type="button" class="btn btn-danger" :disabled="busy" @click="remove">
            {{ t('projects.deleteConfirm') }}
          </button>
        </div>
        <div v-else class="flex items-center gap-2">
          <button
            v-if="project"
            type="button"
            class="-ml-2 flex items-center gap-1.5 rounded-control px-2 py-1.5 text-meta font-semibold text-ink-3 transition-colors duration-150 hover:bg-danger-soft hover:text-danger"
            @click="pendingDelete = true"
          >
            <Icon name="trash" :size="14" />
            {{ t('projects.delete') }}
          </button>
          <button
            v-if="dirty && project"
            type="button"
            class="btn ml-auto"
            @click="choose(layout.projectSettingsFor)"
          >
            {{ t('projects.discard') }}
          </button>
          <button
            type="submit"
            class="btn btn-primary"
            :class="{ 'ml-auto': !(dirty && project) }"
            :disabled="!canSave"
          >
            {{ creating ? t('projects.create') : t('projects.save') }}
          </button>
        </div>
      </footer>
    </form>
  </Modal>

  <Modal
    :open="layout.projectFolderOpen"
    :label="t('projects.addFolder')"
    :z="57"
    wrapper-class="p-3 sm:p-6"
    panel-class="h-[min(72dvh,480px)] w-full max-w-[560px] rounded-sheet"
    @close="layout.projectFolderOpen = false"
  >
    <header class="flex flex-none items-center gap-3 px-5 pt-4 pb-3">
      <h3 class="min-w-0 truncate text-title font-bold">{{ t('projects.addFolder') }}</h3>
      <button
        type="button"
        class="icon-btn ml-auto size-8 flex-none"
        :title="t('common.close')"
        :aria-label="t('common.close')"
        @click="layout.projectFolderOpen = false"
      >
        <Icon name="close" :size="14" />
      </button>
    </header>
    <FolderPicker ref="picker" start="~" class="mx-5 min-h-0 flex-1" />
    <footer class="flex flex-none justify-end gap-2 px-5 py-4">
      <button type="button" class="btn" @click="layout.projectFolderOpen = false">
        {{ t('common.cancel') }}
      </button>
      <button type="button" class="btn btn-primary" :disabled="!picker?.path" @click="addFolder">
        {{ t('projects.addFolderConfirm') }}
      </button>
    </footer>
  </Modal>
</template>
