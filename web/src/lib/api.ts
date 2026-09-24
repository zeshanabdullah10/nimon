/* Typed client for the NIMon hub REST API (crates/nimon-hub/src/server).
 * All errors from the hub are `{ "error": string }`. Path segments are
 * always encoded: device ids contain `:` and `#`. */

import { noteServerDate } from '@/lib/format'

export type AlertSeverity = 'critical' | 'warning' | 'info'
export type AlertStatus = 'pending' | 'firing' | 'acknowledged' | 'resolved' | 'suppressed'
export type DeviceStatus = 'healthy' | 'warning' | 'error' | 'offline' | 'unknown' | string
export type MetricValue = number | string | boolean

export interface Health {
  status: 'healthy' | 'degraded'
  version: string
  db_ok: boolean
  alert_manager_ok: boolean
  edges_connected: number
  uptime_secs: number
}

export interface Thresholds {
  temperature_warning: number
  temperature_critical: number
}

export interface EdgeThresholds extends Thresholds {
  poll_interval_secs: number | null
}

export interface Settings {
  version: string
  thresholds: Thresholds
  edge_thresholds: Record<string, EdgeThresholds>
  prediction_alert_threshold: number
  edge_offline_after_secs: number
  auth: { writes_require_token: boolean }
}

/** Core `Alert` (nimon-core/src/alert/mod.rs) as served by /api/v1/alerts */
export interface Alert {
  id: string
  rule_id: string
  edge_id: string
  device_id: string
  severity: AlertSeverity
  status: AlertStatus
  title: string
  message: string
  metric_name: string | null
  metric_value: number | null
  threshold: number | null
  triggered_at: string
  resolved_at: string | null
  fired_count: number
  notification_sent: boolean
  last_fired_at: string | null
  acknowledged_at: string | null
}

/** Row of /api/v1/alerts/history */
export interface AlertRecord {
  id: string
  rule_id: string
  rule_name: string
  device_id: string | null
  edge_id: string | null
  severity: AlertSeverity | string
  status: AlertStatus | string
  title: string
  message: string
  metric_name: string | null
  metric_value: number | null
  threshold: number | null
  fired_count: number
  notification_sent: boolean
  action_taken: string | null
  action_result: string | null
  triggered_at: string
  created_at: string
  last_fired_at: string | null
  acknowledged_at: string | null
  resolved_at: string | null
}

export interface Prediction {
  id: number
  device_id: string
  edge_id: string
  prediction_type: string
  probability: number
  eta_minutes: number | null
  status: string
  created_at: string
}

export interface Edge {
  edge_id: string
  name: string
  hostname: string | null
  ip_address: string | null
  status: 'online' | 'offline'
  live: boolean
  connected_at: string | null
  last_seen: string | null
  device_count: number
  protocol_version: string | null
  version: string | null
  uptime_secs: number | null
}

export interface Device {
  device_id: string
  edge_id: string
  name: string
  device_type: string | null
  model: string | null
  serial_number: string | null
  slot: number | null
  status: DeviceStatus
  metrics: Record<string, MetricValue>
  last_seen: string | null
  is_simulated: boolean
  is_reachable: boolean
  live: boolean
}

export interface MetricPoint {
  timestamp: string
  value: number
}

export interface ActionRecord {
  id: number | string
  alert_id: string | null
  device_id: string | null
  action_id: string
  action_type: string
  command: string | null
  exit_code: number | null
  output: string | null
  duration_ms: number | null
  success: boolean | null
  retry_count: number | null
  executed_at: string
}

export type ManualActionType = 'power_cycle' | 'reset_driver' | 'restart_services' | 'custom_script'

export interface EdgeConfigPush {
  poll_interval_secs?: number
  temperature_warning?: number
  temperature_critical?: number
}

/* ═══ transport ═══════════════════════════════════════════════════════ */

export class ApiError extends Error {
  /** HTTP status; 0 = network failure / hub unreachable */
  status: number
  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
  /** Hub itself is not answering (network error or dev-proxy gateway error) */
  get unreachable() {
    return this.status === 0 || this.status === 502 || this.status === 504
  }
}

export const enc = encodeURIComponent

async function parseError(res: Response): Promise<ApiError> {
  let msg = `${res.status} ${res.statusText}`.trim()
  try {
    const body = await res.json()
    if (body && typeof body.error === 'string') msg = body.error
  } catch { /* non-JSON body */ }
  return new ApiError(res.status, msg)
}

export interface RequestOpts {
  signal?: AbortSignal
  /** Accept these non-2xx statuses as a valid JSON body (e.g. /health 503) */
  acceptStatus?: number[]
}

export async function getJSON<T>(path: string, opts: RequestOpts = {}): Promise<T> {
  let res: Response
  const sentAt = Date.now()
  try {
    res = await fetch(path, { signal: opts.signal, headers: { Accept: 'application/json' }, cache: 'no-store' })
  } catch (e) {
    if ((e as Error).name === 'AbortError') throw e
    throw new ApiError(0, 'Hub unreachable')
  }
  noteServerDate(res.headers.get('Date'), sentAt, Date.now())
  if (!res.ok && !opts.acceptStatus?.includes(res.status)) throw await parseError(res)
  try {
    return (await res.json()) as T
  } catch {
    throw new ApiError(res.status, 'Invalid JSON from hub')
  }
}

export async function postJSON<T>(path: string, body: unknown, token: string | null): Promise<T> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json', Accept: 'application/json' }
  if (token) headers.Authorization = `Bearer ${token}`
  let res: Response
  try {
    res = await fetch(path, { method: 'POST', headers, body: body === undefined ? undefined : JSON.stringify(body) })
  } catch {
    throw new ApiError(0, 'Hub unreachable')
  }
  if (!res.ok) throw await parseError(res)
  try {
    return (await res.json()) as T
  } catch {
    return {} as T
  }
}

function qs(params: Record<string, string | number | undefined | null>): string {
  const p = Object.entries(params)
    .filter(([, v]) => v !== undefined && v !== null && v !== '')
    .map(([k, v]) => `${enc(k)}=${enc(String(v))}`)
  return p.length ? `?${p.join('&')}` : ''
}

/* ═══ endpoints ═══════════════════════════════════════════════════════ */

export interface HistoryQuery {
  severity?: string
  edge_id?: string
  device_id?: string
  status?: string
  since?: string
  until?: string
  limit?: number
}

export const api = {
  health: (o?: RequestOpts) => getJSON<Health>('/health', { ...o, acceptStatus: [503] }),
  settings: (o?: RequestOpts) => getJSON<Settings>('/api/v1/settings', o),
  alerts: (o?: RequestOpts) => getJSON<{ alerts: Alert[]; total: number }>('/api/v1/alerts', o),
  alertHistory: (q: HistoryQuery, o?: RequestOpts) =>
    getJSON<{ alerts: AlertRecord[]; total: number }>(`/api/v1/alerts/history${qs({ ...q })}`, o),
  predictions: (o?: RequestOpts) => getJSON<{ predictions: Prediction[]; total: number }>('/api/v1/predictions', o),
  edges: (o?: RequestOpts) => getJSON<{ edges: Edge[]; total: number }>('/api/v1/edges', o),
  devices: (o?: RequestOpts) => getJSON<{ devices: Device[]; total: number }>('/api/v1/devices', o),
  metrics: (deviceId: string, q: { metric?: string; since?: string; until?: string; limit?: number }, o?: RequestOpts) =>
    getJSON<{ device_id: string; metric: string; points: MetricPoint[] }>(
      `/api/v1/devices/${enc(deviceId)}/metrics${qs({ metric: 'temperature', ...q })}`, o),
  actions: (q: { device_id?: string; edge_id?: string; limit?: number }, o?: RequestOpts) =>
    getJSON<{ actions: ActionRecord[]; total: number }>(`/api/v1/actions${qs(q)}`, o),

  acknowledge: (id: string, token: string | null) =>
    postJSON<{ status: 'acknowledged' }>(`/api/v1/alerts/${enc(id)}/acknowledge`, undefined, token),
  resolve: (id: string, token: string | null) =>
    postJSON<{ status: 'resolved' }>(`/api/v1/alerts/${enc(id)}/resolve`, undefined, token),
  deviceAction: (deviceId: string, action_type: ManualActionType, parameters: Record<string, string>, token: string | null) =>
    postJSON<{ action_id: string }>(`/api/v1/devices/${enc(deviceId)}/actions`, { action_type, parameters }, token),
  pushEdgeConfig: (edgeId: string, body: EdgeConfigPush, token: string | null) =>
    postJSON<{ status: 'pushed' | 'stored' }>(`/api/v1/edges/${enc(edgeId)}/config`, body, token),
}
