/* The one set of severity rules used by every view (see README of web/).
 *
 *  temperature: ≥ critical → critical, ≥ warning → warning, else ok
 *  device:      worse of mapped device.status and temperature severity
 *               (error→critical, warning→warning, offline→offline, healthy→ok)
 *  is_reachable=false → "no link" (unless the edge reports an error)
 *  live=false / edge not live / last_seen older than edge_offline_after_secs
 *               → stale: greyed, "last known at T", never counted as live. */

import type { Device, Edge, Settings, Thresholds } from '@/lib/api'
import { parseTs } from '@/lib/format'

export type Sev = 'critical' | 'warning' | 'nolink' | 'offline' | 'unknown' | 'ok'

/** Lower = worse */
export const SEV_RANK: Record<Sev, number> = {
  critical: 0, warning: 1, nolink: 2, offline: 3, unknown: 4, ok: 5,
}

export const SEV_LABEL: Record<Sev, string> = {
  critical: 'Critical',
  warning: 'Warning',
  nolink: 'No link',
  offline: 'Offline',
  unknown: 'Unknown',
  ok: 'OK',
}

export const worse = (a: Sev, b: Sev): Sev => (SEV_RANK[a] <= SEV_RANK[b] ? a : b)

export const DEFAULT_THRESHOLDS: Thresholds = { temperature_warning: 65, temperature_critical: 75 }

/** Effective thresholds for an edge: per-edge override, else hub defaults */
export function thresholdsFor(settings: Settings | null, edgeId: string | null | undefined): Thresholds {
  if (!settings) return DEFAULT_THRESHOLDS
  const e = edgeId ? settings.edge_thresholds?.[edgeId] : undefined
  return e ?? settings.thresholds ?? DEFAULT_THRESHOLDS
}

export function tempSeverity(temp: number | null, t: Thresholds): Sev {
  if (temp === null) return 'unknown'
  if (temp >= t.temperature_critical) return 'critical'
  if (temp >= t.temperature_warning) return 'warning'
  return 'ok'
}

export function statusSeverity(status: string | null | undefined): Sev {
  switch ((status ?? '').toLowerCase()) {
    case 'error':
    case 'critical':
      return 'critical'
    case 'warning':
      return 'warning'
    case 'offline':
      return 'offline'
    case 'healthy':
    case 'ok':
      return 'ok'
    default:
      return 'unknown'
  }
}

export const numMetric = (v: unknown): number | null =>
  typeof v === 'number' && Number.isFinite(v) ? v : null

export interface DeviceHealth {
  sev: Sev
  statusSev: Sev
  tempSev: Sev
  /** Latest reported temperature (null when unreachable or not reported) */
  temp: number | null
  thresholds: Thresholds
  stale: boolean
  /** Human reason for the stale flag */
  staleReason: string | null
  lastSeenMs: number | null
}

export function deviceHealth(
  d: Device,
  settings: Settings | null,
  edge: Edge | undefined,
  now: number,
): DeviceHealth {
  const thresholds = thresholdsFor(settings, d.edge_id)
  const statusSev = statusSeverity(d.status)
  const reachable = d.is_reachable !== false
  const rawTemp = numMetric(d.metrics?.temperature)
  const temp = reachable ? rawTemp : null
  const tempSev = tempSeverity(temp, thresholds)

  let sev: Sev
  if (!reachable) sev = statusSev === 'critical' ? 'critical' : 'nolink'
  else if (statusSev === 'unknown') sev = tempSev
  else if (tempSev === 'unknown') sev = statusSev
  else sev = worse(statusSev, tempSev)

  const lastSeenMs = parseTs(d.last_seen)
  const offlineAfter = (settings?.edge_offline_after_secs ?? 90) * 1000
  let staleReason: string | null = null
  if (!d.live) staleReason = 'not reported by a connected edge'
  else if (edge && !edge.live) staleReason = 'edge offline'
  else if (lastSeenMs !== null && now - lastSeenMs > offlineAfter) staleReason = 'no update within offline window'
  else if (lastSeenMs === null) staleReason = 'never reported'

  return { sev, statusSev, tempSev, temp, thresholds, stale: staleReason !== null, staleReason, lastSeenMs }
}

/* ═══ station headline ════════════════════════════════════════════════ */

export type Headline = 'OK' | 'WARN' | 'CRIT' | 'STALE' | 'HUB DOWN' | 'LOADING'

export const HEADLINE_RANK: Record<Headline, number> = {
  'HUB DOWN': 0, CRIT: 1, WARN: 2, STALE: 3, LOADING: 4, OK: 5,
}

export function alertSev(s: string | null | undefined): Sev {
  const v = (s ?? '').toLowerCase()
  if (v === 'critical') return 'critical'
  if (v === 'warning') return 'warning'
  return 'unknown'
}
