/* The dashboard's single data layer.
 *
 * - one Poller per hub resource, each with last-good cache + stale/error/updatedAt
 * - settings are fetched first (thresholds drive every colour) and refreshed
 * - writes go through `withAuth`, which prompts for an API token when the hub
 *   requires one, stores it in localStorage and retries once on 401 */

import { useEffect, useMemo, useRef, useState } from 'react'
import {
  api, ApiError,
  type Alert, type Device, type Edge, type Health, type Prediction, type Settings,
} from '@/lib/api'
import { createStore, emptyRes, Poller, useStore, type Res, type Store } from '@/lib/store'
import { deviceHealth, SEV_RANK, type DeviceHealth } from '@/lib/severity'
import { hubNow } from '@/lib/format'

/* ═══ resources ═══════════════════════════════════════════════════════ */

export const INTERVALS = {
  alerts: 5_000,
  devices: 5_000,
  health: 10_000,
  predictions: 15_000,
  edges: 30_000,
  settings: 30_000,
} as const

export const pollers = {
  settings: new Poller<Settings>((s) => api.settings({ signal: s }), INTERVALS.settings),
  health: new Poller<Health>((s) => api.health({ signal: s }), INTERVALS.health),
  alerts: new Poller<Alert[]>(async (s) => (await api.alerts({ signal: s })).alerts ?? [], INTERVALS.alerts),
  devices: new Poller<Device[]>(async (s) => (await api.devices({ signal: s })).devices ?? [], INTERVALS.devices),
  edges: new Poller<Edge[]>(async (s) => (await api.edges({ signal: s })).edges ?? [], INTERVALS.edges),
  predictions: new Poller<Prediction[]>(
    async (s) => (await api.predictions({ signal: s })).predictions ?? [], INTERVALS.predictions),
}
export type ResourceKey = keyof typeof pollers

export const useSettings = () => useStore(pollers.settings.store)
export const useHealth = () => useStore(pollers.health.store)
export const useAlerts = () => useStore(pollers.alerts.store)
export const useDevices = () => useStore(pollers.devices.store)
export const useEdges = () => useStore(pollers.edges.store)
export const usePredictions = () => useStore(pollers.predictions.store)

export const refreshAll = () => Promise.all(Object.values(pollers).map((p) => p.refresh()))

/** Start polling on mount; abort everything on unmount. */
export function usePolling() {
  useEffect(() => {
    let cancelled = false
    // Settings first so thresholds are known before anything is coloured
    const first = pollers.settings
    first.start()
    const t = setTimeout(startRest, 2500) // don't wait forever on a slow hub
    const unsub = first.store.subscribe(() => { if (!first.store.get().loading) startRest() })
    function startRest() {
      if (cancelled) return
      clearTimeout(t)
      unsub()
      for (const [k, p] of Object.entries(pollers)) if (k !== 'settings') p.start()
    }
    const onVis = () => { if (document.visibilityState === 'visible') void refreshAll() }
    document.addEventListener('visibilitychange', onVis)
    return () => {
      cancelled = true
      clearTimeout(t)
      unsub()
      document.removeEventListener('visibilitychange', onVis)
      Object.values(pollers).forEach((p) => p.stop())
    }
  }, [])
}

/** View-scoped polled resource (history, metrics, action log…). `key` null = idle. */
export function usePolled<T>(key: string | null, fetcher: (signal: AbortSignal) => Promise<T>, intervalMs: number): Res<T> & { refresh: () => void } {
  const fetchRef = useRef(fetcher)
  fetchRef.current = fetcher
  const [poller, setPoller] = useState<Poller<T> | null>(null)
  useEffect(() => {
    if (key === null) { setPoller(null); return }
    const p = new Poller<T>((s) => fetchRef.current(s), intervalMs)
    setPoller(p)
    p.start()
    const onVis = () => { if (document.visibilityState === 'visible') void p.refresh() }
    document.addEventListener('visibilitychange', onVis)
    return () => { p.stop(); document.removeEventListener('visibilitychange', onVis) }
  }, [key, intervalMs])
  const idle = useMemo(() => createStore<Res<T>>(emptyRes<T>()), [])
  const res = useStore(poller?.store ?? idle)
  return { ...res, refresh: () => void poller?.refresh() }
}

/* ═══ tickers (only the components that print times subscribe) ═════════ */

function ticker(ms: number): Store<number> {
  const s = createStore(hubNow())
  let t: ReturnType<typeof setInterval> | null = null
  let subs = 0
  const base = s.subscribe
  return {
    ...s,
    subscribe(l) {
      const un = base(l)
      if (subs++ === 0) {
        s.set(hubNow())
        t = setInterval(() => s.set(hubNow()), ms)
      }
      return () => {
        un()
        if (--subs === 0 && t) { clearInterval(t); t = null }
      }
    },
  }
}
const secondTicker = ticker(1000)
const coarseTicker = ticker(5000)
/** Hub-clock now, updated every second */
export const useNow = () => useStore(secondTicker)
/** Hub-clock now, updated every 5 s (for freshness math in derived data) */
export const useCoarseNow = () => useStore(coarseTicker)

/* ═══ derived: device rows ════════════════════════════════════════════ */

export interface DeviceRow {
  device: Device
  edge: Edge | undefined
  health: DeviceHealth
}

export function sortRows(rows: DeviceRow[]): DeviceRow[] {
  return [...rows].sort((a, b) => {
    // fresh before stale, then worst severity, then slot, then id
    if (a.health.stale !== b.health.stale) return a.health.stale ? 1 : -1
    const r = SEV_RANK[a.health.sev] - SEV_RANK[b.health.sev]
    if (r) return r
    const sa = a.device.slot ?? 9999
    const sb = b.device.slot ?? 9999
    if (sa !== sb) return sa - sb
    return a.device.device_id.localeCompare(b.device.device_id)
  })
}

/** Every device with its computed health; memoized on data identity + 5 s tick */
export function useDeviceRows(): DeviceRow[] {
  const res = useDevices()
  const devices = res.data
  // the device list itself is not refreshing: nothing in it is live
  const listStale = res.stale
  const edges = useEdges().data
  const settings = useSettings().data
  const now = useCoarseNow()
  return useMemo(() => {
    const byId = new Map((edges ?? []).map((e) => [e.edge_id, e]))
    return sortRows((devices ?? []).map((d) => {
      const edge = byId.get(d.edge_id)
      const health = deviceHealth(d, settings, edge, now)
      if (listStale && !health.stale) {
        health.stale = true
        health.staleReason = 'device list not updating — hub request failing'
      }
      return { device: d, edge, health }
    }))
  }, [devices, edges, settings, now, listStale])
}

/* ═══ edge filter (shared by every view) ══════════════════════════════ */

const LS_EDGE = 'nimon.edgeFilter'
const LS_TOKEN = 'nimon.apiToken'
const LS_THEME = 'nimon.theme'

function lsGet(k: string): string | null {
  try { return localStorage.getItem(k) } catch { return null }
}
function lsSet(k: string, v: string | null) {
  try { if (v === null) localStorage.removeItem(k); else localStorage.setItem(k, v) } catch { /* private mode */ }
}

export const edgeFilterStore = createStore<string | null>(lsGet(LS_EDGE))
edgeFilterStore.subscribe(() => lsSet(LS_EDGE, edgeFilterStore.get()))
export const useEdgeFilter = () => useStore(edgeFilterStore)
export const setEdgeFilter = (id: string | null) => edgeFilterStore.set(id)

/* ═══ theme ═══════════════════════════════════════════════════════════ */

export type ThemePref = 'system' | 'dark' | 'light'
export const themeStore = createStore<ThemePref>((lsGet(LS_THEME) as ThemePref) || 'system')
const mq = typeof matchMedia === 'function' ? matchMedia('(prefers-color-scheme: light)') : null
function applyTheme() {
  const pref = themeStore.get()
  const resolved = pref === 'system' ? (mq?.matches ? 'light' : 'dark') : pref
  document.documentElement.dataset.theme = resolved
  document.documentElement.style.colorScheme = resolved
}
themeStore.subscribe(() => { lsSet(LS_THEME, themeStore.get()); applyTheme() })
mq?.addEventListener?.('change', applyTheme)
applyTheme()
export const useTheme = () => useStore(themeStore)

/* ═══ toasts + screen-reader announcements ════════════════════════════ */

export interface Toast { id: number; kind: 'ok' | 'error' | 'info'; title: string; detail?: string }
export const toastStore = createStore<Toast[]>([])
let toastId = 0
export function toast(kind: Toast['kind'], title: string, detail?: string) {
  const id = ++toastId
  toastStore.set((l) => [...l.slice(-3), { id, kind, title, detail }])
  setTimeout(() => dismissToast(id), kind === 'error' ? 12_000 : 5_000)
}
export const dismissToast = (id: number) => toastStore.set((l) => l.filter((t) => t.id !== id))

export const announceStore = createStore<{ text: string; urgent: boolean; n: number }>({ text: '', urgent: false, n: 0 })
export const announce = (text: string, urgent = false) =>
  announceStore.set((p) => ({ text, urgent, n: p.n + 1 }))

/* ═══ API token + authenticated writes ════════════════════════════════ */

export const tokenStore = createStore<string | null>(lsGet(LS_TOKEN))
tokenStore.subscribe(() => lsSet(LS_TOKEN, tokenStore.get()))
export const useToken = () => useStore(tokenStore)
export const setToken = (t: string | null) => tokenStore.set(t && t.trim() ? t.trim() : null)

export interface TokenPrompt { message: string; resolve: (token: string | null) => void }
export const tokenPromptStore = createStore<TokenPrompt | null>(null)

function promptToken(message: string): Promise<string | null> {
  return new Promise((resolve) => {
    tokenPromptStore.get()?.resolve(null)
    tokenPromptStore.set({
      message,
      resolve: (t) => {
        tokenPromptStore.set(null)
        if (t) setToken(t)
        resolve(t ? t.trim() : null)
      },
    })
  })
}

/** Run a write; prompt for a token when required or on 401, retry once. */
export async function withAuth<T>(fn: (token: string | null) => Promise<T>): Promise<T> {
  const required = pollers.settings.store.get().data?.auth?.writes_require_token ?? false
  let token = tokenStore.get()
  if (required && !token) {
    token = await promptToken('This hub requires an API token for changes (acknowledge, resolve, actions, config).')
    if (!token) throw new ApiError(401, 'Cancelled — an API token is required for this change')
  }
  try {
    return await fn(token)
  } catch (e) {
    if (!(e instanceof ApiError) || e.status !== 401) throw e
    const t = await promptToken(token
      ? 'The hub rejected the stored API token (401 Unauthorized). Enter a valid token.'
      : 'The hub requires an API token for changes (401 Unauthorized).')
    if (!t) throw new ApiError(401, 'Unauthorized — missing or invalid API token')
    try {
      return await fn(t)
    } catch (e2) {
      if (e2 instanceof ApiError && e2.status === 401) {
        throw new ApiError(401, 'Unauthorized — the hub rejected this API token')
      }
      throw e2
    }
  }
}

export const errText = (e: unknown) => (e instanceof Error ? e.message : String(e))

/* ═══ alert actions ═══════════════════════════════════════════════════ */

export async function ackAlert(id: string): Promise<boolean> {
  try {
    await withAuth((t) => api.acknowledge(id, t))
    toast('ok', 'Alert acknowledged')
    return true
  } catch (e) {
    toast('error', 'Acknowledge failed', errText(e))
    return false
  } finally {
    void pollers.alerts.refresh()
  }
}

export async function resolveAlert(id: string): Promise<boolean> {
  try {
    await withAuth((t) => api.resolve(id, t))
    toast('ok', 'Alert resolved')
    return true
  } catch (e) {
    toast('error', 'Resolve failed', errText(e))
    return false
  } finally {
    void pollers.alerts.refresh()
  }
}
