<script setup lang="ts">
// A workspace in the registry (`workspaces.yaml`) on the projects page: add,
// edit or delete it (`/workspaces/<name>`, `?new` to add one). Adding starts at the
// folder; the name follows it (made unique) until typed by hand. Names are
// keys: an existing workspace keeps its name.
import { computed, reactive, ref, watch } from 'vue'
import { t } from '../../i18n'
import { replacePage } from '../../lib/router'
import { deleteWorkspace, saveWorkspace, state } from '../../lib/store'
import type { Workspace } from '../../lib/types'
import FolderPicker from '../FolderPicker.vue'
import Icon from '../ui/Icon.vue'

interface Form {
  name: string
  remote: boolean
  ssh: string
  root: string
  read_only: boolean
}

const isNew = computed(() => !!state.page?.fresh)
/** The workspace open on the page (none for a new one, or a stale name). */
const current = computed(() =>
  isNew.value ? null : (state.workspaces.find((w) => w.name === state.page?.item) ?? null),
)
const editing = computed(() => current.value?.name ?? '')
const form = reactive<Form>(blank())
const pristine = ref(JSON.stringify(form))
const pendingDelete = ref(false)
const busy = ref(false)
const error = ref('')
/** The folder picker is open (local roots only). */
const browsing = ref(false)
/** The name was typed by hand; until then it follows the folder. */
const nameEdited = ref(false)

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

/** Load the form from the open workspace (a blank one when adding). */
function reset(): void {
  error.value = ''
  pendingDelete.value = false
  const w = current.value
  Object.assign(form, w ? fromWorkspace(w) : blank())
  pristine.value = JSON.stringify(form)
  browsing.value = !w
  nameEdited.value = false
}

// A new workspace untouched by hand takes its name from the folder.
watch(
  () => form.root,
  (root) => {
    if (isNew.value && !nameEdited.value) form.name = folderName(root)
  },
)

function onNameInput(): void {
  nameEdited.value = !!form.name
  if (!nameEdited.value) form.name = folderName(form.root)
}

/** The workspace name a folder suggests: its basename folded to the name
 *  rule, with a number added when taken ('' when it suggests none). */
function folderName(path: string): string {
  const base = path.split(/[\\/]/).filter(Boolean).pop() ?? ''
  const name = base
    .toLowerCase()
    .replace(/[^a-z0-9.-]+/g, '-')
    .replace(/^[^a-z]+/, '')
    .replace(/[.-]+$/, '')
  if (!NAME.test(name)) return ''
  const taken = (n: string): boolean => state.workspaces.some((w) => w.name === n)
  let unique = name
  for (let i = 2; taken(unique); i++) unique = `${name}-${i}`
  return unique
}

async function save(): Promise<void> {
  if (!canSave.value) return
  busy.value = true
  error.value = ''
  try {
    const ws = await saveWorkspace({
      name: form.name,
      root: form.root.trim(),
      ssh: form.remote ? form.ssh.trim() : undefined,
      read_only: form.read_only,
    })
    if (isNew.value) replacePage(ws.name)
    else reset()
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
    replacePage(null)
  } catch (err) {
    error.value = `${t('workspaces.deleteError')}: ${(err as Error).message}`
  } finally {
    busy.value = false
  }
}

// The form follows the open workspace, once the registry names it.
watch([isNew, () => current.value?.name], reset, { immediate: true })
</script>

<template>
  <form class="flex min-h-0 min-w-0 flex-1 flex-col" @submit.prevent="save">
    <header class="flex flex-none items-center gap-3 px-4 pt-5 pb-1 sm:px-6">
      <h3 class="min-w-0 truncate text-title font-bold" :class="{ 'font-card': !isNew }">
        {{ isNew ? t('workspaces.add') : editing }}
      </h3>
    </header>

    <div class="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-4 pt-3 pb-6 sm:px-6">
      <div class="flex flex-col gap-1.5">
        <div class="flex items-center gap-2">
          <label for="workspace-root" class="field-label">{{ t('workspaces.root') }}</label>
          <div
            class="ml-auto flex flex-none rounded-control bg-surface-2 p-0.5"
            role="radiogroup"
            :aria-label="t('workspaces.location')"
          >
            <button
              v-for="remote in [false, true]"
              :key="String(remote)"
              type="button"
              role="radio"
              :aria-checked="form.remote === remote"
              class="rounded-xs px-2 py-0.5 text-micro font-semibold transition-colors duration-150"
              :class="
                form.remote === remote ? 'bg-surface text-ink shadow-soft' : 'text-ink-3 hover:text-ink'
              "
              @click="form.remote = remote"
            >
              {{ remote ? t('workspaces.ssh') : t('workspaces.local') }}
            </button>
          </div>
        </div>
        <input
          v-if="form.remote"
          v-model="form.ssh"
          type="text"
          class="w-full font-card"
          placeholder="user@host"
          :aria-label="t('workspaces.host')"
          autocomplete="off"
          spellcheck="false"
        />
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
          class="max-h-56"
          :key="editing"
          :start="form.root"
          @pick="form.root = $event"
        />
      </div>

      <label v-if="isNew" class="flex flex-col gap-1.5">
        <span class="field-label">{{ t('workspaces.name') }}</span>
        <input
          v-model.trim="form.name"
          type="text"
          class="w-full font-card"
          @input="onNameInput"
          autocomplete="off"
          spellcheck="false"
        />
        <span v-if="nameInvalid" class="field-hint text-danger">{{ t('workspaces.nameHint') }}</span>
        <span v-else-if="nameTaken" class="field-hint text-danger">{{ t('workspaces.nameTaken') }}</span>
      </label>

      <label class="flex items-center gap-2 text-secondary">
        <input v-model="form.read_only" type="checkbox" />
        {{ t('workspaces.readOnly') }}
      </label>
    </div>

    <footer class="flex flex-none flex-col gap-2 border-t border-line-soft px-4 py-3 sm:px-6">
      <p v-if="error" class="text-meta text-danger">{{ error }}</p>
      <div v-if="pendingDelete" class="flex items-center gap-3">
        <Icon name="trash" :size="14" class="flex-none text-danger" />
        <div class="flex min-w-0 flex-1 flex-col">
          <p class="text-meta font-semibold text-ink">
            {{ t('workspaces.deletePrompt', { name: editing }) }}
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
        <button v-if="dirty && !isNew" type="button" class="btn ml-auto" @click="reset">
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
</template>
