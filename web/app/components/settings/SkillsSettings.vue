<script setup lang="ts">
// Skills: the repositories installed from git (each updated or removed
// here), and every skill found, installed or in `~/.agents/skills` and
// `~/.claude/skills`, with why one is unusable.
import { computed, onBeforeUnmount, ref, watchEffect } from 'vue'
import { t } from '../../i18n'
import { layout } from '../../lib/layout'
import { editSettings, models } from '../../lib/state/models'
import Icon from '../ui/Icon.vue'

const info = computed(() => models.settings?.skills ?? { skills: [], repos: [] })

const url = ref('')
const name = ref('')
/** What runs: `install`, or the repository being updated or removed. */
const busy = ref<string | null>(null)
const pendingDelete = ref<string | null>(null)

watchEffect(() => (layout.settingsBusy = busy.value !== null))
onBeforeUnmount(() => (layout.settingsBusy = false))

/** The name the server gives a URL: its last segment without `.git`. */
const derived = computed(
  () =>
    url.value
      .trim()
      .replace(/\/+$/, '')
      .split(/[/:]/)
      .pop()
      ?.replace(/\.git$/, '') ?? '',
)

async function install(): Promise<void> {
  if (!url.value.trim() || busy.value) return
  busy.value = 'install'
  const ok = await editSettings('settings.install_skills', {
    url: url.value.trim(),
    name: name.value.trim() || undefined,
  })
  busy.value = null
  if (ok) {
    url.value = ''
    name.value = ''
  }
}

async function update(repo: string): Promise<void> {
  busy.value = repo
  await editSettings('settings.update_skills', { name: repo })
  busy.value = null
}

async function remove(repo: string): Promise<void> {
  busy.value = repo
  await editSettings('settings.remove_skills', { name: repo })
  busy.value = null
  pendingDelete.value = null
}

function firstLine(text: string): string {
  return (
    text
      .split('\n')
      .find((l) => l.trim())
      ?.trim() ?? ''
  )
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto px-4 pt-5 pb-6 sm:px-6">
    <section class="flex flex-col gap-2">
      <span class="field-label">{{ t('settings.skillRepos') }}</span>
      <div
        v-for="repo in info.repos"
        :key="repo.name"
        class="flex items-center gap-3 rounded-card border border-line-soft bg-surface px-3 py-2"
      >
        <span class="font-mono font-semibold text-ink">{{ repo.name }}</span>
        <span class="min-w-0 flex-1 truncate font-mono text-micro text-ink-3" :title="repo.url">{{
          repo.url
        }}</span>
        <template v-if="pendingDelete === repo.name">
          <button type="button" class="btn" :disabled="!!busy" @click="pendingDelete = null">
            {{ t('common.cancel') }}
          </button>
          <button type="button" class="btn btn-danger" :disabled="!!busy" @click="remove(repo.name)">
            {{ t('settings.delete') }}
          </button>
        </template>
        <template v-else>
          <button
            type="button"
            class="icon-btn size-7 flex-none"
            :title="t('settings.updateSkills')"
            :aria-label="t('settings.updateSkills')"
            :disabled="!!busy"
            @click="update(repo.name)"
          >
            <Icon name="regenerate" :size="13" :class="{ 'animate-spin': busy === repo.name }" />
          </button>
          <button
            type="button"
            class="icon-btn size-7 flex-none"
            :title="t('settings.delete')"
            :aria-label="t('settings.delete')"
            :disabled="!!busy"
            @click="pendingDelete = repo.name"
          >
            <Icon name="trash" :size="13" />
          </button>
        </template>
      </div>
      <form class="flex flex-wrap items-center gap-2" @submit.prevent="install">
        <input
          v-model="url"
          type="text"
          class="min-w-0 flex-[3_1_16rem] font-mono text-meta"
          placeholder="https://github.com/owner/skills"
          autocomplete="off"
          spellcheck="false"
          :aria-label="t('settings.skillRepoUrl')"
        />
        <input
          v-model="name"
          type="text"
          class="min-w-0 flex-[1_1_7rem] font-mono text-meta"
          :placeholder="derived || t('settings.mcpName')"
          autocomplete="off"
          spellcheck="false"
          :aria-label="t('settings.mcpName')"
        />
        <button type="submit" class="btn" :disabled="!url.trim() || !!busy">
          <span
            v-if="busy === 'install'"
            class="size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent"
          />
          <Icon v-else name="download" :size="13" />
          {{ t('settings.installSkills') }}
        </button>
      </form>
    </section>

    <section class="flex flex-col gap-2">
      <span class="field-label">{{ t('settings.skillsFound', { n: info.skills.length }) }}</span>
      <ul v-if="info.skills.length" class="flex flex-col rounded-control border border-line-soft">
        <li
          v-for="s in info.skills"
          :key="s.location"
          class="flex flex-col gap-0.5 border-b border-line-soft px-3 py-2 last:border-b-0"
        >
          <div class="flex items-baseline gap-2">
            <span
              class="flex-none font-mono text-meta"
              :class="s.error ? 'text-danger' : s.shadowed ? 'text-ink-3 line-through' : 'text-ink'"
              >{{ s.name }}</span
            >
            <span class="min-w-0 truncate text-meta text-ink-3" :title="s.description">{{
              firstLine(s.description)
            }}</span>
          </div>
          <div class="flex items-baseline gap-2 font-mono text-micro text-ink-3">
            <span class="min-w-0 truncate" :title="s.location">{{ s.location }}</span>
            <span v-if="s.shadowed" class="flex-none">{{ t('settings.skillShadowed') }}</span>
            <span v-if="s.error" class="flex-none text-danger">{{ s.error }}</span>
          </div>
        </li>
      </ul>
    </section>
  </div>
</template>
