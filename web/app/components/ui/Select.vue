<script setup lang="ts" generic="T extends string">
// A styled single select: a field-shaped trigger and a listbox floated under
// it (teleported, so dialogs and scrollers never clip it). Keyboard follows
// the ARIA listbox pattern: arrows / Home / End move, Enter or Space picks,
// Escape or Tab closes. Options may carry a muted `hint` and an `icon`; an
// option marked `action` (e.g. "New deck…") is set apart below a rule.
import { computed, nextTick, onBeforeUnmount, ref, useId, watch } from 'vue'
import type { Ref } from 'vue'
import { autoUpdate, flip, offset, shift, size, useFloating } from '@floating-ui/vue'
import Icon from './Icon.vue'

export interface SelectOption<V extends string = string> {
  value: V
  label: string
  hint?: string
  /** The hint is an identifier (a model ref, a path): set in mono. */
  hintMono?: boolean
  icon?: string
  /** An action rather than a value: listed last, after a divider. */
  action?: boolean
}

const props = defineProps<{
  options: SelectOption<T>[]
  label: string
  placeholder?: string
  /** Teleport target; position absolutely inside it. A modal `<dialog>`
   *  makes everything outside inert, so pass the dialog itself. */
  to?: string
}>()
const model = defineModel<T>({ required: true })

const open = ref(false)
const active = ref(0)
const trigger = ref<HTMLElement | null>(null)
const panel = ref<HTMLElement | null>(null)
const id = useId()

/** Values first, actions after; the listbox order is this order. */
const ordered = computed(() => [
  ...props.options.filter((o) => !o.action),
  ...props.options.filter((o) => o.action),
])
const firstAction = computed(() => ordered.value.findIndex((o) => o.action))
const current = computed(() => props.options.find((o) => o.value === model.value))

const { floatingStyles, isPositioned } = useFloating(
  trigger as unknown as Readonly<Ref<HTMLElement | null>>,
  panel as unknown as Readonly<Ref<HTMLElement | null>>,
  {
    open,
    placement: 'bottom-start',
    // A contained listbox is absolute to its target; the default escapes
    // scrollers and sheets as a viewport-fixed layer.
    strategy: props.to ? 'absolute' : 'fixed',
    transform: false,
    whileElementsMounted: autoUpdate,
    middleware: [
      offset(6),
      flip({ padding: 8 }),
      shift({ padding: 8 }),
      size({
        padding: 8,
        apply({ rects, availableHeight, elements }) {
          Object.assign(elements.floating.style, {
            minWidth: `${rects.reference.width}px`,
            maxHeight: `${Math.min(availableHeight, 288)}px`,
          })
        },
      }),
    ],
  },
)

function show(): void {
  if (open.value) return
  active.value = Math.max(
    0,
    ordered.value.findIndex((o) => o.value === model.value),
  )
  open.value = true
}

function close(refocus = true): void {
  if (!open.value) return
  open.value = false
  if (refocus) trigger.value?.focus()
}

function pick(i: number): void {
  const o = ordered.value[i]
  if (o) model.value = o.value
  close()
}

function move(to: number): void {
  const n = ordered.value.length
  if (!n) return
  active.value = (to + n) % n
  void nextTick(() =>
    panel.value?.querySelector(`[data-index="${active.value}"]`)?.scrollIntoView({ block: 'nearest' }),
  )
}

function onKey(e: KeyboardEvent): void {
  if (!open.value) {
    if (['ArrowDown', 'ArrowUp', 'Enter', ' '].includes(e.key)) {
      e.preventDefault()
      show()
    }
    return
  }
  switch (e.key) {
    case 'ArrowDown':
      move(active.value + 1)
      break
    case 'ArrowUp':
      move(active.value - 1)
      break
    case 'Home':
      move(0)
      break
    case 'End':
      move(ordered.value.length - 1)
      break
    case 'Enter':
    case ' ':
      pick(active.value)
      break
    case 'Escape':
      // Close the list, not the dialog around it.
      e.stopPropagation()
      close()
      break
    case 'Tab':
      close(false)
      return
    default:
      return
  }
  e.preventDefault()
}

function onPointer(e: PointerEvent): void {
  const target = e.target as Node
  if (!trigger.value?.contains(target) && !panel.value?.contains(target)) close(false)
}

watch(open, (v) => {
  if (v) document.addEventListener('pointerdown', onPointer, true)
  else document.removeEventListener('pointerdown', onPointer, true)
})

onBeforeUnmount(() => document.removeEventListener('pointerdown', onPointer, true))
</script>

<template>
  <button
    ref="trigger"
    type="button"
    role="combobox"
    class="select-trigger"
    :class="{ 'is-open': open }"
    :aria-label="label"
    :aria-expanded="open"
    :aria-controls="id"
    aria-haspopup="listbox"
    :aria-activedescendant="open ? `${id}-${active}` : undefined"
    @click="open ? close() : show()"
    @keydown="onKey"
  >
    <Icon v-if="current?.icon" :name="current.icon" :size="14" class="flex-none text-ink-3" />
    <span class="min-w-0 flex-1 truncate text-left" :class="current ? 'text-ink' : 'text-ink-3'">
      {{ current?.label ?? placeholder }}
    </span>
    <Icon
      name="down"
      :size="14"
      class="flex-none text-ink-3 transition-transform duration-200"
      :class="open ? 'rotate-180' : ''"
    />
  </button>
  <Teleport :to="to || 'body'">
    <Transition
      enter-active-class="transition-[opacity,translate] duration-150 ease-soft"
      enter-from-class="-translate-y-1 opacity-0"
      leave-active-class="transition-opacity duration-100"
      leave-to-class="opacity-0"
    >
      <ul
        v-if="open"
        :id="id"
        ref="panel"
        role="listbox"
        :aria-label="label"
        :style="{ ...floatingStyles, visibility: isPositioned ? 'visible' : 'hidden' }"
        class="z-60 overflow-y-auto rounded-card border border-line-soft bg-surface p-1 shadow-pop"
      >
        <template v-for="(o, i) in ordered" :key="o.value">
          <li
            v-if="i === firstAction && i > 0"
            role="separator"
            class="mx-2 my-1 border-t border-line-soft"
          />
          <li
            :id="`${id}-${i}`"
            role="option"
            :data-index="i"
            :aria-selected="o.value === model"
            class="flex cursor-pointer items-center gap-2 rounded-control px-2.5 py-1.5 text-secondary transition-colors duration-100"
            :class="[
              i === active ? 'bg-surface-2 text-ink' : 'text-ink-2',
              o.value === model ? 'font-semibold text-ink' : '',
            ]"
            @pointerenter="active = i"
            @click="pick(i)"
          >
            <Icon v-if="o.icon" :name="o.icon" :size="14" class="flex-none text-ink-3" />
            <span class="min-w-0 flex-1 truncate">{{ o.label }}</span>
            <span
              v-if="o.hint"
              class="flex-none text-micro text-ink-3"
              :class="{ 'font-mono': o.hintMono }"
              >{{ o.hint }}</span
            >
            <Icon
              name="ok"
              :size="14"
              class="flex-none text-brand"
              :class="o.value === model ? '' : 'invisible'"
            />
          </li>
        </template>
      </ul>
    </Transition>
  </Teleport>
</template>
