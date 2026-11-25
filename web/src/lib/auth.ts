export function hubBase(): string {
  return ''
}

export function deviceToken(): string | null {
  return localStorage.getItem('corra_token')
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const r = await fetch(hubBase() + path, {
    method: 'POST',
    headers: { 'content-type': 'application/json', ...(deviceToken() ? { authorization: `Bearer ${deviceToken()}` } : {}) },
    credentials: 'include',
    body: JSON.stringify(body),
  })
  const j = await r.json().catch(() => ({}))
  if (!r.ok) throw new Error(j.error ?? `The hub answered ${r.status}`)
  return j as T
}

export async function authState(): Promise<{ local: boolean; signed_in: boolean }> {
  const t = deviceToken()
  const r = await fetch(hubBase() + '/auth/state', {
    credentials: 'include',
    headers: t ? { authorization: `Bearer ${t}` } : {},
  })
  if (!r.ok) throw new Error(`The hub answered ${r.status}`)
  return r.json()
}

export async function signInWithKey(key: string) {
  await post('/auth/key', { key })
}

export async function pairDevice(code: string, name: string) {
  const { token } = await post<{ token: string }>('/auth/pair/finish', { code, name })
  localStorage.setItem('corra_token', token)
}

export async function newPairingCode(): Promise<string> {
  const { code } = await post<{ code: string }>('/auth/pair/start', {})
  return code
}

export async function signOut() {
  localStorage.removeItem('corra_token')
  await post('/auth/logout', {}).catch(() => {})
}
