<script setup lang="ts">
// A project on the projects page (`/projects/<id>`, `?new` for a new one): its
// seal and name, its charter, its mounted workspaces (add a registered one or a
// folder, reorder — the first is the default — remove) and deleting it. A
// folder added here is registered only when the form is submitted.
import { computed, reactive, ref, watch } from 'vue'
import { t } from '../../i18n'
import { trackItemDirty } from '../../lib/layout'
import { replacePage } from '../../lib/router'
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
import { PROJECT_TONES, projectTone, toneColor } from '../../lib/projectIcon'
import type { Workspace } from '../../lib/types'
import FolderPicker from '../FolderPicker.vue'
import ProjectSeal from '../brand/ProjectSeal.vue'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'

/** `folders`: the workspaces to register on submit, name → root. `glyph`
 *  and `tone` are the seal's, blank for the defaults. */
const form = reactive<{
  name: string
  glyph: string
  tone: string
  workspaces: string[]
  folders: Record<string, string>
  private: boolean
}>({
  name: '',
  glyph: '',
  tone: '',
  workspaces: [],
  folders: {},
  private: false,
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
trackItemDirty(dirty)
const canSave = computed(() => !busy.value && (creating.value || dirty.value) && !!form.name.trim())

/** The seal as the form has it; a new project's default tone is fixed when
 *  the form opens, since its id (what the default comes from) is not known yet. */
const seal = computed(() => ({
  id: project.value?.id ?? '',
  name: form.name || '?',
  icon: { glyph: form.glyph.trim() || undefined, tone: form.tone || undefined },
}))

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
  form.glyph = p?.icon?.glyph ?? ''
  form.tone = p
    ? (p.icon?.tone ?? '')
    : PROJECT_TONES[Math.floor(Math.random() * (PROJECT_TONES.length - 1))]!
  form.workspaces = [...(p?.workspaces ?? [])]
  form.folders = {}
  form.private = p?.private ?? false
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
    const icon = { glyph: form.glyph.trim() || undefined, tone: form.tone || undefined }
    const changes = {
      name: form.name.trim(),
      workspaces: form.workspaces,
      icon,
      private: form.private,
    }
    const p = project.value
    if (p) {
      await updateProject(p.id, changes)
      reset()
    } else {
      const created = await createProject(changes.name, changes.workspaces, icon, form.private)
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
  <form class="flex min-h-0 flex-1 flex-col" @submit.prevent="save">
    <div class="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-4 pt-5 pb-6 sm:px-6">
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
        <div class="flex items-center gap-2.5">
          <Popover :label="t('projects.icon')" placement="bottom-start" menu>
            <template #trigger="{ open, toggle }">
              <button
                type="button"
                class="seal-btn group relative flex-none rounded-[11px] outline-none focus-visible:ring-3 focus-visible:ring-brand/30"
                :title="t('projects.icon')"
                :aria-label="t('projects.icon')"
                :aria-expanded="open"
                @click="toggle"
              >
                <ProjectSeal :project="seal" :size="36" />
                <span
                  class="absolute -right-1 -bottom-1 flex size-4 items-center justify-center rounded-full bg-surface text-ink-3 shadow-soft transition-colors group-hover:text-ink"
                >
                  <Icon name="edit" :size="10" />
                </span>
              </button>
            </template>
            <div class="flex flex-col gap-2.5 p-1.5">
              <div
                class="flex items-center justify-between"
                role="radiogroup"
                :aria-label="t('projects.tone')"
              >
                <button
                  v-for="tone in PROJECT_TONES"
                  :key="tone"
                  type="button"
                  role="radio"
                  class="swatch"
                  :style="{ '--tone': toneColor(tone) }"
                  :aria-checked="projectTone(seal) === tone"
                  :title="t(`projects.tones.${tone}`)"
                  :aria-label="t(`projects.tones.${tone}`)"
                  @click="form.tone = tone"
                />
              </div>
              <div class="flex items-center gap-2">
                <input
                  v-model="form.glyph"
                  type="text"
                  maxlength="4"
                  class="w-12 flex-none text-center font-card"
                  :placeholder="Array.from(form.name)[0]?.toUpperCase() ?? ''"
                  :title="t('projects.glyph')"
                  :aria-label="t('projects.glyph')"
                  autocomplete="off"
                  spellcheck="false"
                />
                <button
                  v-if="form.glyph || (project && form.tone)"
                  type="button"
                  class="ml-auto rounded-control px-1.5 py-0.5 text-meta font-semibold text-ink-3 transition-colors hover:bg-surface-2 hover:text-ink"
                  @click="((form.glyph = ''), project && (form.tone = ''))"
                >
                  {{ t('projects.resetIcon') }}
                </button>
              </div>
            </div>
          </Popover>
          <input
            id="project-name"
            v-model.trim="form.name"
            type="text"
            class="w-full"
            maxlength="80"
            autocomplete="off"
            spellcheck="false"
          />
        </div>
      </div>

      <label class="flex items-center gap-2 text-secondary">
        <input v-model="form.private" type="checkbox" />
        {{ t('projects.private') }}
        <span class="text-micro text-ink-3">{{ t('projects.privateHint') }}</span>
      </label>

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
</template>

<style scoped>
.swatch {
  width: 1.375rem;
  height: 1.375rem;
  border-radius: 999px;
  background: var(--tone);
  box-shadow:
    0 0 0 2px var(--surface),
    0 0 0 3px transparent;
  transition:
    box-shadow 0.15s,
    scale 0.15s var(--ease-spring);
}

.swatch:hover {
  scale: 1.1;
}

.swatch[aria-checked='true'] {
  box-shadow:
    0 0 0 2px var(--surface),
    0 0 0 3.5px var(--tone);
}

.swatch:focus-visible {
  outline: 2px solid var(--brand);
  outline-offset: 3px;
}
</style>
