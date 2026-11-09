<script setup lang="ts">
// The projects page (`/projects/<id>`, `?new` for a new one): a project's
// name, its charter, its mounted workspaces (add a registered one or a
// folder, reorder — the first is the default — remove) and deleting it. A
// folder added here is registered only when the form is submitted.
import { computed, reactive, ref, watch } from 'vue'
import { t } from '../../i18n'
import { openPage, pageUp, replacePage } from '../../lib/router'
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
} from '../../lib/store'
import type { Workspace } from '../../lib/types'
import FolderPicker from '../FolderPicker.vue'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'
import PageSheet from './PageSheet.vue'

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
/** The folder picker is open, to add a folder as a workspace. */
const addingFolder = ref(false)

const creating = computed(() => !!state.page?.fresh)
/** The project open on the page (none for a new one, or a stale id). */
const project = computed(() =>
  creating.value ? null : (state.projects.find((p) => p.id === state.page?.item) ?? null),
)
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

/** Load the form from the open project (a blank one when adding). */
function reset(): void {
  error.value = ''
  pendingDelete.value = false
  addingFolder.value = false
  const p = project.value
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
  addingFolder.value = false
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
      reset()
    } else {
      const created = await createProject(changes.name, changes.workspaces)
      chooseLandingProject(created.id)
      replacePage(created.id)
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
    replacePage(null)
  } catch (err) {
    error.value = `${t('projects.deleteError')}: ${(err as Error).message}`
  } finally {
    busy.value = false
  }
}

// The form follows the open project, once the list names it.
watch([creating, () => project.value?.id], reset, { immediate: true })
</script>

<template>
  <PageSheet
    :title="t('projects.title')"
    :count="state.projects.length"
    icon="mount"
    tone="text-tool"
    :detail="creating || !!project"
    :pick="t('projects.pick')"
    @back="pageUp"
  >
    <template #list>
      <div class="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto p-3">
        <button
          v-for="p in state.projects"
          :key="p.id"
          type="button"
          class="flex flex-col rounded-control px-2.5 py-2 text-left transition-colors duration-150"
          :class="
            p.id === project?.id
              ? 'bg-surface font-semibold text-ink shadow-soft'
              : 'text-ink-2 hover:bg-surface/70 hover:text-ink'
          "
          @click="openPage('projects', p.id)"
        >
          <span class="truncate text-secondary">{{ p.name }}</span>
          <span class="truncate font-card text-micro font-normal text-ink-3">{{
            p.workspaces.join(' · ') || t('projects.noWorkspaces')
          }}</span>
        </button>
        <button
          type="button"
          class="mt-1 flex items-center gap-2.5 rounded-control border border-dashed px-2.5 py-2 text-left text-secondary transition-colors duration-150"
          :class="
            creating
              ? 'border-brand/50 bg-brand-soft/50 font-semibold text-ink'
              : 'border-line text-ink-3 hover:border-ink-3 hover:text-ink'
          "
          @click="openPage('projects', null, true)"
        >
          <Icon name="plus" :size="14" />
          {{ t('projects.add') }}
        </button>
      </div>
    </template>

    <form class="flex min-h-0 flex-1 flex-col" @submit.prevent="save">
      <header class="flex flex-none items-center gap-3 px-4 pt-5 pb-1 sm:px-6">
        <h2 class="min-w-0 truncate text-title font-bold">
          {{ creating ? t('projects.add') : project?.name }}
        </h2>
      </header>

      <div class="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-4 pt-3 pb-6 sm:px-6">
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

          <div v-if="addingFolder" class="flex flex-col gap-2">
            <FolderPicker ref="picker" start="~" class="max-h-64" />
            <div class="flex justify-end gap-2">
              <button type="button" class="btn" @click="addingFolder = false">
                {{ t('common.cancel') }}
              </button>
              <button type="button" class="btn btn-primary" :disabled="!picker?.path" @click="addFolder">
                {{ t('projects.addFolderConfirm') }}
              </button>
            </div>
          </div>
          <div v-else class="flex items-center gap-2">
            <button type="button" class="btn flex-none" @click="addingFolder = true">
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

      <footer class="flex flex-none flex-col gap-2 border-t border-line-soft px-4 py-3 sm:px-6">
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
          <button v-if="dirty && project" type="button" class="btn ml-auto" @click="reset">
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
  </PageSheet>
</template>
