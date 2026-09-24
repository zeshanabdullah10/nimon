import { useMemo } from 'react'
import { ChevronRight } from 'lucide-react'
import {
  useAlerts, useDeviceRows, useDevices, useEdgeFilter, useEdges, usePredictions, useSettings, type DeviceRow,
} from '@/hooks/nimon'
import { href } from '@/hooks/route'
import { EdgeChips } from '@/components/EdgeChips'
import { DeviceList } from '@/components/DeviceList'
import { StationHeadline, useStation } from '@/components/station'
import { Ago, Card, Empty, Meter, SevBadge, SevIcon, Updated } from '@/components/ui'
import { deviceLabel, gb, humanize, pct, parseTs } from '@/lib/format'
import { alertSev, numMetric, type Sev } from '@/lib/severity'

export function Overview() {
  const status = useStation(true)
  const filter = useEdgeFilter()
  const edges = useEdges().data ?? []
  const filteredName = filter ? (edges.find((e) => e.edge_id === filter)?.name ?? filter) : null
  return (
    <div className="flex flex-col gap-3.5">
      <EdgeChips />
      {filteredName && <p className="-mb-1 text-xs text-t2">Showing edge <strong className="text-t1">{filteredName}</strong> only.</p>}
      <StationHeadline status={status} />
      <div className="grid grid-cols-1 gap-3.5 xl:grid-cols-[minmax(0,1fr)_340px]">
        <ModulesCard />
        <div className="flex min-w-0 flex-col gap-3.5">
          <ActiveAlertsCard />
          <PredictionsCard />
          <ControllerCard />
        </div>
      </div>
    </div>
  )
}

export function useScopedRows(): DeviceRow[] {
  const rows = useDeviceRows()
  const filter = useEdgeFilter()
  return useMemo(() => (filter ? rows.filter((r) => r.device.edge_id === filter) : rows), [rows, filter])
}

function ModulesCard() {
  const rows = useScopedRows()
  const devices = useDevices()
  const multiEdge = (useEdges().data?.length ?? 0) > 1
  const { live, stale, hottest } = useMemo(() => {
    const fresh = rows.filter((r) => !r.health.stale)
    let hottest: DeviceRow | null = null
    for (const r of fresh) if (r.health.temp !== null && (!hottest || r.health.temp > hottest.health.temp!)) hottest = r
    return { live: fresh.length, stale: rows.length - fresh.length, hottest }
  }, [rows])

  return (
    <Card title={`Modules · ${rows.length}`}
      actions={<Updated at={devices.updatedAt} stale={devices.stale} error={devices.error?.message} />}>
      <div className="mb-3 flex flex-wrap gap-x-6 gap-y-2 text-[13px]">
        <span><strong className="text-lg">{live}</strong> <span className="text-t2">reporting live</span></span>
        <span><strong className="text-lg">{stale}</strong> <span className="text-t2">not reporting</span></span>
        <span className="min-w-0">
          <span className="text-t2">Hottest module now: </span>
          {hottest ? (
            <a className="font-semibold underline-offset-2 hover:underline" href={href({ device: hottest.device.device_id })}>
              {hottest.health.temp!.toFixed(1)} °C · {deviceLabel(hottest.device.device_id, hottest.device.name).name}
            </a>
          ) : <span>—</span>}
        </span>
      </div>
      {devices.loading ? <Empty>Loading modules…</Empty>
        : rows.length === 0 ? <Empty>{devices.error ? `Modules unavailable: ${devices.error.message}` : 'No modules reported yet'}</Empty>
          : <DeviceList rows={rows} showEdge={multiEdge} />}
    </Card>
  )
}

function ActiveAlertsCard() {
  const alerts = useAlerts()
  const filter = useEdgeFilter()
  const list = useMemo(() => (alerts.data ?? []).filter((a) => !filter || a.edge_id === filter), [alerts.data, filter])
  const devices = useDevices().data
  const names = useMemo(() => new Map((devices ?? []).map((d) => [d.device_id, d])), [devices])
  return (
    <Card title={`Active alerts · ${alerts.data ? list.length : '—'}`}
      actions={<a className="btn btn-sm" href={href({ view: 'alerts', sub: null, device: null })}>Triage <ChevronRight aria-hidden className="h-3.5 w-3.5" /></a>}>
      {alerts.error && <p className="mb-2 rounded-md border border-warn/40 bg-warn/10 px-2 py-1.5 text-xs text-t1">Alerts unavailable — {alerts.error.message}. {alerts.data ? 'Showing last known list.' : ''}</p>}
      {alerts.loading ? <Empty>Loading…</Empty> : list.length === 0 ? <Empty>{alerts.data ? 'No active alerts' : 'No alert data'}</Empty> : (
        <ul className="flex flex-col gap-1">
          {list.slice(0, 6).map((a) => {
            const d = names.get(a.device_id)
            const sev: Sev = alertSev(a.severity)
            return (
              <li key={a.id}>
                <a href={href({ view: 'alerts', sub: null, device: null })} className="row flex items-center gap-2.5 px-2.5 py-2 hover:bg-fg/[0.07]">
                  <SevIcon sev={sev} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[13px] font-medium">{a.title || a.rule_id}</span>
                    <span className="block truncate text-xs text-t2">
                      {d ? deviceLabel(d.device_id, d.name).name : a.device_id}{d?.slot != null ? ` · slot ${d.slot}` : ''}
                      {a.status === 'acknowledged' ? ' · acknowledged' : ''}
                    </span>
                  </span>
                  <span className="shrink-0 text-xs text-t2"><Ago ms={parseTs(a.triggered_at)} /></span>
                </a>
              </li>
            )
          })}
          {list.length > 6 && <li className="pt-1 text-center text-xs text-t2">+{list.length - 6} more in Alerts</li>}
        </ul>
      )}
      <div className="mt-2"><Updated at={alerts.updatedAt} stale={alerts.stale} error={alerts.error?.message} /></div>
    </Card>
  )
}

function PredictionsCard() {
  const preds = usePredictions()
  const threshold = useSettings().data?.prediction_alert_threshold ?? null
  const filter = useEdgeFilter()
  const list = useMemo(() => [...(preds.data ?? [])]
    .filter((p) => !filter || p.edge_id === filter)
    .sort((a, b) => b.probability - a.probability), [preds.data, filter])
  const above = threshold === null ? [] : list.filter((p) => p.probability >= threshold)
  const top = list[0]
  return (
    <Card title="Predictions"
      actions={<a className="btn btn-sm" href={href({ view: 'predictions', sub: null, device: null })}>All <ChevronRight aria-hidden className="h-3.5 w-3.5" /></a>}>
      <p className="text-[13px]">
        <strong>{above.length}</strong> above alert threshold ({threshold === null ? '—' : pct(threshold)}) · {list.length} active
      </p>
      {top && (
        <p className="mt-1 truncate text-xs text-t2">
          Highest: {humanize(top.prediction_type)} on {deviceLabel(top.device_id).name} · {pct(top.probability)}{top.eta_minutes ? ` · ETA ~${top.eta_minutes} min` : ''}
        </p>
      )}
      <div className="mt-2"><Updated at={preds.updatedAt} stale={preds.stale} error={preds.error?.message} /></div>
    </Card>
  )
}

function ControllerCard() {
  const rows = useScopedRows()
  const controllers = rows.filter((r) => numMetric(r.device.metrics?.mem_total_mb) !== null || numMetric(r.device.metrics?.disk_total_mb) !== null)
  return (
    <Card title="Controller resources">
      {controllers.length === 0 ? <Empty>No controller reports memory or disk</Empty> : (
        <div className="flex flex-col gap-3">
          {controllers.map((r) => {
            const m = r.device.metrics
            const memT = numMetric(m.mem_total_mb), memF = numMetric(m.mem_free_mb)
            const dT = numMetric(m.disk_total_mb), dF = numMetric(m.disk_free_mb)
            const memUsed = memT && memF !== null ? ((memT - memF) / memT) * 100 : null
            const diskUsed = dT && dF !== null ? ((dT - dF) / dT) * 100 : null
            const s = (u: number | null): Sev => (u === null ? 'unknown' : u >= 90 ? 'critical' : u >= 75 ? 'warning' : 'ok')
            return (
              <div key={r.device.device_id}>
                <div className="mb-1.5 flex items-center justify-between gap-2 text-xs">
                  <span className="truncate font-medium">{r.edge?.name || r.device.edge_id} · {deviceLabel(r.device.device_id, r.device.name).name}</span>
                  {r.health.stale && <SevBadge sev="unknown" stale />}
                </div>
                <Res label="RAM" used={memUsed} sev={s(memUsed)} text={memF === null ? '—' : `${gb(memF)} free of ${gb(memT)}`} />
                <Res label="Disk" used={diskUsed} sev={s(diskUsed)} text={dF === null ? '—' : `${gb(dF)} free of ${gb(dT)}`} />
              </div>
            )
          })}
        </div>
      )}
    </Card>
  )
}

function Res({ label, used, sev, text }: { label: string; used: number | null; sev: Sev; text: string }) {
  return (
    <div className="mb-1.5 grid grid-cols-[40px_minmax(0,1fr)] items-center gap-x-2 text-xs">
      <span className="text-t2">{label}</span>
      <Meter value={used} label={`${label} used`} sev={sev} />
      <span />
      <span className="text-t2">{used === null ? '—' : `${Math.round(used)}% used`} · {text}</span>
    </div>
  )
}
