/* Station headline: OK / WARN / CRIT / STALE / HUB DOWN, with reasons. */

import type { Alert, Edge, Health } from '@/lib/api'
import type { Res } from '@/lib/store'
import { HEADLINE_RANK, type Headline } from '@/lib/severity'
import type { DeviceRow } from '@/hooks/nimon'

export interface Reason { level: Headline; text: string }
export interface StationStatus {
  level: Headline
  reasons: Reason[]
  counts: { critical: number; warning: number; nolink: number; offline: number; ok: number; unknown: number; stale: number; total: number }
}

export function stationStatus(
  health: Res<Health>,
  devices: Res<unknown>,
  alerts: Res<Alert[]>,
  edges: Res<Edge[]>,
  rows: DeviceRow[],
  edgeFilter: string | null,
): StationStatus {
  const counts = { critical: 0, warning: 0, nolink: 0, offline: 0, ok: 0, unknown: 0, stale: 0, total: 0 }
  const reasons: Reason[] = []
  const add = (level: Headline, text: string) => reasons.push({ level, text })

  const scoped = edgeFilter ? rows.filter((r) => r.device.edge_id === edgeFilter) : rows
  for (const r of scoped) {
    counts.total++
    if (r.health.stale) counts.stale++
    else counts[r.health.sev]++
  }

  const hubDown =
    (health.error?.unreachable ?? false) &&
    (devices.error ? devices.error.unreachable : devices.loading)
  if (hubDown) {
    add('HUB DOWN', 'The hub is not responding — every value shown is last known.')
    return { level: 'HUB DOWN', reasons, counts }
  }
  if (devices.loading) return { level: 'LOADING', reasons, counts }

  const h = health.data
  if (h && h.status === 'degraded') {
    const parts = [!h.db_ok && 'database', !h.alert_manager_ok && 'alert manager'].filter(Boolean).join(' and ')
    add('WARN', `Hub degraded: ${parts || 'a component'} unavailable.`)
  }
  if (alerts.error) add('STALE', `Alerts unavailable (${alerts.error.message}) — alert list may be out of date.`)
  if (devices.error) add('STALE', `Device list unavailable (${devices.error.message}) — showing last known values.`)

  if (counts.critical) add('CRIT', `${counts.critical} module${counts.critical > 1 ? 's' : ''} critical`)
  if (counts.warning) add('WARN', `${counts.warning} module${counts.warning > 1 ? 's' : ''} in warning`)
  if (counts.nolink) add('WARN', `${counts.nolink} module${counts.nolink > 1 ? 's' : ''} with no link`)
  if (counts.offline) add('WARN', `${counts.offline} module${counts.offline > 1 ? 's' : ''} offline`)
  if (counts.stale) add('STALE', `${counts.stale} module${counts.stale > 1 ? 's' : ''} not reporting (last known values)`)

  const scopedEdges = (edges.data ?? []).filter((e) => !edgeFilter || e.edge_id === edgeFilter)
  const offlineEdges = scopedEdges.filter((e) => !e.live)
  if (offlineEdges.length) {
    add('STALE', `Edge offline: ${offlineEdges.map((e) => e.name || e.edge_id).join(', ')}`)
  }

  const active = (alerts.data ?? []).filter((a) => !edgeFilter || a.edge_id === edgeFilter)
  const firingCrit = active.filter((a) => a.severity === 'critical' && a.status !== 'acknowledged').length
  const firingWarn = active.filter((a) => a.severity === 'warning' && a.status !== 'acknowledged').length
  const acked = active.filter((a) => a.status === 'acknowledged').length
  if (firingCrit) add('CRIT', `${firingCrit} critical alert${firingCrit > 1 ? 's' : ''} firing`)
  if (firingWarn) add('WARN', `${firingWarn} warning alert${firingWarn > 1 ? 's' : ''} firing`)
  if (acked) add('WARN', `${acked} acknowledged alert${acked > 1 ? 's' : ''} still active`)

  if (!counts.total && !scopedEdges.length && !devices.loading) {
    add('STALE', 'No edge has reported to this hub yet.')
  }

  let level: Headline = 'OK'
  for (const r of reasons) if (HEADLINE_RANK[r.level] < HEADLINE_RANK[level]) level = r.level
  reasons.sort((a, b) => HEADLINE_RANK[a.level] - HEADLINE_RANK[b.level])
  return { level, reasons, counts }
}
