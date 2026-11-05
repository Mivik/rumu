// Parametric mascot drawing. Each CatState is a preset CatPose; every path
// keeps the same commands for any pose, so two poses blend by plain lerp and
// the drawing morphs.

export type CatState = 'idle' | 'waiting' | 'thinking' | 'working' | 'done' | 'blocked' | 'offline'

export interface CatPose {
  /** Ears, each side: -1 flattened and turned out, 0 at rest, 1 perked. */
  earL: number
  earR: number
  /** -1…1: both ears bend that way, tips swinging about their feet. Runs on
   * its own faster beat, independent of the glance. */
  earSway: number
  /** -1…1: the face slides that way and the head turns a little after it. */
  look: number
  /** Head lean in degrees, + to the right. The paws stay planted. */
  tilt: number
  /** 1 a round dot, 0 a short line. */
  open: number
  /** Bends the line: 1 a happy hump (the m again), -1 a sleepy ‿. */
  arc: number
  /** 0…1: folds the line into a squeezed > <. */
  squint: number
  /** Nudges the eyes up (-) or down (+). */
  eyeY: number
  /** The ω mouth: one u's width, how deep its bowl drops, how much higher its outer tips start. */
  mouthW: number
  mouthDepth: number
  mouthLift: number
  /** 0 coral, 1 grey. */
  mouthGrey: number
  /** 0…1: the hook's rubber stamp. */
  stamp: number
}

const REST: CatPose = {
  earL: 0,
  earR: 0,
  earSway: 0,
  look: 0,
  tilt: 0,
  open: 1,
  arc: 0,
  squint: 0,
  eyeY: 0,
  mouthW: 4,
  mouthDepth: 2.4,
  mouthLift: 0.8,
  mouthGrey: 0,
  stamp: 0,
}

export const POSES: Record<CatState, CatPose> = {
  idle: REST,
  // - ω -; poseAt() rocks it gently side to side
  waiting: { ...REST, earL: 0.5, earR: 0.5, open: 0, eyeY: -0.4 },
  // wide-eyed and looking up; poseAt() has it gaze off to one side, then the other
  thinking: {
    ...REST,
    earL: 1,
    earR: 1,
    eyeY: -2.4,
    mouthW: 3.4,
    mouthDepth: 1.8,
    mouthLift: 0.9,
  },
  // eyes on the page, a touch low; poseAt() sets it reading along a line
  working: { ...REST, earL: 0.6, earR: 0.6, eyeY: 0.5 },
  done: { ...REST, earL: 0.3, earR: 0.3, open: 0, arc: 1, mouthW: 4.2, mouthDepth: 2.8 },
  blocked: {
    ...REST,
    earL: -1,
    earR: -1,
    open: 0,
    squint: 1,
    mouthDepth: 1.2,
    mouthLift: 0.2,
    stamp: 1,
  },
  offline: { ...REST, earL: -0.25, earR: -0.25, open: 0, arc: -0.55, eyeY: 0.6, mouthGrey: 1 },
}

export function lerpPose(a: CatPose, b: CatPose, t: number): CatPose {
  const out = { ...a }
  for (const k of Object.keys(a) as (keyof CatPose)[]) out[k] = a[k] + (b[k] - a[k]) * t
  return out
}

const smooth = (x: number): number => x * x * (3 - 2 * x)

/** The working read: the eyes drift along a line left to right, then snap
 * back to the next one, blinking on the way. `look` is -1…1 and `blink` 0…1
 * (1 shut). It starts from the centre at t = 0, so it picks up from a still
 * face without a jump. `t` in ms. */
function read(t: number): { look: number; blink: number } {
  const IN = 400
  const SWEEP = 1700
  const BACK = 360
  const HOLD = 300
  const AMP = 0.7
  if (t < IN) return { look: -AMP * smooth(t / IN), blink: 0 }
  const p = (t - IN) % (SWEEP + BACK + HOLD)
  if (p < SWEEP) return { look: AMP * (2 * smooth(p / SWEEP) - 1), blink: 0 }
  if (p >= SWEEP + BACK) return { look: -AMP, blink: 0 }
  const q = (p - SWEEP) / BACK
  return {
    look: AMP * (1 - 2 * smooth(q)),
    blink: Math.sin(Math.PI * Math.min(Math.max(1.6 * q - 0.15, 0), 1)),
  }
}

/** A swing, -amp…amp: drift left, hold, cross to the right, hold, and back.
 * It starts from the centre at t = 0, so it picks up from a still face
 * without a jump. `t` in ms. */
function swing(t: number, hold: number, move: number, amp: number): number {
  const into = move / 2
  if (t < into) return -amp * smooth(t / into)
  const p = (t - into) % (2 * (hold + move))
  const leg = p % (hold + move)
  const from = p < hold + move ? -amp : amp
  return leg < hold ? from : from - 2 * from * smooth((leg - hold) / move)
}

/** Ears bending back and forth, `amp` at most, once every `ms`. */
const sway = (t: number, ms: number, amp: number): number => amp * Math.sin((t / ms) * Math.PI * 2)

/** Whether a state keeps moving after the morph into it has settled. */
export function isAnimated(state: CatState): boolean {
  return state === 'waiting' || state === 'thinking' || state === 'working'
}

/** A state's pose `t` ms after it began. The live states move: waiting rocks
 * gently with its eyes shut until the model speaks; thinking gazes up and
 * off to each side in turn, head cocked; working reads along line after
 * line, ears ticking, while the answer streams. */
export function poseAt(state: CatState, t: number): CatPose {
  const p = POSES[state]
  if (state === 'waiting') {
    const g = swing(t, 500, 1300, 0.6)
    return { ...p, look: g, tilt: g * 5, earSway: sway(t, 1800, 0.5) }
  }
  if (state === 'thinking') {
    const g = swing(t, 1800, 1100, 1)
    return {
      ...p,
      look: 0.65 * g,
      tilt: 8 * g,
      // the head cocks: the ear on the side it leans to drops
      earL: p.earL + 1.2 * Math.min(g, 0),
      earR: p.earR - 1.2 * Math.max(g, 0),
      earSway: sway(t, 2400, 0.3),
    }
  }
  if (state !== 'working') return p
  const { look: g, blink } = read(t)
  return {
    ...p,
    look: g,
    tilt: g * 3,
    open: p.open - blink,
    earL: p.earL - 0.3 * g,
    earR: p.earR + 0.3 * g,
    earSway: sway(t, 900, 1),
  }
}

// ——— geometry (viewBox 10 14 80 64)

type Pt = [number, number]
type Seg = [Pt, Pt, Pt, Pt]

const CX = 50
const HALF = 22
const YB = 55
const Y_TOP = 35.6
const BR = 4.5
const RISE = 0.55
const TIP = 0.32
const CHIN = 5

const EYE_Y = 47.5
const EYE_DX = 10
const MOUTH_Y = EYE_Y + 3.6

/** How far a full glance moves the face, the ears and the crown. */
const FACE_SHIFT = 3.2
const EAR_SHIFT = 2.2
const CROWN_SHIFT = 2.6
/** How far a full sway bends an ear tip sideways, how much of it the foot
 * takes, and how much the tip turns, so the ear swings from its base
 * instead of sliding. */
const EAR_SWAY = 2.6
const EAR_SWAY_ROOT = 0.3
const EAR_SWAY_TILT = 8
/** The head leans about a point between the paws. */
const PIVOT: Pt = [CX, 56]

// left ear, flattened / at rest / perked: tip, inner foot, splay (deg out from upright)
const EAR_FLAT = { T: [31.4, 36.4], B: [41.0, 37.6], splay: 44 }
const EAR_REST = { T: [33.8, 33.4], B: [42.6, 37.0], splay: 30 }
const EAR_PERK = { T: [34.6, 31.4], B: [42.6, 37.0], splay: 22 }

const mix = (a: number, b: number, t: number): number => a + (b - a) * t
const mixPt = (a: readonly number[], b: readonly number[], t: number): Pt => [
  mix(a[0]!, b[0]!, t),
  mix(a[1]!, b[1]!, t),
]

function ear(v: number): { T: Pt; B: Pt; splay: number } {
  const [a, b, t] = v < 0 ? [EAR_REST, EAR_FLAT, -v] : [EAR_REST, EAR_PERK, v]
  return { T: mixPt(a.T, b.T, t), B: mixPt(a.B, b.B, t), splay: mix(a.splay, b.splay, t) }
}

const dist = (a: Pt, b: Pt): number => Math.hypot(b[0] - a[0], b[1] - a[1])

function unit(x: number, y: number): Pt {
  const n = Math.hypot(x, y)
  return [x / n, y / n]
}

/** Rotates p about PIVOT, clockwise on screen for positive degrees. */
function lean([x, y]: Pt, deg: number): Pt {
  const a = (deg * Math.PI) / 180
  const c = Math.cos(a)
  const s = Math.sin(a)
  const dx = x - PIVOT[0]
  const dy = y - PIVOT[1]
  return [PIVOT[0] + c * dx - s * dy, PIVOT[1] + s * dx + c * dy]
}

/** Cubic a → b leaving along da and arriving along db, handles la and lb long. */
function seg(a: Pt, da: Pt, la: number, b: Pt, db: Pt, lb: number): Seg {
  return [a, [a[0] + da[0] * la, a[1] + da[1] * la], [b[0] - db[0] * lb, b[1] - db[1] * lb], b]
}

function split([a, b, c, d]: Seg): [Seg, Seg] {
  const ab = mixPt(a, b, 0.5)
  const bc = mixPt(b, c, 0.5)
  const cd = mixPt(c, d, 0.5)
  const abc = mixPt(ab, bc, 0.5)
  const bcd = mixPt(bc, cd, 0.5)
  const m = mixPt(abc, bcd, 0.5)
  return [
    [a, ab, abc, m],
    [m, bcd, cd, d],
  ]
}

/** One half of the line, drawn as the left half: from the paw up the side,
 * over the ear tip T to its inner foot B, then the head arcs over to the
 * crown C, where it runs flat. */
function half(T: Pt, B: Pt, splay: number, C: Pt): Seg[] {
  const P: Pt = [CX - HALF, YB]
  const sp = (splay * Math.PI) / 180
  const dT: Pt = [Math.cos(sp), -Math.sin(sp)]
  const slope = (2 * (B[1] - C[1])) / (C[0] - B[0])
  const dB = unit(1, -slope)
  const k = (C[0] - B[0]) / 3
  const outer = seg(P, [0, -1], RISE * dist(P, T), T, dT, TIP * dist(P, T) * 0.5)
  return [
    ...split(outer),
    seg(T, dT, TIP * dist(T, B), B, dB, 0.4 * dist(T, B)),
    [B, [B[0] + k, B[1] - slope * k], [C[0] - k, C[1]], C],
  ]
}

const f = (n: number): string => n.toFixed(2)
const cmd = ([, c1, c2, b]: Seg): string =>
  `C${f(c1[0])} ${f(c1[1])} ${f(c2[0])} ${f(c2[1])} ${f(b[0])} ${f(b[1])}`
const flip = (p: Pt): Pt => [2 * CX - p[0], p[1]]

/** The head's segments, paw to paw. The right half is built mirrored (where
 * look, tilt and the ear sway flip sign), so a turn or a lean, which break
 * the symmetry, still keep every join smooth. */
function head(p: CatPose): Seg[] {
  const side = (v: number, look: number, tilt: number, sway: number): Seg[] => {
    const e = ear(v)
    /** The tip takes the full sway, the foot only `EAR_SWAY_ROOT` of it, and
     * the tip turns with it: the ear swings from its base instead of sliding. */
    const shift = (q: Pt, k: number): Pt => lean([q[0] + look * EAR_SHIFT + sway * EAR_SWAY * k, q[1]], tilt)
    const crown = lean([CX + look * CROWN_SHIFT, Y_TOP], tilt)
    const splay = e.splay - tilt - sway * EAR_SWAY_TILT
    return half(shift(e.T, 1), shift(e.B, EAR_SWAY_ROOT), splay, crown)
  }
  const left = side(p.earL, p.look, p.tilt, p.earSway)
  const right = side(p.earR, -p.look, -p.tilt, -p.earSway)
  return [...left, ...right.reverse().map(([a, b, c, d]): Seg => [flip(d), flip(c), flip(b), flip(a)])]
}

/** The ink line: paw, head, paw. */
export function lineD(p: CatPose): string {
  const l = CX - HALF
  const r = CX + HALF
  return (
    `M${f(l - 2 * BR)} ${f(YB)} A${BR} ${BR} 0 0 0 ${f(l)} ${f(YB)} ` +
    head(p).map(cmd).join(' ') +
    ` A${BR} ${BR} 0 0 0 ${f(r + 2 * BR)} ${f(YB)}`
  )
}

/** The paper face inside the line (dark mode only): the head, closed by a soft chin. */
export function faceD(p: CatPose): string {
  const l = CX - HALF
  const r = CX + HALF
  const y = YB + CHIN
  return (
    `M${f(l)} ${f(YB)} ${head(p).map(cmd).join(' ')} ` +
    `Q${f(r)} ${f(y)} ${f(CX)} ${f(y)} Q${f(l)} ${f(y)} ${f(l)} ${f(YB)} Z`
  )
}

/** Eyes and mouth lean with the head; apply this to the group holding them. */
export function featuresTransform(p: CatPose): string {
  return `rotate(${f(p.tilt)} ${PIVOT[0]} ${PIVOT[1]})`
}

const K = 0.5523 // cubic handle length for a quarter circle
const EYE_W = 3.2 // half the line's length

/** Both eyes as one path. Each eye is two cubics end → middle → end, so a
 * dot (zero length; the round caps draw it), a line, a hump and a > < are
 * all the same commands. */
export function eyesD(p: CatPose): string {
  const y = EYE_Y + p.eyeY
  const x0 = CX + p.look * FACE_SHIFT
  const s = (pt: Pt): string => `${f(pt[0])} ${f(pt[1])}`
  return [-1, 1]
    .map((side) => {
      const x = x0 + side * EYE_DX
      const dir = -side // which way the > < points: toward the nose
      const w = Math.max(EYE_W * (1 - p.open), 0.01)
      const h = p.arc * w
      // line / hump: ends at ±w, middle raised by h
      const L: Pt = [x - w, y + h / 2]
      const M: Pt = [x, y - h / 2]
      const R: Pt = [x + w, y + h / 2]
      const round: Pt[] = [
        L,
        [x - w, y + h / 2 - K * h],
        [x - K * w, y - h / 2],
        M,
        [x + K * w, y - h / 2],
        [x + w, y + h / 2 - K * h],
        R,
      ]
      // > <: both ends pulled back from the nose, the middle pushed toward it
      const A: Pt = [x - 3 * dir, y - 2.6]
      const V: Pt = [x + 2 * dir, y]
      const Z: Pt = [x - 3 * dir, y + 2.6]
      const fold: Pt[] = [
        A,
        mixPt(A, V, 1 / 3),
        mixPt(A, V, 2 / 3),
        V,
        mixPt(V, Z, 1 / 3),
        mixPt(V, Z, 2 / 3),
        Z,
      ]
      // the right eye runs toward its nose side first, so its fold is mirrored
      const to = side < 0 ? fold : [...fold].reverse()
      const q = round.map((pt, i) => mixPt(pt, to[i]!, p.squint))
      return `M${s(q[0]!)} C${s(q[1]!)} ${s(q[2]!)} ${s(q[3]!)} C${s(q[4]!)} ${s(q[5]!)} ${s(q[6]!)}`
    })
    .join(' ')
}

/** Dot → line: the stroke thins as the eye closes, a little more when squeezed. */
export function eyeWidth(p: CatPose): number {
  return mix(mix(2.8, 5.8, p.open), 2.6, p.squint)
}

/** The ω mouth: two little u's side by side. */
export function mouthD(p: CatPose): string {
  const cx = CX + p.look * FACE_SHIFT
  const lx = cx - p.mouthW
  const rx = cx + p.mouthW
  const ty = MOUTH_Y - p.mouthLift
  const k = (p.mouthDepth * 4) / 3 // handle drop that makes the bowl reach `depth`
  return (
    `M${f(lx)} ${f(ty)} C${f(lx)} ${f(ty + k)} ${f(cx)} ${f(MOUTH_Y + k)} ${f(cx)} ${f(MOUTH_Y)} ` +
    `C${f(cx)} ${f(MOUTH_Y + k)} ${f(rx)} ${f(ty + k)} ${f(rx)} ${f(ty)}`
  )
}
