import { useCallback, useEffect, useRef, useState } from 'react'

/* ═══ types ═════════════════════════════════════════════════════════ */

export type Severity = 'ok' | 'warn' | 'crit' | 'idle'

export interface Edge {
  edge_id: string
  name: string
  connected_at?: string
  device_count?: number
}

export interface Device {
  device_id: string
  status: 'healthy' | 'warning' | 'error' | 'offline' | string
  last_seen: string
  metrics: Record<string, number | boolean | string>
}

export interface Alert {
  id: number
  rule_name: string
  severity: 'critical' | 'warning' | string
  message: string
  device_id?: string
  created_at: string
}

export interface Prediction {
  id: number
  prediction_type: string
  probability: number
  eta_minutes?: number
  device_id?: string
  created_at: string
}

export interface NimonData {
  up: boolean
  edges: Edge[]
  devices: Device[]
  alerts: Alert[]
  predictions: Prediction[]
}

/* ═══ severity: ONE ramp used everywhere ═════════════════════════════ */

export const T_MAX = 85

/* temp: headroom > 20°C → ok | 5–20 → warn | < 5 → crit */
export function sevTemp(tempC: number | null | undefined): Severity {
  if (tempC === null || tempC === undefined) return 'idle'
  const headroom = T_MAX - tempC
  if (headroom < 5) return 'crit'
  if (headroom <= 20) return 'warn'
  return 'ok'
}

/* percent: < 60 ok | 60–84 warn | ≥ 85 crit */
export function sevPct(pct: number | null | undefined): Severity {
  if (pct === null || pct === undefined) return 'idle'
  if (pct >= 85) return 'crit'
  if (pct >= 60) return 'warn'
  return 'ok'
}

export const SEV_RANK: Record<Severity, number> = { crit: 0, warn: 1, ok: 2, idle: 3 }

export const SEV_COLOR: Record<Severity, string> = {
  ok: '#2FD463',
  warn: '#E8B437',
  crit: '#E5484D',
  idle: 'rgba(255,255,255,0.28)',
}

/* ═══ helpers ════════════════════════════════════════════════════════ */

export function parseTs(ts?: string): number | null {
  if (!ts) return null
  let s = String(ts).trim().replace(' ', 'T')
  if (!/[zZ]$/.test(s) && !/[+-]\d{2}:?\d{2}$/.test(s)) s += 'Z'
  const t = Date.parse(s)
  return Number.isNaN(t) ? null : t
}

export function relTime(ts?: string): string {
  const t = parseTs(ts)
  if (t === null) return ''
  const s = Math.max(0, Math.round((Date.now() - t) / 1000))
  if (s < 60) return `${s}s ago`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ago`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h ago`
  return `${Math.floor(h / 24)}d ago`
}

export function deviceTitle(id: string): { name: string; tag: string } {
  const i = id.indexOf(':')
  const short = i === -1 ? id : id.slice(i + 1)
  const h = short.lastIndexOf('#')
  return {
    name: (h === -1 ? short : short.slice(0, h)).trim(),
    tag: h === -1 ? '' : short.slice(h + 1),
  }
}

export const initials = (name: string) =>
  (name.split(/\s+/).map((w) => w[0]).join('') || '?').slice(0, 2).toUpperCase()

export const gb = (mb?: number | null) =>
  mb === null || mb === undefined ? '—' : `${(mb / 1024).toFixed(1)} GB`

export const num = (v: unknown): number | null =>
  typeof v === 'number' && Number.isFinite(v) ? v : null

/* ═══ hook: live hub data ════════════════════════════════════════════ */

const POLL_MS = 5000
const EMA_ALPHA = 0.3

async function getJSON<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, init)
  if (!res.ok) throw new Error(`${url} → ${res.status}`)
  return res.json()
}

export function useNimon() {
  const [data, setData] = useState<NimonData>({
    up: false, edges: [], devices: [], alerts: [], predictions: [],
  })
  const [connected, setConnected] = useState<boolean | null>(null)
  const [busy, setBusy] = useState(false)
  const ema = useRef<Record<string, number>>({})
  const [, setTick] = useState(0) // 30s relative-time refresh

  const refresh = useCallback(async () => {
    setBusy(true)
    try {
      const edges = await getJSON<{ edges: Edge[] }>('/api/v1/edges')
      const list = edges.edges ?? []
      const devices: Device[] = []
      await Promise.all(
        list.map(async (e) => {
          try {
            const d = await getJSON<{ devices: Device[] }>(
              `/api/v1/edges/${encodeURIComponent(e.edge_id)}/devices`,
            )
            devices.push(...(d.devices ?? []))
          } catch { /* edge unreachable: keep what we have */ }
        }),
      )
      const [alerts, preds] = await Promise.all([
        getJSON<{ alerts: Alert[] }>('/api/v1/alerts').catch(() => ({ alerts: [] })),
        getJSON<{ predictions: Prediction[] }>('/api/v1/predictions').catch(() => ({ predictions: [] })),
      ])
      setConnected(true)
      setData({
        up: true,
        edges: list,
        devices,
        alerts: (alerts.alerts ?? []) as Alert[],
        predictions: (preds.predictions ?? []) as Prediction[],
      })
    } catch {
      setConnected(false)
      setData((d) => ({ ...d, up: false }))
    } finally {
      setBusy(false)
    }
  }, [])

  /* smoothed display temperature — no flickering tenths */
  const smoothed = useCallback((id: string, raw: number): number => {
    const prev = ema.current[id]
    const next = prev === undefined ? raw : prev + EMA_ALPHA * (raw - prev)
    ema.current[id] = next
    return next
  }, [])

  const ack = useCallback(
    async (id: number) => {
      await getJSON(`/api/v1/alerts/${encodeURIComponent(id)}/acknowledge`, {
        method: 'POST',
      }).catch(() => null)
      await refresh()
    },
    [refresh],
  )

  useEffect(() => {
    refresh()
    const poll = setInterval(refresh, POLL_MS)
    const tick = setInterval(() => setTick((t) => t + 1), 30000)
    return () => { clearInterval(poll); clearInterval(tick) }
  }, [refresh])

  return { data, connected, busy, refresh, ack, smoothed }
}
