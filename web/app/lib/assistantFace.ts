// An assistant's look, drawn from its id: a tone, and the mascot's own
// expression (how its ears sit, its lean, its glance, its eyes and mouth),
// laid over whatever state the cat is in. Stable for an id, so an assistant
// is recognised by its face rather than its uuid.

import type { CatPose } from './cat'
import { PROJECT_TONES, hash, random, toneColor } from './projectIcon'

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
