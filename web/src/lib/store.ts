/* Tiny external stores + a non-overlapping poller.
 * Components subscribe to exactly the store they read (useSyncExternalStore),
 * so a devices update never re-renders the alerts list and vice versa. */

import { useSyncExternalStore } from 'react'
import { ApiError } from '@/lib/api'

export interface Store<T> {
  get(): T
  set(next: T | ((prev: T) => T)): void
  subscribe(listener: () => void): () => void
}

export function createStore<T>(initial: T): Store<T> {
  let value = initial
  const listeners = new Set<() => void>()
  return {
    get: () => value,
    set(next) {
      const v = typeof next === 'function' ? (next as (p: T) => T)(value) : next
      if (Object.is(v, value)) return
      value = v
      listeners.forEach((l) => l())
    },
    subscribe(l) {
      listeners.add(l)
      return () => listeners.delete(l)
    },
  }
}

export function useStore<T>(store: Store<T>): T {
  return useSyncExternalStore(store.subscribe, store.get, store.get)
}

/* ═══ resource state ══════════════════════════════════════════════════ */

export interface Res<T> {
  /** Last good data (kept when a later fetch fails) */
  data: T | null
  /** Error of the most recent fetch, null when it succeeded */
  error: ApiError | null
  /** When `data` was last fetched successfully (ms, local clock) */
  updatedAt: number | null
  /** `data` is from an earlier fetch: the latest one failed */
  stale: boolean
  /** No fetch has completed yet */
  loading: boolean
}

export const emptyRes = <T,>(): Res<T> => ({ data: null, error: null, updatedAt: null, stale: false, loading: true })

function toApiError(e: unknown): ApiError {
  if (e instanceof ApiError) return e
  return new ApiError(0, e instanceof Error ? e.message : String(e))
}

/* ═══ poller ═════════════════════════════════════════════════════════
 * - the next fetch is scheduled only after the current one completes
 * - `stop()` aborts the in-flight request
 * - hidden tab → interval × HIDDEN_FACTOR; becoming visible → refresh now
 * - unchanged payloads keep the previous `data` reference (cheap memo) */

export const HIDDEN_FACTOR = 6

export class Poller<T> {
  readonly store: Store<Res<T>>
  private timer: ReturnType<typeof setTimeout> | null = null
  private ctrl: AbortController | null = null
  private inFlight: Promise<void> | null = null
  private again = false
  private running = false
  private lastJson = ''

  constructor(
    private fetcher: (signal: AbortSignal) => Promise<T>,
    public intervalMs: number,
    store?: Store<Res<T>>,
  ) {
    this.store = store ?? createStore<Res<T>>(emptyRes<T>())
  }

  start() {
    if (this.running) return
    this.running = true
    void this.run()
  }

  stop() {
    this.running = false
    if (this.timer) clearTimeout(this.timer)
    this.timer = null
    this.ctrl?.abort()
    this.ctrl = null
  }

  /** Fetch now (coalesces with an in-flight fetch). Resolves when done. */
  refresh(): Promise<void> {
    if (this.inFlight) {
      this.again = true
      return this.inFlight
    }
    return this.run()
  }

  private run(): Promise<void> {
    if (this.timer) clearTimeout(this.timer)
    this.timer = null
    const ctrl = new AbortController()
    this.ctrl = ctrl
    const p = (async () => {
      try {
        const data = await this.fetcher(ctrl.signal)
        if (ctrl.signal.aborted) return
        const json = JSON.stringify(data)
        const same = json === this.lastJson
        this.lastJson = json
        this.store.set((prev) => ({
          data: same && prev.data !== null ? prev.data : data,
          error: null,
          updatedAt: Date.now(),
          stale: false,
          loading: false,
        }))
      } catch (e) {
        if (ctrl.signal.aborted || (e as Error)?.name === 'AbortError') return
        const err = toApiError(e)
        this.store.set((prev) => ({ ...prev, error: err, stale: prev.data !== null, loading: false }))
      }
    })()
    const done: Promise<void> = p.finally(() => {
      if (this.inFlight === done) this.inFlight = null
      if (this.ctrl === ctrl) this.ctrl = null
      if (ctrl.signal.aborted) return // stopped: a restarted poller owns scheduling
      if (this.again) {
        this.again = false
        if (this.running) void this.run()
        return
      }
      this.schedule()
    })
    this.inFlight = done
    return done
  }

  private schedule() {
    if (!this.running) return
    const hidden = typeof document !== 'undefined' && document.visibilityState === 'hidden'
    this.timer = setTimeout(() => void this.run(), hidden ? this.intervalMs * HIDDEN_FACTOR : this.intervalMs)
  }
}
