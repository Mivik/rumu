// The slash commands the composer knows: its own two (`/compact`, `/btw`,
// run in Composer.submit) and the ones plugins registered (lib/plugins).
// They need a session and a plain draft: no edit, no attachments.
import { computed, type Ref } from 'vue'
import { t } from '../../i18n'
import { plugins } from '../plugins'
import { state } from './state'

export interface Command {
  name: string
  /** What it does, in a few words. */
  label: string
  /** What may follow the name. */
  arg?: string
  icon: string
  tone?: 'brand' | 'agent'
}

/** The commands a composer offers; none while `off`. */
export function useCommands(off: Ref<boolean>) {
  const all = computed<Command[]>(() => {
    if (off.value) return []
    const own: Command[] = state.sessionId
      ? [
          {
            name: 'compact',
            label: t('chat.command.compact'),
            arg: t('chat.command.compactArg'),
            icon: 'history',
            tone: 'brand',
          },
          {
            name: 'btw',
            label: t('chat.side.title'),
            arg: t('chat.command.btwArg'),
            icon: 'side',
            tone: 'agent',
          },
        ]
      : []
    const taken = new Set(own.map((c) => c.name))
    return [
      ...own,
      ...plugins.commands
        .filter((c) => !taken.has(c.name))
        .map((c) => ({ name: c.name, label: c.description ?? c.plugin, icon: 'plugin' })),
    ]
  })

  /** The commands whose name starts with `query`. */
  function matching(query: string): Command[] {
    return all.value.filter((c) => c.name.startsWith(query))
  }

  /** The known command `text` opens with (`/name`, then a space or the end). */
  function leading(text: string): Command | null {
    const m = /^\/([\w-]+)(?=\s|$)/.exec(text)
    return (m && all.value.find((c) => c.name === m[1])) || null
  }

  return { matching, leading }
}
