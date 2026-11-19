<script setup lang="ts">
// One model of a provider: what the catalog knows of it, and the protocol
// it is held to.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { rumu } from '../../lib/api'
import { PROTOCOL_LABELS, protocolFor } from '../../lib/state/models'
import type { ModelInfo } from '../../lib/generated/ModelInfo'
import type { Protocol } from '../../lib/generated/Protocol'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'

const props = defineProps<{
  provider: string
  id: string
  /** Known already (an enabled model); else asked for when opened. */
  info?: ModelInfo
  /** The protocol it is held to; `null` is its own. */
  held: Protocol | null
  /** The protocols the provider has endpoints for. */
  usable: Protocol[]
}>()
const emit = defineEmits<{ protocol: [protocol: Protocol | null] }>()

const fetched = ref<ModelInfo | null>(null)
const m = computed(() => props.info ?? fetched.value)

async function load(): Promise<void> {
  if (props.info || fetched.value) return
  try {
    fetched.value = await rumu.request<ModelInfo>('settings.model', {
      model: `${props.provider}/${props.id}`,
    })
  } catch {
    // Nothing known: the protocol can still be set.
  }
}

/** What it runs on when not held to one. */
const auto = computed(() => protocolFor(null, m.value?.native ?? null, props.usable))

function size(n: number): string {
  if (n >= 1_000_000) return `${+(n / 1_000_000).toFixed(1)}M`
  return `${Math.round(n / 1000)}K`
}
/** Per million tokens. */
function usd(n: number): string {
  return `$${+n.toFixed(2)} / M`
}

const facts = computed(() => {
  const x = m.value
  if (!x?.known) return []
  const out: { label: string; value: string }[] = []
  if (x.context) out.push({ label: t('settings.context'), value: size(x.context) })
  if (x.cost) {
    out.push({ label: t('settings.inputCost'), value: usd(x.cost.input) })
    out.push({ label: t('settings.outputCost'), value: usd(x.cost.output) })
  }
  if (x.efforts.length) out.push({ label: t('settings.effort'), value: x.efforts.join(' ') })
  out.push({ label: t('settings.imageInput'), value: t(x.image_input ? 'settings.yes' : 'settings.no') })
  return out
})
</script>

<template>
  <Popover :label="id" placement="left-start">
    <template #trigger="{ open, toggle }">
      <button
        type="button"
        class="icon-btn size-6"
        :class="{ 'bg-surface-2 text-ink': open }"
        :title="t('settings.modelSettings')"
        :aria-label="t('settings.modelSettings')"
        :aria-expanded="open"
        @click="(toggle(), load())"
      >
        <Icon name="settings" :size="13" />
      </button>
    </template>
    <div class="flex flex-col gap-3">
      <div class="flex min-w-0 flex-col gap-0.5">
        <span class="truncate font-semibold text-ink">{{ m?.name ?? id }}</span>
        <span class="truncate font-mono text-micro text-ink-3" :title="`${provider}/${id}`"
          >{{ provider }}/{{ id }}</span
        >
      </div>

      <dl v-if="facts.length" class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-meta">
        <template v-for="f in facts" :key="f.label">
          <dt class="text-ink-3">{{ f.label }}</dt>
          <dd class="text-ink-2 tabular-nums">{{ f.value }}</dd>
        </template>
      </dl>
      <p v-if="m?.known_as" class="text-meta text-ink-3 italic">
        <i18n-t keypath="settings.knownAs" scope="global" tag="span">
          <template #id>
            <span class="font-mono not-italic">{{ m.known_as }}</span>
          </template>
        </i18n-t>
      </p>
      <p v-else-if="m && !facts.length" class="text-meta text-ink-3 italic">
        {{ t('settings.notInCatalog') }}
      </p>

      <div v-if="usable.length" class="flex flex-col gap-1 border-t border-line-soft pt-2">
        <span class="field-label">{{ t('settings.protocol') }}</span>
        <div class="-mx-1.5 flex flex-col" role="radiogroup" :aria-label="t('settings.protocol')">
          <button
            v-for="p in [null, ...usable]"
            :key="p ?? ''"
            type="button"
            role="radio"
            :aria-checked="held === p"
            class="flex items-center gap-2 rounded-control px-1.5 py-1 text-left text-secondary transition-colors hover:bg-surface-2"
            :class="held === p ? 'text-ink' : 'text-ink-2'"
            @click="emit('protocol', p)"
          >
            <Icon name="ok" :size="13" class="flex-none text-brand" :class="held === p ? '' : 'invisible'" />
            <template v-if="p">{{ PROTOCOL_LABELS[p] }}</template>
            <template v-else>
              {{ t('settings.protocolAuto') }}
              <span v-if="auto" class="text-meta text-ink-3">{{ PROTOCOL_LABELS[auto] }}</span>
            </template>
          </button>
        </div>
      </div>
    </div>
  </Popover>
</template>
