// The timeline's virtual list. Every entry keeps a wrapper in the column, but
// only the entries near the view (and the ones that must stay: the live turn,
// those you touched) render their content; the rest are blanks of the height
// they last measured.
//
// No jitter is the constraint everything here serves:
// - A wrapper's height is its content's, rounded up to a whole px. Scroll
//   offsets are whole px, so a change above the view — a blank filling in, a
//   backfill measuring an entry — moves the content by a whole px, and the
//   scroller's anchoring (or the end pin) takes it back exactly. Fractional
//   heights left a sub-px remainder that made rows on a rounding edge jump.
// - A blank is never a guess. A session opens on its tail; the rest is then
//   measured in idle time, newest first (rendered, measured and dropped
//   before the frame paints), so a scroll up only ever meets blanks of the
//   exact height their content will have.
// - Content mounted for these reasons is history: it opens quiet (no
//   entrance motion, see `vEnter`). Only the tail a session opens on and
//   entries that appear afterwards play their entrance.

import { nextTick, ref, shallowRef, type Ref } from 'vue'

/** Bumped after the rendered entries change: what looks things up in the
 *  timeline's DOM (the side questions' underlines) looks again. */
export const timelineRendered = ref(0)

const revealers = new Set<(node: string) => boolean>()

/** Render the entry holding `node` (and keep it rendered), for code about to
 *  look the node up in the DOM. */
export async function revealNode(node: string): Promise<void> {
  let any = false
  for (const r of revealers) any = r(node) || any
  if (any) await nextTick()
}

/** Register a timeline's revealer; returns its removal. */
export function onReveal(fn: (node: string) => boolean): () => void {
  revealers.add(fn)
  return () => revealers.delete(fn)
}

export interface VirtualEntry {
  key: string
  /** May render as a blank (an assistant turn); the rest always render. */
  virtual: boolean
  /** Roughly how much it renders (nodes), to size boot and backfill batches. */
  weight: number
}

/** Weight the tail a session opens on renders at once. */
const BOOT_WEIGHT = 300
/** Weight one backfill step renders. */
const BATCH_WEIGHT = 120
/** Entries within this share of the view's height above and below it render. */
const OVERSCAN = '150%'
/** Up to this weight a conversation renders whole. */
const WHOLE_WEIGHT = 800

export function useVirtualEntries(opts: {
  scrollEl: Ref<HTMLElement | null>
  columnEl: Ref<HTMLElement | null>
  entries: () => VirtualEntry[]
  /** Keys that render whatever their place (the live turn). */
  forced: () => Iterable<string>
  /** The backfill measured everything there is: the place to load more. */
  measured?: () => void
  /** The key a view opens at instead of its tail (where a reload left
   *  off), when it is in the list. Such an open is history: it plays no
   *  entrance. */
  bootAt?: () => string | null
  /** What arrived below the entries known is history too (a later stretch
   *  of the line loaded), not something that just happened. */
  quietArrivals?: () => boolean
}) {
  /** Measured heights, whole px. */
  const heights = new Map<string, number>()
  /** Measured at another column width: right enough to hold a place, due a
   *  new measurement. */
  const stale = new Set<string>()
  const wraps = new Map<string, HTMLElement>()
  const bodies = new Map<Element, string>()
  const near = new Set<string>()
  const pinned = new Set<string>()
  /** Rendering until the observer first places them: the tail a view
   *  opens on, and entries that appear after it. */
  const held = new Set<string>()
  /** Keys the observer has placed at least once. */
  const placed = new Set<string>()
  /** Rendering for the backfill to measure. */
  const batch = new Set<string>()
  /** Keys whose wrapper plays its entrance when it mounts. */
  const entering = new Set<string>()
  let booted = false
  let width = 0
  let known = new Set<string>()

  /** Keys whose content renders. */
  const shown = shallowRef(new Set<string>())
  /** Keys whose content is rendering as history (no entrance motion). */
  const quiet = shallowRef(new Set<string>())

  const ro =
    typeof ResizeObserver === 'undefined'
      ? null
      : new ResizeObserver((records) => {
          let released = false
          for (const r of records) {
            const key = bodies.get(r.target)
            if (key === undefined) continue
            const size = r.borderBoxSize?.[0]?.blockSize ?? r.contentRect.height
            // a hair over a whole px from float noise still rounds down
            const h = Math.ceil(size - 0.01)
            if (heights.get(key) !== h) {
              heights.set(key, h)
              const w = wraps.get(key)
              if (w) w.style.height = `${h}px`
            }
            stale.delete(key)
            if (batch.delete(key)) released = true
          }
          if (released && !batch.size) {
            update()
            scheduleBackfill()
          }
        })

  let io: IntersectionObserver | null = null
  function observer(): IntersectionObserver | null {
    const root = opts.scrollEl.value
    if (!root || typeof IntersectionObserver === 'undefined') return null
    if (io && io.root === root) return io
    io?.disconnect()
    io = new IntersectionObserver(
      (records) => {
        for (const r of records) {
          const key = (r.target as HTMLElement).dataset.vkey
          if (key === undefined) continue
          held.delete(key)
          placed.add(key)
          if (r.isIntersecting) near.add(key)
          else near.delete(key)
        }
        update()
      },
      { root, rootMargin: `${OVERSCAN} 0px ${OVERSCAN} 0px` },
    )
    for (const w of wraps.values()) io.observe(w)
    return io
  }

  /** Recompute what renders; a change re-renders the column once. */
  function update(): void {
    const list = opts.entries()
    // a short conversation renders whole: blanks would only cost mounts
    // on scroll there
    const whole = list.reduce((s, e) => s + e.weight, 0) <= WHOLE_WEIGHT
    const next = new Set<string>()
    for (const e of list) {
      if (whole || !e.virtual || near.has(e.key) || held.has(e.key) || batch.has(e.key) || pinned.has(e.key))
        next.add(e.key)
    }
    for (const k of opts.forced()) next.add(k)
    const prev = shown.value
    if (next.size === prev.size && [...next].every((k) => prev.has(k))) return
    // what the list has not reported yet is new: it is not history
    const hush = [...next].filter((k) => !prev.has(k) && !entering.has(k) && known.has(k))
    const entered = [...next].filter((k) => !prev.has(k) && entering.has(k))
    if (hush.length) quiet.value = new Set([...quiet.value, ...hush])
    if (hush.length || entered.length) {
      // the directives of what mounts read these in this flush; a later
      // mount of the same entry is history
      requestAnimationFrame(() => {
        for (const k of entered) entering.delete(k)
        if (!hush.length) return
        const q = new Set(quiet.value)
        for (const k of hush) q.delete(k)
        quiet.value = q
      })
    }
    shown.value = next
    void nextTick(() => timelineRendered.value++)
  }

  /** The list changed: a first fill opens on the tail; later arrivals are
   *  new (they render, and enter). */
  function sync(): void {
    const list = opts.entries()
    if (!list.length) return
    // a list with nothing of the last one is another conversation (the
    // tree of a session just switched to landing): it opens like one
    if (booted && known.size && !list.some((e) => known.has(e.key))) {
      booted = false
      filled = false
      held.clear()
      entering.clear()
      batch.clear()
    }
    if (!booted) {
      booted = true
      const whole = list.reduce((s, e) => s + e.weight, 0) <= WHOLE_WEIGHT
      let weight = 0
      // the tail, or from where the view opens on down (and up from there,
      // if that runs out first)
      const at = opts.bootAt?.()
      const from = at == null ? -1 : list.findIndex((e) => e.key === at)
      const hold = (e: VirtualEntry): boolean => {
        if (e.virtual && weight >= BOOT_WEIGHT && !whole) return false
        held.add(e.key)
        if (from < 0) entering.add(e.key)
        weight += e.weight
        return true
      }
      if (from >= 0) {
        for (let i = from; i < list.length && hold(list[i]!); i++);
        for (let i = from - 1; i >= 0 && hold(list[i]!); i--);
      } else {
        for (let i = list.length - 1; i >= 0 && hold(list[i]!); i--);
      }
      if (from < 0) for (const e of list) if (!e.virtual) entering.add(e.key)
    } else {
      // what arrived above the entries known is history loaded late (the
      // main line's earlier part): measured at once, quiet — left to the
      // observer, the ones near the top would render on its report instead
      const top = list.findIndex((e) => known.has(e.key))
      const later = opts.quietArrivals?.() ?? false
      list.forEach((e, i) => {
        if (known.has(e.key)) return
        if (i < top || later) {
          if (e.virtual) batch.add(e.key)
          return
        }
        held.add(e.key)
        entering.add(e.key)
      })
    }
    known = new Set(list.map((e) => e.key))
    update()
    if (batch.size) release(batch)
    if (!filled) void nextTick(fill)
    scheduleBackfill()
  }

  /** Before the first paint, the tail must fill the view and the overscan
   *  above it: blanks that fill in later above a short tail would push it
   *  down in view. Grows the tail until it does (all in this task). */
  let filled = false
  async function fill(): Promise<void> {
    const sc = opts.scrollEl.value
    const col = opts.columnEl.value
    if (!sc || !col || filled) return
    filled = true
    for (let round = 0; round < 50; round++) {
      if (col.offsetHeight >= sc.clientHeight * 2.5) return
      const list = opts.entries()
      let weight = 0
      for (let i = list.length - 1; i >= 0 && weight < BATCH_WEIGHT; i--) {
        const e = list[i]!
        if (!e.virtual || shown.value.has(e.key)) continue
        // already placed (and so not near): the observer won't report it
        // again to let it go, the measurement does
        if (placed.has(e.key)) batch.add(e.key)
        else held.add(e.key)
        entering.add(e.key)
        weight += e.weight
      }
      if (!weight) return
      update()
      // the render lands in this microtask chain, before the frame paints
      await nextTick()
    }
  }

  // ---- backfill ----

  let idle = 0
  const ric: (cb: () => void) => number =
    typeof requestIdleCallback === 'function'
      ? (cb) => requestIdleCallback(cb, { timeout: 500 })
      : (cb) => window.setTimeout(cb, 50)
  const cancelRic = (id: number): void =>
    typeof cancelIdleCallback === 'function' ? cancelIdleCallback(id) : clearTimeout(id)

  function scheduleBackfill(): void {
    if (idle || batch.size) return
    idle = ric(() => {
      idle = 0
      void document.fonts?.ready.then(backfill)
    })
  }

  /** Render the next unmeasured (or stale) entries, newest first; the
   *  resize observer measures them and lets them go. */
  function backfill(): void {
    if (batch.size || !booted) return
    const list = opts.entries()
    let weight = 0
    for (let i = list.length - 1; i >= 0 && weight < BATCH_WEIGHT; i--) {
      const e = list[i]!
      if (!e.virtual || shown.value.has(e.key)) continue
      if (heights.has(e.key) && !stale.has(e.key)) continue
      batch.add(e.key)
      weight += e.weight
    }
    if (!batch.size) {
      opts.measured?.()
      return
    }
    update()
    release(batch)
  }

  /** A body the observer never reports (no size change to report) still
   *  lets its batch go. */
  function release(keys: Set<string>): void {
    const pending = new Set(keys)
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        let released = false
        for (const k of pending) if (batch.delete(k)) released = true
        if (released && !batch.size) {
          update()
          scheduleBackfill()
        }
      }),
    )
  }

  // ---- element bindings ----

  const wrapRefs = new Map<string, (el: unknown) => void>()
  const bodyRefs = new Map<string, (el: unknown) => void>()

  /** Stable function ref for an entry's wrapper (a new function each render
   *  would unbind and rebind it every time). */
  function wrapRef(key: string): (el: unknown) => void {
    let fn = wrapRefs.get(key)
    if (!fn) {
      fn = (el) => {
        const old = wraps.get(key)
        if (old && old !== el) {
          io?.unobserve(old)
          wraps.delete(key)
        }
        if (!(el instanceof HTMLElement) || old === el) return
        wraps.set(key, el)
        el.dataset.vkey = key
        const h = heights.get(key)
        if (h !== undefined) el.style.height = `${h}px`
        observer()?.observe(el)
      }
      wrapRefs.set(key, fn)
    }
    return fn
  }

  function bodyRef(key: string): (el: unknown) => void {
    let fn = bodyRefs.get(key)
    if (!fn) {
      let cur: Element | null = null
      fn = (el) => {
        if (cur && cur !== el) {
          ro?.unobserve(cur)
          bodies.delete(cur)
          cur = null
        }
        if (!(el instanceof Element) || el === cur) return
        cur = el
        bodies.set(el, key)
        ro?.observe(el)
      }
      bodyRefs.set(key, fn)
    }
    return fn
  }

  /** You touched it: it keeps rendering, so what you opened stays open. */
  function pin(key: string): void {
    if (pinned.has(key)) return
    pinned.add(key)
    update()
  }

  /** A new column width: every blank is due a new measurement. */
  function resized(w: number): void {
    if (!w || w === width) return
    const first = !width
    width = w
    if (first) return
    for (const k of heights.keys()) if (!shown.value.has(k)) stale.add(k)
    scheduleBackfill()
  }

  /** Whether an entry's wrapper plays its entrance as it mounts. */
  function enters(key: string): boolean {
    return entering.has(key)
  }

  /** A different view: start over. */
  function reset(): void {
    heights.clear()
    stale.clear()
    near.clear()
    pinned.clear()
    held.clear()
    placed.clear()
    batch.clear()
    entering.clear()
    known = new Set()
    booted = false
    filled = false
    width = 0
    if (idle) cancelRic(idle)
    idle = 0
    shown.value = new Set()
    quiet.value = new Set()
  }

  function dispose(): void {
    reset()
    io?.disconnect()
    io = null
    ro?.disconnect()
  }

  return {
    shown,
    quiet,
    sync,
    update,
    fill,
    wrapRef,
    bodyRef,
    pin,
    resized,
    enters,
    reset,
    dispose,
    observer,
  }
}
