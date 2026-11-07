<script setup lang="ts">
// Browse the local disk for a workspace root (`fs.dirs`). Opening a folder
// picks it: the root field follows the browsing.
import { nextTick, onMounted, ref } from 'vue'
import { t } from '../i18n'
import { rumu } from '../lib/api'
import Icon from './ui/Icon.vue'

interface Listing {
  path: string
  parent: string | null
  dirs: string[]
}

const props = defineProps<{ start: string }>()
const emit = defineEmits<{ pick: [path: string] }>()

const listing = ref<Listing | null>(null)
const error = ref('')
const crumbsEl = ref<HTMLElement | null>(null)

/** Each ancestor of the open folder, root first. */
function crumbs(path: string): { name: string; path: string }[] {
  const parts = path.split('/').filter(Boolean)
  return [
    { name: '/', path: '/' },
    ...parts.map((name, i) => ({ name, path: '/' + parts.slice(0, i + 1).join('/') })),
  ]
}

async function load(path: string): Promise<boolean> {
  try {
    listing.value = await rumu.request<Listing>('fs.dirs', { path })
    error.value = ''
    await nextTick()
    crumbsEl.value?.scrollTo({ left: crumbsEl.value.scrollWidth })
    return true
  } catch (err) {
    error.value = (err as Error).message
    return false
  }
}

async function open(path: string): Promise<void> {
  if (await load(path)) emit('pick', listing.value!.path)
}

function child(name: string): string {
  const base = listing.value!.path
  return base.endsWith('/') ? base + name : `${base}/${name}`
}

onMounted(async () => {
  if (!(props.start.trim() && (await load(props.start.trim())))) await load('~')
})
</script>

<template>
  <div class="flex flex-col overflow-hidden rounded-control border border-line bg-surface">
    <div v-if="listing" class="flex items-center gap-1 border-b border-line-soft px-1.5 py-1">
      <button
        type="button"
        class="icon-btn size-6 flex-none"
        :disabled="!listing.parent"
        :title="t('workspaces.parentFolder')"
        :aria-label="t('workspaces.parentFolder')"
        @click="listing.parent && open(listing.parent)"
      >
        <Icon name="up" :size="13" />
      </button>
      <div
        ref="crumbsEl"
        class="flex min-w-0 flex-1 items-center overflow-x-auto font-card text-meta whitespace-nowrap"
      >
        <template v-for="(c, i) in crumbs(listing.path)" :key="c.path">
          <span v-if="i > 1" class="px-0.5 text-ink-3/60">/</span>
          <button
            type="button"
            class="rounded-xs px-1 py-0.5 transition-colors duration-150 hover:bg-surface-2"
            :class="c.path === listing.path ? 'font-semibold text-ink' : 'text-ink-3 hover:text-ink'"
            @click="open(c.path)"
          >
            {{ c.name }}
          </button>
        </template>
      </div>
    </div>
    <p v-if="error" class="px-3 py-2 text-meta text-danger">{{ error }}</p>
    <div v-if="listing?.dirs.length" class="flex max-h-52 flex-col overflow-y-auto p-1">
      <button
        v-for="name in listing.dirs"
        :key="name"
        type="button"
        class="flex items-center gap-2 rounded-xs px-2 py-1 text-left font-card text-secondary text-ink-2 transition-colors duration-150 hover:bg-surface-2 hover:text-ink"
        @click="open(child(name))"
      >
        <Icon name="mount" :size="13" class="flex-none text-ink-3" />
        <span class="truncate">{{ name }}</span>
      </button>
    </div>
  </div>
</template>
