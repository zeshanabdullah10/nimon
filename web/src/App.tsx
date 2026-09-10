import { useMemo, useState } from 'react'
import { TitleBar } from '@/components/TitleBar'
import { NavRail, type ViewId } from '@/components/NavRail'
import { ModulesPanel } from '@/components/ModulesPanel'
import { Donut } from '@/components/Donut'
import {
  PredictionsList, PredictionCard, RecentAlerts, ResourceRings, StationHealth,
} from '@/components/RightColumn'
import { Card, CardContent, CardTitle } from '@/components/ui/card'
import { useNimon, num, relTime } from '@/hooks/nimon'
import { cn } from '@/lib/utils'

export default function App() {
  const { data, connected, busy, refresh, ack, smoothed } = useNimon()
  const [view, setView] = useState<ViewId>('overview')
  const [ackPending, setAckPending] = useState<Set<number>>(new Set())

  const up = connected !== false

  /* smoothed temperatures per device */
  const temps = useMemo(() => {
    const out: Record<string, number> = {}
    for (const d of data.devices) {
      const raw = num(d.metrics?.temperature)
      if (raw !== null && d.metrics?.is_reachable !== false) {
        out[d.device_id] = smoothed(d.device_id, raw)
      }
    }
    return out
  }, [data.devices, smoothed])

  const station = data.devices.find((d) => num(d.metrics?.mem_total_mb) !== null)
  const online = data.devices.filter((d) => ['healthy', 'warning'].includes(d.status)).length
  const offline = data.devices.length - online

  const onAck = async (id: number) => {
    setAckPending((s) => new Set(s).add(id))
    await ack(id)
    setAckPending((s) => { const n = new Set(s); n.delete(id); return n })
  }

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-window">
      <TitleBar up={up} busy={busy} edges={data.edges.length} onRefresh={refresh} />

      <div className="flex min-h-0 flex-1">
        <NavRail view={view} alertCount={data.alerts.length} onSelect={setView} />

        <div className="min-w-0 flex-1 overflow-y-auto p-3.5">
          {view === 'overview' && (
            <div className="grid grid-cols-1 gap-3.5 lg:h-full lg:min-h-0 lg:grid-cols-[minmax(0,1fr)_260px]">
              <ModulesPanel
                devices={data.devices}
                station={station}
                temps={temps}
                alertCount={data.alerts.length}
                predCount={data.predictions.length}
                up={up}
              />
              <div className="flex flex-col gap-2.5 lg:h-full lg:min-h-0">
                <StationHealth online={online} offline={offline} alerts={data.alerts.length} />
                <ResourceRings station={station} />
                <PredictionCard predictions={data.predictions} onOpen={() => setView('predictions')} />
                <RecentAlerts alerts={data.alerts} onAck={onAck} ackPending={ackPending} />
              </div>
            </div>
          )}

          {view === 'modules' && (
            <div className="h-full min-h-0">
              <ModulesPanel wide devices={data.devices} station={station} temps={temps}
                alertCount={data.alerts.length} predCount={data.predictions.length} />
            </div>
          )}

          {view === 'alerts' && (
            <Card>
              <CardContent className="pb-4 pt-4">
                <CardTitle>Active Alerts — {data.alerts.length}</CardTitle>
                {data.alerts.length === 0 ? (
                  <div className="py-8 text-center text-[13px] text-t3">No active alerts</div>
                ) : (
                  <div className="flex flex-col gap-2">
                    {[...data.alerts]
                      .sort((a, b) =>
                        ((a.severity || '').toLowerCase() === 'critical' ? -1 : 1) -
                        ((b.severity || '').toLowerCase() === 'critical' ? -1 : 1))
                      .map((a) => {
                        const crit = (a.severity || '').toLowerCase() === 'critical'
                        const dev = a.device_id ? a.device_id.slice(a.device_id.indexOf(':') + 1) : ''
                        const pending = ackPending.has(a.id)
                        return (
                          <div key={a.id} className="flex items-start gap-3.5 rounded-lg bg-row p-4">
                            <span className={cn('mt-[7px] h-[9px] w-[9px] shrink-0 rounded-full',
                              crit ? 'bg-crit' : 'bg-warn')} />
                            <div className="min-w-0 flex-1">
                              <div className="text-[15px] font-medium">{a.rule_name || 'Alert'}</div>
                              <div className="mt-1 break-words text-[13.5px] leading-relaxed text-t2">
                                {a.message || ''}
                              </div>
                              <div className="mt-2 font-mono text-xs text-t3">
                                {dev} · {relTime(a.created_at)}
                              </div>
                            </div>
                            <button
                              onClick={() => onAck(a.id)}
                              disabled={pending}
                              className="flex h-9 shrink-0 items-center gap-1.5 rounded-full border border-line px-4 text-[13px] font-medium text-t2 transition-colors hover:border-ok/45 hover:text-ok disabled:opacity-40"
                            >
                              {pending ? '···' : 'Acknowledge'}
                            </button>
                          </div>
                        )
                      })}
                  </div>
                )}
              </CardContent>
            </Card>
          )}

          {view === 'predictions' && (
            <Card>
              <CardContent className="pb-4 pt-4">
                <CardTitle>Predictions — {data.predictions.length}</CardTitle>
                <PredictionsList predictions={data.predictions} />
              </CardContent>
            </Card>
          )}

          {view === 'resources' && (
            <div className="grid grid-cols-[repeat(auto-fit,minmax(260px,1fr))] gap-3.5">
              <ResourcesBig label="Memory"
                pct={station && num(station.metrics.mem_total_mb)
                  ? (num(station.metrics.mem_free_mb)! / num(station.metrics.mem_total_mb)!) * 100
                  : null}
                value={station ? `${(num(station.metrics.mem_free_mb)! / 1024).toFixed(1)} GB` : '—'}
                caption={station ? `of ${(num(station.metrics.mem_total_mb)! / 1024).toFixed(1)} GB physical` : ''} />
              <ResourcesBig label="Primary Disk"
                pct={station && num(station.metrics.disk_total_mb)
                  ? (num(station.metrics.disk_free_mb)! / num(station.metrics.disk_total_mb)!) * 100
                  : null}
                value={station ? `${(num(station.metrics.disk_free_mb)! / 1024).toFixed(1)} GB` : '—'}
                caption={station ? `of ${(num(station.metrics.disk_total_mb)! / 1024).toFixed(1)} GB` : ''} />
              <Card className="self-start">
                <CardContent className="pb-4 pt-4">
                  <CardTitle>Station</CardTitle>
                  <div className="mt-3">
                    {[
                      ['Host', station ? station.device_id.slice(station.device_id.indexOf(':') + 1) : '—'],
                      ['Product', typeof station?.metrics.product === 'string' ? station.metrics.product : '—'],
                      ['Modules', String(data.devices.length)],
                      ['Edges', String(data.edges.length)],
                    ].map(([k, v]) => (
                      <div key={k} className="flex justify-between border-b border-linecard py-2 text-[13px] last:border-0">
                        <span className="text-t3">{k}</span>
                        <span className="font-mono text-xs">{v}</span>
                      </div>
                    ))}
                  </div>
                </CardContent>
              </Card>
            </div>
          )}

          {view === 'settings' && (
            <Card className="max-w-[620px]">
              <CardContent className="pb-4 pt-4">
                <CardTitle>Settings</CardTitle>
                <div className="mt-3">
                  {[
                    ['Hub endpoint', location.host || '—'],
                    ['Dashboard poll', '5 s'],
                    ['Edge sweep', '30 s'],
                    ['Connected edges', String(data.edges.length)],
                    ['Hub state', up ? 'online' : 'offline'],
                  ].map(([k, v]) => (
                    <div key={k} className="flex justify-between border-b border-linecard py-3 text-[13px] last:border-0">
                      <span className="text-t2">{k}</span>
                      <span className="font-mono text-xs">{v}</span>
                    </div>
                  ))}
                </div>
                <a href="/api/v1/status"
                  className="mt-3.5 block rounded-md bg-white/5 py-2 text-center text-[13px] text-ok hover:bg-white/10">
                  Raw API — /api/v1/status
                </a>
              </CardContent>
            </Card>
          )}
        </div>
      </div>
    </div>
  )
}

function ResourcesBig({ label, pct, value, caption }: {
  label: string
  pct: number | null
  value: string
  caption: string
}) {
  return (
    <Card className="flex flex-col items-center pb-5 pt-4">
      <CardContent className="flex flex-col items-center">
        <CardTitle>{label}</CardTitle>
        <div className="mt-4">
          <Donut pct={pct} size={150} stroke={6} />
        </div>
        <div className="mt-4 text-[28px] font-bold">{value}</div>
        <div className="mt-1 text-[13px] text-t2">{caption}</div>
      </CardContent>
    </Card>
  )
}
