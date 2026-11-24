<script setup lang="ts">
// The MCP servers and skills an assistant or a project enables: a chip per
// configured server (a bound one opens a checklist of its tools) and per
// usable skill. The rules are the server's (`rulesEnable`): unchecking under
// `*` adds `!name`, checking from none adds the name.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { openPage } from '../../lib/router'
import { models, rulesEnable } from '../../lib/state/models'
import Icon from '../ui/Icon.vue'

/** Each bound server with the rules enabling its tools. */
const mcp = defineModel<Record<string, string[]>>('mcp', { required: true })
/** The rules enabling skills. */
const skills = defineModel<string[]>('skills', { required: true })

/** The configured MCP servers, then names kept that are no longer
 *  configured. */
const mcpOptions = computed(() => {
  const configured = (models.settings?.mcp ?? []).map((s) => s.name)
  return [
    ...configured.map((name) => ({ name, gone: false })),
    ...Object.keys(mcp.value)
      .filter((n) => !configured.includes(n))
      .map((name) => ({ name, gone: true })),
  ]
})

/** The bound servers that list tools, each with the ones its rules enable. */
const mcpTools = computed(() =>
  (models.settings?.mcp ?? [])
    .filter((s) => mcp.value[s.name] && s.tools.length)
    .map((s) => ({
      name: s.name,
      tools: s.tools,
      on: s.tools.filter((tool) => rulesEnable(mcp.value[s.name]!, tool.name)).length,
    })),
)
/** The servers whose tool list is open. */
const openTools = ref(new Set<string>())

/** Bind a server with all its tools, or unbind it. Kept sorted, as the
 *  server stores them, so binding back is no change. */
function toggleServer(name: string): void {
  const next = { ...mcp.value }
  if (next[name]) delete next[name]
  else next[name] = ['*']
  mcp.value = Object.fromEntries(Object.entries(next).sort(([a], [b]) => (a < b ? -1 : 1)))
}

function setTools(server: string, rules: string[]): void {
  mcp.value = { ...mcp.value, [server]: rules }
}

/** `rules` with `id` flipped by a rule naming it, when the other rules
 *  would not already put it there. */
function flip(rules: string[], id: string): string[] {
  const rest = rules.filter((r) => r !== id && r !== `!${id}`)
  const want = !rulesEnable(rules, id)
  if (rulesEnable(rest, id) !== want) rest.push(want ? id : `!${id}`)
  return rest
}

/** The usable skills, then names the rules keep that none is. */
const skillOptions = computed(() => {
  const found = [
    ...new Set(
      (models.settings?.skills.skills ?? []).filter((s) => !s.error && !s.shadowed).map((s) => s.name),
    ),
  ]
  const gone = skills.value.filter((r) => !r.startsWith('!') && !r.includes('*') && !found.includes(r))
  return [...found.map((name) => ({ name, gone: false })), ...gone.map((name) => ({ name, gone: true }))]
})
const allSkills = computed(() => skills.value.includes('*'))

function toggleSkill(name: string, gone: boolean): void {
  skills.value = gone ? skills.value.filter((r) => r !== name) : flip(skills.value, name)
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
  <div class="flex flex-col gap-1.5">
    <span class="field-label">{{ t('assistants.mcp') }}</span>
    <div class="flex flex-wrap gap-1.5">
      <button
        v-for="server in mcpOptions"
        :key="server.name"
        type="button"
        class="chip font-mono"
        :class="server.gone ? 'tone-danger line-through' : mcp[server.name] ? 'tone-tool' : 'tone-neutral'"
        :aria-pressed="!!mcp[server.name]"
        :title="server.gone ? t('assistants.mcpGone') : undefined"
        @click="toggleServer(server.name)"
      >
        <Icon :name="server.gone ? 'close' : mcp[server.name] ? 'ok' : 'plus'" :size="12" />
        {{ server.name }}
      </button>
      <button type="button" class="chip tone-neutral" @click="openPage('settings', 'mcp')">
        <Icon name="settings" :size="12" />
        {{ t('assistants.manageMcp') }}
      </button>
    </div>
    <div v-for="server in mcpTools" :key="server.name" class="rounded-control border border-line-soft">
      <button
        type="button"
        class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-meta"
        :aria-expanded="openTools.has(server.name)"
        @click="openTools.has(server.name) ? openTools.delete(server.name) : openTools.add(server.name)"
      >
        <Icon
          name="chevron"
          :size="12"
          class="flex-none text-ink-3 transition-transform duration-150"
          :class="{ 'rotate-90': openTools.has(server.name) }"
        />
        <span class="font-mono text-ink">{{ server.name }}</span>
        <span class="text-ink-3">{{
          t('assistants.toolsOn', { n: server.on, total: server.tools.length })
        }}</span>
      </button>
      <div v-if="openTools.has(server.name)" class="border-t border-line-soft">
        <div class="flex gap-1.5 px-3 py-1.5">
          <button type="button" class="chip tone-neutral" @click="setTools(server.name, ['*'])">
            {{ t('assistants.allTools') }}
          </button>
          <button type="button" class="chip tone-neutral" @click="setTools(server.name, [])">
            {{ t('assistants.noTools') }}
          </button>
        </div>
        <ul class="flex max-h-80 flex-col overflow-y-auto">
          <li v-for="tool in server.tools" :key="tool.name" class="border-t border-line-soft">
            <label class="flex cursor-pointer items-baseline gap-2 px-3 py-1.5">
              <input
                type="checkbox"
                class="flex-none self-center"
                :checked="rulesEnable(mcp[server.name]!, tool.name)"
                @change="setTools(server.name, flip(mcp[server.name]!, tool.name))"
              />
              <span class="flex-none font-mono text-meta text-ink">{{ tool.name }}</span>
              <span class="min-w-0 truncate text-meta text-ink-3" :title="tool.description">{{
                firstLine(tool.description)
              }}</span>
            </label>
          </li>
        </ul>
      </div>
    </div>
  </div>

  <div class="flex flex-col gap-1.5">
    <span class="field-label">{{ t('assistants.skills') }}</span>
    <div class="flex flex-wrap gap-1.5">
      <button
        type="button"
        class="chip"
        :class="allSkills ? 'tone-tool' : 'tone-neutral'"
        :aria-pressed="allSkills"
        @click="skills = allSkills ? [] : ['*']"
      >
        <Icon :name="allSkills ? 'ok' : 'plus'" :size="12" />
        {{ t('assistants.allTools') }}
      </button>
      <button
        v-for="skill in skillOptions"
        :key="skill.name"
        type="button"
        class="chip font-mono"
        :class="
          skill.gone
            ? 'tone-danger line-through'
            : rulesEnable(skills, skill.name)
              ? 'tone-tool'
              : 'tone-neutral'
        "
        :aria-pressed="!skill.gone && rulesEnable(skills, skill.name)"
        :title="skill.gone ? t('assistants.skillGone') : undefined"
        @click="toggleSkill(skill.name, skill.gone)"
      >
        <Icon :name="skill.gone ? 'close' : rulesEnable(skills, skill.name) ? 'ok' : 'plus'" :size="12" />
        {{ skill.name }}
      </button>
      <button type="button" class="chip tone-neutral" @click="openPage('settings', 'skills')">
        <Icon name="settings" :size="12" />
        {{ t('assistants.manageMcp') }}
      </button>
    </div>
  </div>
</template>
