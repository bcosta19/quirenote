// Tipos espelhados de internal/vault e internal/watch (Go).

export interface Entry {
  name: string
  path: string
  dir: boolean
  children?: Entry[]
}

export interface Note {
  path: string
  content: string
  /** UnixNano como string: é opaco para o front, só volta ao servidor. */
  mtime: string
}

export interface VaultEvent {
  path: string
  op: 'write' | 'create' | 'remove' | 'rename'
}

export class ConflictError extends Error {
  constructor(public current: Note) {
    super('arquivo foi modificado no disco')
  }
}

const enc = (p: string) => p.split('/').map(encodeURIComponent).join('/')

async function json<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const body = await res.json().catch(() => ({ error: res.statusText }))
    throw new Error(body.error ?? res.statusText)
  }
  return res.json()
}

export const fileUrl = (path: string) => `/api/files/${enc(path)}`

export async function getTree(): Promise<Entry[]> {
  return json(await fetch('/api/tree'))
}

export async function getNote(path: string): Promise<Note> {
  return json(await fetch(`/api/notes/${enc(path)}`))
}

/** mtime = versão que o editor leu; "0" força a gravação. */
export async function saveNote(path: string, content: string, mtime: string): Promise<Note> {
  const res = await fetch(`/api/notes/${enc(path)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ content, mtime }),
  })
  if (res.status === 409) throw new ConflictError(await res.json())
  return json(res)
}

export async function createDir(path: string): Promise<{ path: string }> {
  return json(await fetch(`/api/dirs/${enc(path)}`, { method: 'POST' }))
}

export function subscribe(onEvent: (e: VaultEvent) => void): () => void {
  const es = new EventSource('/api/events')
  es.onmessage = (m) => onEvent(JSON.parse(m.data))
  return () => es.close()
}
