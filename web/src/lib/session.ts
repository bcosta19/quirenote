// Where the user left off: the open note and document, restored when the app opens
// without a URL state (e.g. from the desktop launcher) and kept across reloads.

export interface Session {
  note: string | null
  doc: string | null // vault PDF/EPUB
  page: number
  local: string | null // name of a PDF/EPUB opened from the device (its handle is in IndexedDB)
}

const KEY = 'quirenote:session'

export function loadSession(): Session | null {
  try {
    const s = JSON.parse(localStorage.getItem(KEY) || 'null')
    if (!s || typeof s !== 'object') return null
    const str = (v: unknown) => (typeof v === 'string' && v ? v : null)
    return { note: str(s.note), doc: str(s.doc), page: Number(s.page) || 1, local: str(s.local) }
  } catch {
    return null // a corrupted value must not keep the app from starting
  }
}

export function saveSession(s: Session) {
  localStorage.setItem(KEY, JSON.stringify(s))
}

// Device files: browsers cannot reopen a file by path, but Chromium can keep a
// FileSystemFileHandle (only a reference, not the file contents) in IndexedDB.

type Handle = FileSystemFileHandle & {
  queryPermission?(o: { mode: 'read' }): Promise<PermissionState>
  requestPermission?(o: { mode: 'read' }): Promise<PermissionState>
}

/** The file picker that returns a reusable handle (Chromium, in a secure context). */
export const canKeepFiles = typeof window !== 'undefined' && 'showOpenFilePicker' in window

export async function pickFile(): Promise<{ file: File; handle: Handle } | null> {
  try {
    const [handle] = await (window as any).showOpenFilePicker({
      types: [
        {
          description: 'PDF or EPUB',
          accept: { 'application/pdf': ['.pdf'], 'application/epub+zip': ['.epub'] },
        },
      ],
    })
    return { file: await handle.getFile(), handle }
  } catch (e) {
    if ((e as Error).name === 'AbortError') return null // picker closed
    throw e
  }
}

function db(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open('quirenote', 1)
    req.onupgradeneeded = () => req.result.createObjectStore('files')
    req.onsuccess = () => resolve(req.result)
    req.onerror = () => reject(req.error)
  })
}

async function tx<T>(mode: IDBTransactionMode, run: (s: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const d = await db()
  return new Promise((resolve, reject) => {
    const req = run(d.transaction('files', mode).objectStore('files'))
    req.onsuccess = () => resolve(req.result)
    req.onerror = () => reject(req.error)
  }).finally(() => d.close()) as Promise<T>
}

export const saveLocalHandle = (h: Handle) => tx('readwrite', (s) => s.put(h, 'local')).catch(() => {})
export const clearLocalHandle = () => tx('readwrite', (s) => s.delete('local')).catch(() => {})
export const loadLocalHandle = () => tx<Handle | undefined>('readonly', (s) => s.get('local')).catch(() => undefined)

/**
 * Reads the remembered file. Without user interaction the browser may only report
 * "prompt"; then `ask` must be true and the call must come from a click.
 */
export async function readLocalHandle(h: Handle, ask: boolean): Promise<File | null> {
  let perm: PermissionState = (await h.queryPermission?.({ mode: 'read' })) ?? 'granted'
  if (perm === 'prompt' && ask) perm = (await h.requestPermission?.({ mode: 'read' })) ?? 'denied'
  if (perm !== 'granted') return null
  try {
    return await h.getFile()
  } catch {
    return null // moved or deleted
  }
}
