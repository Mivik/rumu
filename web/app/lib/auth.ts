import { reactive } from 'vue'

export const auth = reactive({
  status: 'checking' as 'checking' | 'required' | 'ready',
  error: '' as '' | 'invalid' | 'unreachable',
})

// Only kept until the first successful exchange; persistence is an HttpOnly cookie.
let pendingToken: string | null = null
let attempt = 0

/** Take the token out of the address bar — rewriting the current history
 *  entry in place, so it never sits in back/forward history either — and
 *  return it, or null when the fragment carries none. */
export function captureToken(): string | null {
  const params = new URLSearchParams(window.location.hash.slice(1))
  const token = params.get('token')
  if (token === null) return null
  params.delete('token')
  const rest = params.toString()
  window.history.replaceState(
    window.history.state,
    '',
    window.location.pathname + window.location.search + (rest ? `#${rest}` : ''),
  )
  return token
}

export function initAuth(): void {
  // The inline head script (nuxt.config.ts) strips the fragment before the
  // first paint and stashes it on the window; read that when it ran.
  const early = (window as { __rumuToken?: string }).__rumuToken
  if (early !== undefined) {
    delete (window as { __rumuToken?: string }).__rumuToken
    if (early) setToken(early)
    return
  }
  const token = captureToken()
  if (token !== null) setToken(token)
}

export function setToken(token: string): void {
  attempt += 1
  pendingToken = token.trim()
  auth.status = 'checking'
  auth.error = ''
}

/** Check before each WS handshake so a 401 never becomes an endless reconnect. */
export async function authorize(): Promise<boolean> {
  const current = ++attempt
  const token = pendingToken
  let response: Response
  try {
    response = await fetch('/auth', {
      method: token === null ? 'GET' : 'POST',
      headers: token === null ? undefined : { Authorization: `Bearer ${token}` },
      credentials: 'same-origin',
      cache: 'no-store',
      signal: AbortSignal.timeout(10_000),
    })
    if (response.status !== 204 && response.status !== 401) throw new Error('authentication unavailable')
  } catch {
    if (current !== attempt) return false
    if (auth.status !== 'ready') {
      auth.status = 'required'
      auth.error = 'unreachable'
    }
    throw new Error('authentication unavailable')
  }
  if (current !== attempt) return false
  pendingToken = null
  if (response.status === 401) {
    auth.status = 'required'
    auth.error = token === null ? '' : 'invalid'
    return false
  }
  auth.status = 'ready'
  auth.error = ''
  return true
}
