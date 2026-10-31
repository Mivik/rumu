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
