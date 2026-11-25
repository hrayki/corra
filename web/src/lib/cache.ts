import type { StoredEvent } from '../proto/StoredEvent'

const DB = 'corra'
let dbp: Promise<IDBDatabase> | undefined

function db(): Promise<IDBDatabase> {
  dbp ??= new Promise((resolve, reject) => {
    const req = indexedDB.open(DB, 1)
    req.onupgradeneeded = () => {
      const d = req.result
      d.createObjectStore('kv')
      d.createObjectStore('events')
    }
    req.onsuccess = () => resolve(req.result)
    req.onerror = () => reject(req.error)
  })
  return dbp
}

function tx<T>(store: string, mode: IDBTransactionMode, f: (s: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  return db().then(
    (d) =>
      new Promise<T>((resolve, reject) => {
        const t = d.transaction(store, mode)
        const r = f(t.objectStore(store))
        r.onsuccess = () => resolve(r.result)
        r.onerror = () => reject(r.error)
      }),
  )
}

export const cache = {
  get: <T>(key: string) => tx<T | undefined>('kv', 'readonly', (s) => s.get(key)).catch(() => undefined),
  set: (key: string, v: unknown) => tx('kv', 'readwrite', (s) => s.put(v, key)).catch(() => undefined),
  events: (thread: string) => tx<StoredEvent[] | undefined>('events', 'readonly', (s) => s.get(thread)).catch(() => undefined),
  setEvents: (thread: string, events: StoredEvent[]) => tx('events', 'readwrite', (s) => s.put(events, thread)).catch(() => undefined),
}
