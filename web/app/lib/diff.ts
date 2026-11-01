// File diffs for the tool rows: the `workspace.file_patch` payload the
// backend attaches to write/edit results, and the browser-side fallback for
// results without one (older sessions) — a plain line-level LCS over the
// call's own strings, enough to render a +/- view and count a row's changes.

import type { Content, FilePatch } from './types'

/** The `workspace.file_patch` payload of a tool result's content, if any. */
export function filePatchOf(content: Content | null | undefined): FilePatch | null {
  const d = content?.data
  return d && d.tag === 'workspace.file_patch' && d.v === 1 ? (d.data as FilePatch) : null
}

/** One line of a fallback diff: context, removed or added. */
export interface SignLine {
  sign: ' ' | '-' | '+'
  text: string
}

/** `a\nb` → `['a', 'b']`; a trailing newline ends no extra line. */
export function splitLines(text: string): string[] {
  if (!text) return []
  const lines = text.split('\n')
  if (lines.length > 1 && lines[lines.length - 1] === '') lines.pop()
  return lines
}

/** A line-level LCS diff — the fallback when no hunks came from the backend. */
export function diffLines(oldText: string, newText: string): SignLine[] {
  const a = splitLines(oldText)
  const b = splitLines(newText)
  // Edits share their context: trim the common head and tail first so the
  // LCS table only covers what actually changed.
  let head = 0
  while (head < a.length && head < b.length && a[head] === b[head]) head++
  let tailA = a.length
  let tailB = b.length
  while (tailA > head && tailB > head && a[tailA - 1] === b[tailB - 1]) {
    tailA--
    tailB--
  }
  const mid: SignLine[] = []
  const la = tailA - head
  const lb = tailB - head
  if (la > 0 && lb > 0 && la * lb <= 4_000_000) {
    // dp[i][j]: LCS length of a[head+i..tailA) and b[head+j..tailB).
    const dp: number[][] = Array.from({ length: la + 1 }, () => new Array<number>(lb + 1).fill(0))
    for (let i = la - 1; i >= 0; i--) {
      for (let j = lb - 1; j >= 0; j--) {
        dp[i]![j]! =
          a[head + i] === b[head + j] ? dp[i + 1]![j + 1]! + 1 : Math.max(dp[i + 1]![j]!, dp[i]![j + 1]!)
      }
    }
    let i = 0
    let j = 0
    while (i < la && j < lb) {
      if (a[head + i] === b[head + j]) {
        mid.push({ sign: ' ', text: a[head + i]! })
        i++
        j++
      } else if (dp[i + 1]![j]! >= dp[i]![j + 1]!) {
        mid.push({ sign: '-', text: a[head + i]! })
        i++
      } else {
        mid.push({ sign: '+', text: b[head + j]! })
        j++
      }
    }
    for (; i < la; i++) mid.push({ sign: '-', text: a[head + i]! })
    for (; j < lb; j++) mid.push({ sign: '+', text: b[head + j]! })
  } else {
    // Nothing shared (or table too big to bother): a whole-block replace.
    for (let i = head; i < tailA; i++) mid.push({ sign: '-', text: a[i]! })
    for (let j = head; j < tailB; j++) mid.push({ sign: '+', text: b[j]! })
  }
  return [
    ...a.slice(0, head).map((text): SignLine => ({ sign: ' ', text })),
    ...mid,
    ...a.slice(tailA).map((text): SignLine => ({ sign: ' ', text })),
  ]
}

/** Added / removed line counts of a diff. */
export interface DiffCounts {
  added: number
  removed: number
}

/** Added / removed line counts of a fallback diff. */
export function diffCounts(lines: SignLine[]): DiffCounts {
  let added = 0
  let removed = 0
  for (const l of lines) {
    if (l.sign === '+') added++
    else if (l.sign === '-') removed++
  }
  return { added, removed }
}
