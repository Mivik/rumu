// Entrance motion for the live timeline.
//
// ConversationView flips `timelineLive` off while a session / branch renders all at
// once, then back on. Elements that mount while it's on — things that just
// happened — get a one-shot entrance class (main.css, "motion vocabulary");
// history stays still. The class is removed once its animations finish, so
// a later animation on the same element can't replay the entrance.

import { ref, type Directive } from 'vue'

export const timelineLive = ref(false)

/** Reduced motion collapses loops and transitions (main.css); JS-driven
 * smooth scrolls must collapse too, since they bypass `scroll-behavior`. */
export function prefersReduced(): boolean {
  return typeof matchMedia !== 'undefined' && matchMedia('(prefers-reduced-motion: reduce)').matches
}

/** Entrances whose animation just ended, settled together in the next frame:
 * a cold boot ends hundreds of entrances at once, and each `el.getAnimations()`
 * forces its own style pass (~12ms each on a long conversation, 10s for 850
 * entrances) — so ask the document once. */
const ended = new Map<HTMLElement, () => void>()
let settleFrame = 0

function settleEntrances(): void {
  settleFrame = 0
  const busy = new Set<Element>()
  for (const a of document.getAnimations()) {
    const target = (a.effect as KeyframeEffect | null)?.target
    if (target && (a.playState === 'running' || a.pending)) busy.add(target)
  }
  const finished = [...ended].filter(([el]) => !busy.has(el))
  ended.clear()
  for (const [, finish] of finished) finish()
}

/** `v-enter="false"` opts an element out (e.g. rows revealed together by a
 * disclosure, which animates as one instead). */
export const vEnter: Directive<HTMLElement, string | false | undefined> = {
  mounted(el, binding) {
    if (!timelineLive.value || binding.value === false) return
    const cls = binding.value || 'enter-rise'
    const finish = (): void => {
      el.classList.remove(cls)
      el.removeEventListener('animationend', done)
      el.removeEventListener('animationcancel', done)
    }
    const done = (e: AnimationEvent): void => {
      if (e.target !== el) return
      ended.set(el, finish)
      settleFrame ||= requestAnimationFrame(settleEntrances)
    }
    el.addEventListener('animationend', done)
    el.addEventListener('animationcancel', done)
    el.classList.add(cls)
  },
}

/** Keys whose reveal has already played in this page load. */
const played = new Set<string>()

/** `v-play-once="{ key, cls }"`: add `cls` (default `.line-in`) the first
 * time `key` is seen; a later mount shows it already drawn. For content that
 * remounts whenever its fold or popover is reopened — an opened catalog, a
 * revisited cache panel — where a fresh mount isn't news. */
export const vPlayOnce: Directive<HTMLElement, { key: string; cls?: string }> = {
  mounted(el, binding) {
    const { key, cls = 'line-in' } = binding.value
    if (played.has(key)) return
    played.add(key)
    el.classList.add(cls)
  },
}

/** Within this of its end, a scroll box counts as following its stream. */
const FOLLOW_SLOP = 24
const following = new WeakMap<HTMLElement, boolean>()

/** `v-follow="live"`: while `live`, keep a scroll box pinned to its end as
 * its content grows — unless it was scrolled up away from the end, which
 * holds it in place until scrolled back down. A box that opens mid-stream
 * starts at its end. */
export const vFollow: Directive<HTMLElement, boolean> = {
  mounted(el, binding) {
    if (binding.value) el.scrollTop = el.scrollHeight
  },
  beforeUpdate(el) {
    following.set(el, el.scrollHeight - el.scrollTop - el.clientHeight < FOLLOW_SLOP)
  },
  updated(el, binding) {
    if (binding.value && following.get(el)) el.scrollTop = el.scrollHeight
  },
}

// ---- swaps: one view taking another's place (Vue <Transition> classes) ----

/** A sheet or pane replacing another: it rises in while the one it replaces
 *  fades out. Either both share one grid cell (so they cross-fade in place),
 *  or the transition runs `mode="out-in"`. */
export const SWAP = {
  enterActiveClass: 'transition-[opacity,translate] duration-300 ease-soft',
  enterFromClass: 'translate-y-2 opacity-0',
  leaveActiveClass: 'pointer-events-none transition-opacity duration-150 ease-soft',
  leaveToClass: 'opacity-0',
}

/** The side sheet beside the chat (an artifact, a sub-agent). At xl its grid
 *  column eases open and shut (app.vue); while it does, the sheet holds its
 *  final width — half of what the sidebar (280px) and the two 16px gaps
 *  leave, less its own 16px gutter — so its contents don't reflow: the
 *  growing column slides it in from the right edge, where the desk clips it.
 *  Below xl it is an overlay and rises like a dialog. */
const SIDE_WIDTH = 'xl:min-w-[calc(50vw-172px)]'
export const SIDE_SHEET = {
  enterActiveClass: `transition-[opacity,translate] duration-300 ease-soft xl:duration-350 xl:ease-out ${SIDE_WIDTH}`,
  enterFromClass: 'translate-y-3 opacity-0 xl:translate-y-0',
  leaveActiveClass: `pointer-events-none transition-[opacity,translate] duration-200 ease-in xl:duration-350 ${SIDE_WIDTH}`,
  leaveToClass: 'translate-y-2 opacity-0 xl:translate-y-0',
}
