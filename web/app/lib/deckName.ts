// A deck's display name: a label, not a path (decks are stored and addressed
// by id).

/** The longest deck name, in characters (`DECK_NAME_MAX` in rumu-cards). */
export const DECK_NAME_MAX = 80

/** Trimmed, non-empty, at most `DECK_NAME_MAX` characters, no control chars. */
export function validDeckName(name: string): boolean {
  const trimmed = name.trim()
  return (
    trimmed.length > 0 &&
    Array.from(trimmed).length <= DECK_NAME_MAX &&
    !/[\u0000-\u001f\u007f]/.test(trimmed)
  )
}
