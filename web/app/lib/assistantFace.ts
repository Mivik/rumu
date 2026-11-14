// An assistant's look, drawn from its id: a tone, the mascot's own
// expression (how its ears sit, its lean, its glance, its eyes and mouth),
// laid over whatever state the cat is in, and the cat itself (its build and
// what it wears) until `deck.yaml cat` sets one. Stable for an id, so an
// assistant is recognised by its face rather than its uuid.

import type { CatPose } from './cat'
import { CAT_ITEMS, itemSlot } from './catItems'
import { PROJECT_TONES, hash, random, toneColor } from './projectIcon'
import type { DeckCat } from './types'

/** How far `wide` and `ears` go either way (`CAT_SHAPE_MAX` in rumu-cards). */
export const CAT_SHAPE_MAX = 2

/** A cat rolled from `seed`: at most one of the face items, maybe something
 * on top, now and then a bell, so it never gets crowded. */
export function randomCat(seed: string): DeckCat {
  const rnd = random(`cat:${seed}`)
  const pick = <T>(xs: readonly T[]): T => xs[Math.floor(rnd() * xs.length)]!
  const face = CAT_ITEMS.filter((i) => !itemSlot(i))
  const top = CAT_ITEMS.filter((i) => itemSlot(i) === 'top')
  const items = [
    ...(rnd() < 0.55 ? [pick(face)] : []),
    ...(rnd() < 0.6 ? [pick(top)] : []),
    ...(rnd() < 0.2 ? CAT_ITEMS.filter((i) => itemSlot(i) === 'neck') : []),
  ]
  const shape = (): number => Math.round(rnd() * 2 * CAT_SHAPE_MAX - CAT_SHAPE_MAX)
  return { items, tone: pick(PROJECT_TONES.slice(0, -1)), wide: shape(), ears: shape() }
}

/** The cat an assistant shows: its own, else one from its id in its tone. */
export function assistantCat(a: { id: string; cat?: DeckCat }): DeckCat {
  return a.cat ?? { ...randomCat(a.id), tone: PROJECT_TONES[hash(a.id) % (PROJECT_TONES.length - 1)] }
}

/** The cat of the assistant `id` among `assistants` (one from the id when
 * the roster has not loaded it). */
export function catOf(assistants: readonly { id: string; cat?: DeckCat }[], id: string): DeckCat {
  return assistantCat(assistants.find((a) => a.id === id) ?? { id })
}

/** The pose a cat's build adds. */
export function catBuild(cat: DeckCat): Partial<CatPose> {
  return { wide: cat.wide / CAT_SHAPE_MAX, ears: cat.ears / CAT_SHAPE_MAX }
}

/** The CSS color an assistant is tinted with: one of the tones (never the
 * neutral ink), its hue turned a little by the id so two of a tone still
 * tell apart. */
export function assistantTone(id: string): string {
  const h = hash(id)
  const tone = toneColor(PROJECT_TONES[h % (PROJECT_TONES.length - 1)]!)
  const turn = ((h >>> 8) % 61) - 30
  return `oklch(from ${tone} l c calc(h + ${turn}))`
}

/** The expression an assistant wears: kept within what still reads as calm. */
export function assistantFace(id: string): Partial<CatPose> {
  const rnd = random(id)
  const ears = rnd() * 1.1 - 0.3
  const lopsided = (rnd() - 0.5) * 1.1
  return {
    earL: ears + lopsided,
    earR: ears - lopsided,
    tilt: (rnd() - 0.5) * 16,
    look: (rnd() - 0.5) * 1.2,
    eyeY: (rnd() - 0.5) * 1.6,
    mouthW: 3.4 + rnd() * 1.2,
    mouthDepth: 1.8 + rnd() * 1.2,
  }
}

/** The face turned to whoever points at it: ears up, eyes on them. */
export function attentive(face: Partial<CatPose>): Partial<CatPose> {
  return { ...face, earL: 1, earR: 1, look: 0, tilt: (face.tilt ?? 0) / 3, eyeY: -1.2 }
}
