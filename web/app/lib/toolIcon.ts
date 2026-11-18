// The glyph a tool call's row leads with: what the call does, not which
// plugin served it. Every row used to share the terminal's icon, which said
// nothing — `bash` is the only call that really is a terminal, so only it
// keeps one. A failed or blocked row keeps its status icon instead (ToolList,
// CodeCalls), and a name we don't know (a plugin's own tool) gets the wrench.
const ICONS: Record<string, string> = {
  read: 'read',
  write: 'write',
  edit: 'edit',
  delete: 'trash',
  list: 'list',
  bash: 'bash',
  show: 'artifact',
  search: 'search',
  code: 'code',
  agent: 'agent',
  delegate: 'agent',
  ask: 'ask',
  card_mount: 'deck',
  // A background task's own name; those are MCP calls in practice.
  invoke: 'mcp',
  // `invoke` rows stand for their action (`displayCall`).
  'task.kill': 'stop',
  'workspace.use': 'switch',
}

/** The Icon name for a `name` tool call; an MCP tool (`<server>/<tool>`)
 * gets the puzzle piece. */
export function toolIcon(name: string): string {
  return ICONS[name] ?? (name.includes('/') ? 'mcp' : 'tool')
}
