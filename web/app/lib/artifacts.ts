// Artifacts: snapshots of workspace files the model `show`s the user. Each
// lives on the path as the `artifact` payload of a `show` result, its bytes a
// blob, so a branch sees exactly the versions it showed; the snapshots of one
// file on the path are its versions, in order. The open one is
// `state.artifact`: a result node id, shown only while that node is on the
// current path, or a file a link snapshotted (`fs.snapshot`), on no node.

import { computed } from 'vue'
import type { Artifact, ArtifactKind, Content, Node } from './types'
import { SCRATCH_WORKSPACE } from './types'
import { pathNodes } from './state/derived'
import { state, type LightboxImage } from './state/state'
import { blobUrl, scratchPath } from './attachments'

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

/** The open artifact: a snapshot while it is on the current path, or a
 *  linked file (version 0, outside the path's versions). */
export const openArtifact = computed<Pick<ArtifactVersion, 'artifact' | 'version'> | null>(() => {
  const open = state.artifact
  return typeof open === 'object' && open ? { artifact: open, version: 0 } : versionOf(open)
})

export function showArtifact(open: string | Artifact | null): void {
  state.artifact = open
  if (open) {
    state.agentView = null
    state.taskDetail = null
  }
}

/** Kinds whose blob is text the panel fetches and renders itself. */
export function isTextKind(kind: ArtifactKind): boolean {
  return kind === 'html' || kind === 'markdown' || kind === 'svg' || kind === 'code'
}

/** The blob, served as the artifact's media type when it has one, so the
 *  browser displays or plays it. */
export function artifactUrl(a: Artifact): string {
  return a.mime ? `${blobUrl(a.hash)}?type=${encodeURIComponent(a.mime)}` : blobUrl(a.hash)
}

/** Images a reply can put in its text as `![alt](address)`, by address:
 *  attachments under their `@scratch:.inbox/attachments/` copy, and image
 *  snapshots under their path — each with the path index of its node, in
 *  path order. Only bytes some node already holds, so a reply shows the same
 *  image however late it is reread. */
export const imagesOnPath = computed(() => {
  const byAddress = new Map<string, { image: LightboxImage; at: number }[]>()
  const add = (address: string, image: LightboxImage, at: number) => {
    const list = byAddress.get(address) ?? []
    list.push({ image, at })
    byAddress.set(address, list)
  }
  pathNodes.value.forEach((node, at) => {
    const body = node.body
    if (!('content' in body)) return
    for (const a of body.content.attachments ?? []) {
      if (a.kind !== 'image') continue
      const image = { src: blobUrl(a.hash), name: a.name, width: a.width, height: a.height }
      add(`${SCRATCH_WORKSPACE}:${scratchPath(a)}`, image, at)
    }
    const artifact = body.type === 'tool_result' && !body.is_error ? artifactOf(body.content) : null
    if (artifact?.kind === 'image')
      add(artifact.path, { src: artifactUrl(artifact), name: artifact.title }, at)
  })
  return byAddress
})

/** The image `![…](address)` in the text of `nodeId` shows: the latest one
 *  at that address before the node (the latest of all for text still
 *  streaming), or null. */
export function pathImage(address: string, nodeId?: string): LightboxImage | null {
  const list = imagesOnPath.value.get(address.replace(/:\.\//, ':'))
  if (!list) return null
  const at = nodeId ? pathNodes.value.findIndex((n) => n.id === nodeId) : -1
  const before = at < 0 ? list : list.filter((v) => v.at < at)
  return before.at(-1)?.image ?? null
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
