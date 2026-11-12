// Artifacts: snapshots of workspace files the model `show`s the user. Each
// lives on the path as the `artifact` payload of a `show` result, its bytes a
// blob, so a branch sees exactly the versions it showed; the snapshots of one
// file on the path are its versions, in order. The open one is
// `state.artifact` (a result node id); it shows only while that node is on
// the current path.

import { computed } from 'vue'
import type { Artifact, Content, Node } from './types'
import { pathNodes } from './state/derived'
import { state } from './state/state'
import { blobUrl } from './attachments'

/** The `artifact` payload of a `show` result, if it is one. */
export function artifactOf(content: Content | null | undefined): Artifact | null {
  const d = content?.data
  return d && d.tag === 'artifact' && d.v === 1 ? (d.data as Artifact) : null
}

export interface ArtifactVersion {
  artifact: Artifact
  node: Node
  /** 1-based, among the snapshots of the same file on the path. */
  version: number
}

/** Every snapshot on the current path, by file, in path order. */
export const artifactsOnPath = computed(() => {
  const byPath = new Map<string, ArtifactVersion[]>()
  for (const node of pathNodes.value) {
    if (node.body.type !== 'tool_result' || node.body.is_error) continue
    const artifact = artifactOf(node.body.content)
    if (!artifact) continue
    const versions = byPath.get(artifact.path) ?? []
    versions.push({ artifact, node, version: versions.length + 1 })
    byPath.set(artifact.path, versions)
  }
  return byPath
})

/** The snapshot recorded by `node`, with its version, when it is on the path. */
export function versionOf(nodeId: string | null): ArtifactVersion | null {
  if (!nodeId) return null
  for (const versions of artifactsOnPath.value.values()) {
    const hit = versions.find((v) => v.node.id === nodeId)
    if (hit) return hit
  }
  return null
}

/** The open artifact, while it is on the current path. */
export const openArtifact = computed(() => versionOf(state.artifact))

export function showArtifact(nodeId: string | null): void {
  state.artifact = nodeId
  if (nodeId) {
    state.agentView = null
    state.taskDetail = null
  }
}

/** Blob text by hash: a snapshot never changes, so one fetch serves all. */
const texts = new Map<string, Promise<string>>()

export function artifactText(hash: string): Promise<string> {
  let text = texts.get(hash)
  if (!text) {
    text = fetch(blobUrl(hash)).then((res) => {
      if (!res.ok) throw new Error(`${res.status} ${res.statusText}`.trim())
      return res.text()
    })
    text.catch(() => texts.delete(hash))
    texts.set(hash, text)
  }
  return text
}

/** No network for a page: inline scripts, styles and data: media only. */
const PAGE_CSP =
  "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval'; style-src 'unsafe-inline'; " +
  'img-src data: blob:; font-src data:; media-src data: blob:'

/** The page as an opaque-origin frame runs it, the CSP meta first thing
 *  after the doctype: the parser opens the head for it, so it applies before
 *  any of the page's own markup (a script ahead of `<head>` included). */
export function sandboxedPage(html: string): string {
  const meta = `<meta http-equiv="Content-Security-Policy" content="${PAGE_CSP}">`
  const at = /^\s*<!doctype[^>]*>/i.exec(html)?.[0].length ?? 0
  return html.slice(0, at) + meta + html.slice(at)
}
