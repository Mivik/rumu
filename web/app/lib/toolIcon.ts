// The glyph a tool call's row leads with: what the call does, not which
// plugin served it. Every row used to share the terminal's icon, which said
// nothing — `bash` is the only call that really is a terminal, so only it
// keeps one. A failed or blocked row keeps its status icon instead (ToolList,
// CodeCalls), and a name we don't know (a plugin's own tool) gets the wrench.
const ICONS: Record<string, string> = {
  read: 'read',
  write: 'write',
  edit: 'edit',
  list: 'list',
  bash: 'bash',
  show: 'artifact',
  search: 'search',
  code: 'code',
  agent: 'agent',
  ask: 'ask',
  task_kill: 'stop',
  card_mount: 'deck',
  workspace_use: 'switch',
  call_tool: 'mcp',
}

/** The Icon name for a `name` tool call. */
export function toolIcon(name: string): string {
  return ICONS[name] ?? 'tool'
}
