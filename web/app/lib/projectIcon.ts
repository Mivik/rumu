// A project's seal: the glyph and tone it shows, from its `deck.yaml icon`
// or, for a part left unset, from its name and id.

import type { Project } from './types'

/** The tones a seal can take (`ICON_TONES` in rumu-cards). */
export const PROJECT_TONES = ['brand', 'tool', 'pine', 'memory', 'agent', 'ink'] as const
export type ProjectTone = (typeof PROJECT_TONES)[number]

type Sealed = Pick<Project, 'id' | 'name' | 'icon'>

/** FNV-1a over `s`: stable, and ids a few hex digits apart still spread. */
export function hash(s: string): number {
  let h = 0x811c9dc5
  for (const c of s) h = Math.imul(h ^ c.charCodeAt(0), 0x01000193)
  return h >>> 0
}

/** A small seeded PRNG (mulberry32) over the hash of `seed`. */
export function random(seed: string): () => number {
  let h = hash(seed)
  return () => {
    h = (h + 0x6d2b79f5) | 0
    let x = Math.imul(h ^ (h >>> 15), h | 1)
    x ^= x + Math.imul(x ^ (x >>> 7), x | 61)
    return ((x ^ (x >>> 14)) >>> 0) / 4294967296
  }
}

export function projectTone(p: Sealed): ProjectTone {
  const set = p.icon?.tone
  if (set && (PROJECT_TONES as readonly string[]).includes(set)) return set as ProjectTone
  // the neutral tone is a choice, never a default
  return PROJECT_TONES[hash(p.id) % (PROJECT_TONES.length - 1)]!
}

export function projectGlyph(p: Sealed): string {
  return p.icon?.glyph || (Array.from(p.name)[0]?.toUpperCase() ?? '')
}

/** The CSS color of a tone. */
export function toneColor(tone: ProjectTone): string {
  return tone === 'ink' ? 'var(--ink-2)' : `var(--${tone})`
}
