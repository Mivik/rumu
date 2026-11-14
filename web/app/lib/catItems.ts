// What a cat wears (`deck.yaml cat.items`): each item is drawn from the
// cat's anchors in the pose on screen, so it leans, glances and morphs with
// the cat. Line items use the ink; colored ones take the cat's tone.

import { anchors, eyeDx, faceSpread, type CatAnchors, type CatPose, type Pt } from './cat'

/** One shape of an item. Unset fill / stroke draw nothing. */
export interface CatPrim {
  d: string
  fill?: string
  stroke?: string
  width?: number
  transform?: string
  /** A class on the path (the blush fades by theme). */
  class?: string
}

/** Below the line, on the face (leaning with the features), or on the head. */
export type CatLayer = 'body' | 'face' | 'head'

interface Item {
  layer: CatLayer
  /** Items of one slot replace each other; face items combine freely. */
  slot?: 'top' | 'neck'
  draw: (p: CatPose, a: CatAnchors, tone: string) => CatPrim[]
}

const f = (n: number): string => n.toFixed(2)
const mix = (a: Pt, b: Pt, t: number): Pt => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
const rot = (deg: number, [x, y]: Pt): string => `rotate(${f(deg)} ${f(x)} ${f(y)})`
const soft = (tone: string, k = 28): string => `color-mix(in oklab, ${tone} ${k}%, var(--surface))`
const ellipse = (cx: number, cy: number, rx: number, ry = rx): string =>
  `M${f(cx - rx)} ${f(cy)} a${f(rx)} ${f(ry)} 0 1 0 ${f(2 * rx)} 0 a${f(rx)} ${f(ry)} 0 1 0 ${f(-2 * rx)} 0`

const ITEMS = {
  blush: {
    layer: 'face',
    draw: (p, a) => {
      const dx = 14.5 * faceSpread(p)
      const y = a.eyeY + 4.4
      return [
        {
          d: ellipse(a.faceX - dx, y, 3.1, 1.7) + ellipse(a.faceX + dx, y, 3.1, 1.7),
          fill: 'var(--brand)',
          class: 'cat-blush',
        },
      ]
    },
  },
  whiskers: {
    layer: 'face',
    draw: (p, a) => {
      const k = faceSpread(p)
      const y = a.mouthY - 0.4
      const side = (s: number): string => {
        const x0 = a.faceX + s * 13.5 * k
        const x1 = a.faceX + s * 21 * k
        return `M${f(x0)} ${f(y - 1)} L${f(x1)} ${f(y - 2.6)} M${f(x0)} ${f(y + 1.2)} L${f(x1)} ${f(y + 1.8)}`
      }
      return [{ d: side(-1) + side(1), stroke: 'var(--ink-3)', width: 1.2 }]
    },
  },
  glasses: {
    layer: 'face',
    // the frames sit on the face; the eyes move behind them
    draw: (p, a) => {
      const r = 4.5 + 0.6 * Math.min(p.wide, 0)
      const dx = eyeDx(p)
      const y = a.eyeY + 0.3 * p.eyeY
      const l = a.faceX - dx
      const rr = a.faceX + dx
      const bridge = `M${f(l + r)} ${f(y - 0.6)} Q${f(a.faceX)} ${f(y - 2.6)} ${f(rr - r)} ${f(y - 0.6)}`
      return [{ d: ellipse(l, y, r) + ellipse(rr, y, r) + bridge, stroke: 'var(--ink)', width: 1.6 }]
    },
  },
  stripes: {
    layer: 'head',
    draw: (p, a) => {
      const [x, y] = a.crown
      const d = [-4, 0, 4]
        .map(
          (dx) =>
            `M${f(x + dx)} ${f(y + 2.6 + Math.abs(dx) * 0.15)} L${f(x + dx * 1.05)} ${f(y + (dx ? 5.6 : 6.6))}`,
        )
        .join(' ')
      return [{ d, stroke: 'var(--ink)', width: 2, transform: rot(p.tilt, a.crown) }]
    },
  },
  bow: {
    layer: 'head',
    slot: 'top',
    draw: (p, a, tone) => {
      const [x, y] = mix(a.earFootR, a.earTipR, 0.35)
      const transform = `translate(${f(x + 1.5)} ${f(y - 1)}) rotate(${f(p.tilt + 25)})`
      return [
        {
          d: 'M0 0 C-2 -4.4 -6.4 -3.6 -6 0 C-6.4 3.6 -2 4.4 0 0 Z M0 0 C2 -4.4 6.4 -3.6 6 0 C6.4 3.6 2 4.4 0 0 Z',
          fill: soft(tone),
          stroke: tone,
          width: 1.5,
          transform,
        },
        { d: ellipse(0, 0, 1.5), fill: tone, transform },
      ]
    },
  },
  sprout: {
    layer: 'head',
    slot: 'top',
    draw: (p, a) => {
      const [x, y] = a.crown
      const transform = rot(p.tilt, a.crown)
      const leaf = soft('var(--pine)', 30)
      const P = (dx: number, dy: number): string => `${f(x + dx)} ${f(y + dy)}`
      return [
        { d: `M${P(0, -1.5)} Q${P(0.6, -5)} ${P(0, -8)}`, stroke: 'var(--pine)', width: 1.8, transform },
        {
          d:
            `M${P(0, -7.6)} C${P(-1.6, -10.8)} ${P(-5.4, -10.4)} ${P(-6, -9.4)} C${P(-5, -7.4)} ${P(-2, -7)} ${P(0, -7.6)} Z ` +
            `M${P(0, -7.6)} C${P(1.4, -11.4)} ${P(4.6, -12)} ${P(5.6, -11)} C${P(4.8, -8.6)} ${P(2, -7.6)} ${P(0, -7.6)} Z`,
          fill: leaf,
          stroke: 'var(--pine)',
          width: 1.8,
          transform,
        },
      ]
    },
  },
  hat: {
    layer: 'head',
    slot: 'top',
    draw: (p, a, tone) => {
      const [x, y] = a.crown
      const transform = rot(p.tilt + 8, a.crown)
      return [
        {
          d: `M${f(x - 5)} ${f(y - 1.2)} L${f(x)} ${f(y - 12)} L${f(x + 5)} ${f(y - 1.2)} Z`,
          fill: soft(tone, 25),
          stroke: tone,
          width: 1.6,
          transform,
        },
        {
          d: `M${f(x - 2.6)} ${f(y - 6.6)} L${f(x + 2.6)} ${f(y - 4.6)}`,
          stroke: tone,
          width: 1.6,
          transform,
        },
        { d: ellipse(x, y - 13, 1.6), fill: tone, transform },
      ]
    },
  },
  headphones: {
    layer: 'head',
    slot: 'top',
    draw: (p, a, tone) => {
      const l: Pt = [a.sideL[0] - 1.2, a.sideL[1]]
      const r: Pt = [a.sideR[0] + 1.2, a.sideR[1]]
      const top = a.crown[1] - 8.5
      const cup = ([x, y]: Pt): CatPrim => ({
        d: `M${f(x - 2.6)} ${f(y - 2)} a2.4 2.4 0 0 1 5.2 0 v4 a2.4 2.4 0 0 1 -5.2 0 Z`,
        fill: tone,
        stroke: tone,
        width: 2,
        transform: rot(p.tilt, [x, y]),
      })
      return [
        {
          d: `M${f(l[0])} ${f(l[1] - 3)} C${f(l[0] - 1)} ${f(top)} ${f(r[0] + 1)} ${f(top)} ${f(r[0])} ${f(r[1] - 3)}`,
          stroke: tone,
          width: 2,
        },
        cup(l),
        cup(r),
      ]
    },
  },
  bell: {
    layer: 'body',
    slot: 'neck',
    draw: (_p, a, tone) => {
      const y = a.pawY + 1.8
      return [
        {
          d: `M${f(a.pawL + 3)} ${f(y)} Q${f(50)} ${f(y + 4.8)} ${f(a.pawR - 3)} ${f(y)}`,
          stroke: tone,
          width: 1.6,
        },
        { d: ellipse(50, y + 5.2, 2.9), fill: soft('var(--warn)', 45), stroke: 'var(--warn)', width: 1.2 },
        { d: `M48.4 ${f(y + 5.8)} H51.6`, stroke: 'var(--warn)', width: 1 },
      ]
    },
  },
} satisfies Record<string, Item>

export type CatItem = keyof typeof ITEMS
const byName: Record<string, Item | undefined> = ITEMS

/** Every item, in the order the picker shows them. */
export const CAT_ITEMS = Object.keys(ITEMS) as CatItem[]

/** The slot an item fills, if it shares one. */
export const itemSlot = (name: string): string | undefined => byName[name]?.slot

/** The items of `names` drawn for pose `p`, by layer. Unknown names are skipped. */
export function drawItems(p: CatPose, names: readonly string[], tone: string): Record<CatLayer, CatPrim[]> {
  const out: Record<CatLayer, CatPrim[]> = { body: [], face: [], head: [] }
  if (!names.length) return out
  const a = anchors(p)
  for (const n of names) {
    const item = byName[n]
    if (item) out[item.layer].push(...item.draw(p, a, tone))
  }
  return out
}
