// Entrance motion for the live timeline.
//
// ChatView flips `timelineLive` off while a session / branch renders all at
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

/** `v-enter="false"` opts an element out (e.g. rows revealed together by a
 * disclosure, which animates as one instead). */
export const vEnter: Directive<HTMLElement, string | false | undefined> = {
  mounted(el, binding) {
    if (!timelineLive.value || binding.value === false) return
    const cls = binding.value || 'enter-rise'
    const done = (e: AnimationEvent): void => {
      if (e.target !== el) return
      const running = el.getAnimations().some((a) => a.playState === 'running' || a.pending)
      if (running) return
      el.classList.remove(cls)
      el.removeEventListener('animationend', done)
      el.removeEventListener('animationcancel', done)
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
