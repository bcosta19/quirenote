// Types mirrored from internal/vault and internal/watch (Go).

export interface Entry {
  name: string
  path: string
  dir: boolean
  children?: Entry[]
}

export interface Note {
  path: string
  content: string
  /** UnixNano as a string: opaque to the frontend, only sent back to the server. */
  mtime: string
}

export interface VaultEvent {
  path: string
  op: 'write' | 'create' | 'remove' | 'rename'
}

export class ExistsError extends Error {}

export class ConflictError extends Error {
  constructor(public current: Note) {
    super('file was modified on disk')
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

/** mtime = the version the editor read; "0" forces the write. */
export async function saveNote(path: string, content: string, mtime: string): Promise<Note> {
  const res = await fetch(`/api/notes/${enc(path)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ content, mtime }),
  })
  if (res.status === 409) throw new ConflictError(await res.json())
  return json(res)
}

/** Creates a new note; fails if it already exists (the server never overwrites). */
export async function createNote(path: string, content: string): Promise<Note> {
  const res = await fetch(`/api/notes/${enc(path)}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ content }),
  })
  if (res.status === 409) throw new ExistsError((await res.json()).error)
  return json(res)
}

export async function createDir(path: string): Promise<{ path: string }> {
  // The server requires JSON for everything that changes the vault (CSRF protection).
  return json(
    await fetch(`/api/dirs/${enc(path)}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' }),
  )
}

export interface SearchResult {
  path: string
  line: number
  parts: { text: string; hit?: boolean }[]
}

export async function searchNotes(q: string, signal?: AbortSignal): Promise<SearchResult[]> {
  return json(await fetch(`/api/search?q=${encodeURIComponent(q)}`, { signal }))
}

async function postJSON<T>(url: string, body: unknown): Promise<T> {
  const res = await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
  if (res.status === 409) throw new ExistsError((await res.json()).error)
  return json(res)
}

/** Renames/moves a file or folder; ExistsError if the destination already exists. */
export const moveItem = (from: string, to: string) => postJSON<{ path: string }>('/api/move', { from, to })

/** Moves to the vault trash (.trash); nothing is deleted. */
export const trashItem = (path: string) => postJSON<{ path: string }>('/api/trash', { path })

function toBase64(file: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result).replace(/^data:[^,]*,/, ''))
    reader.onerror = () => reject(reader.error ?? new Error('could not read the image'))
    reader.readAsDataURL(file)
  })
}

/** Stores a pasted/dropped image in the vault attachment folder; returns the created path. */
export async function uploadImage(note: string, file: Blob): Promise<string> {
  const { path } = await postJSON<{ path: string }>('/api/images', { note, data: await toBase64(file) })
  return path
}

/** Snippets of other notes with a [[link]] to the note. */
export async function getBacklinks(note: string, signal?: AbortSignal): Promise<SearchResult[]> {
  return json(await fetch(`/api/backlinks?note=${encodeURIComponent(note)}`, { signal }))
}

/** Which notes would have their [[links]] updated after renaming from → to (nothing is written). */
export async function previewRelink(from: string, to: string): Promise<{ files: string[]; ambiguous: boolean }> {
  return json(await fetch(`/api/relink?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`))
}

/** Updates the [[links]] in other notes after renaming from → to. */
export const relinkNotes = (from: string, to: string) =>
  postJSON<{ updated: string[]; skipped: string[]; ambiguous: boolean }>('/api/relink', { from, to })

export function subscribe(onEvent: (e: VaultEvent) => void): () => void {
  const es = new EventSource('/api/events')
  es.onmessage = (m) => onEvent(JSON.parse(m.data))
  return () => es.close()
}

export interface SetupStatus {
  configured: boolean
  allowed?: boolean // the server accepts choosing the vault from the browser
  suggested?: string // folder suggested for a fresh start
}

/** Whether a vault is configured (first run shows the setup screen otherwise). */
export async function getSetup(): Promise<SetupStatus> {
  return json(await fetch('/api/setup'))
}

/** Chooses the vault on first run: a new folder or an existing one (e.g. an Obsidian vault). */
export async function runSetup(mode: 'new' | 'existing', path: string): Promise<{ dir: string; obsidian: boolean }> {
  return json(
    await fetch('/api/setup', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ mode, path }),
    }),
  )
}
