import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardTitle } from '@/components/ui/card'
import { Donut } from '@/components/Donut'
import { cn } from '@/lib/utils'
import {
  gb, num, relTime, SEV_COLOR, type Alert, type Device, type Prediction,
} from '@/hooks/nimon'

export function StationHealth({
  online, offline, alerts,
}: { online: number; offline: number; alerts: number }) {
  return (
    <Card>
      <CardContent className="pb-4 pt-4">
        <CardTitle>Station Health</CardTitle>
        <div className="grid grid-cols-3">
          <div className="flex flex-col items-center gap-0.5">
            <span className="text-[32px] font-bold leading-[1.15] text-ok">{online}</span>
            <span className="text-xs font-medium text-t2">Online</span>
          </div>
          <div className="flex flex-col items-center gap-0.5">
            <span className="text-[32px] font-bold leading-[1.15]">{offline}</span>
            <span className="text-xs font-medium text-t2">Offline</span>
          </div>
          <div className="flex flex-col items-center gap-0.5">
            <span className="text-[32px] font-bold leading-[1.15] text-crit">{alerts}</span>
            <span className="text-xs font-medium text-t2">Alerts</span>
          </div>
        </div>
      </CardContent>
    </Card>
  )
}

export function ResourceRings({ station }: { station?: Device }) {
  const m = station?.metrics
  const ramPct = m && num(m.mem_total_mb) ? (num(m.mem_free_mb)! / num(m.mem_total_mb)!) * 100 : null
  const diskPct = m && num(m.disk_total_mb) ? (num(m.disk_free_mb)! / num(m.disk_total_mb)!) * 100 : null

  return (
    <Card>
      <CardContent className="pb-4 pt-4">
        <CardTitle>Resources</CardTitle>
        <div className="flex justify-around gap-5">
          <RingUnit pct={ramPct} unit="RAM"
            caption={m && num(m.mem_total_mb)
              ? `${gb(num(m.mem_free_mb))} of ${gb(num(m.mem_total_mb))}` : '—'} />
          <RingUnit pct={diskPct} unit="Disk"
            caption={m && num(m.disk_total_mb)
              ? `${gb(num(m.disk_free_mb))} of ${gb(num(m.disk_total_mb))}` : '—'} />
        </div>
      </CardContent>
    </Card>
  )
}

function RingUnit({ pct, unit, caption }: { pct: number | null; unit: string; caption: string }) {
  return (
    <div className="flex flex-col items-center">
      <div className="relative">
        <Donut pct={pct} size={92} stroke={6} />
        <div className="absolute inset-0 flex flex-col items-center justify-center">
          <span className="text-[17px] font-bold leading-[1.1]">
            {pct === null ? '—' : Math.round(pct) + '%'}
          </span>
          <span className="text-[11px] font-medium tracking-wide text-t2">{unit}</span>
        </div>
      </div>
      <div className="mt-3 text-xs text-t3">{caption}</div>
    </div>
  )
}

export function PredictionCard({
  predictions, onOpen,
}: { predictions: Prediction[]; onOpen: () => void }) {
  const preds = predictions.filter((p) => (p.probability ?? 0) >= 0.5)
  const top = [...preds].sort((a, b) => b.probability - a.probability)[0]
  const type = top ? top.prediction_type.replace(/([a-z])([A-Z])/g, '$1 $2') : ''

  return (
    <Card>
      <CardContent className="pb-4 pt-4">
        <CardTitle>Prediction</CardTitle>
        <button
          onClick={onOpen}
          className="flex w-full items-center gap-3 rounded-lg px-0.5 py-1.5 text-left transition-colors hover:bg-white/[0.04]"
        >
          <span className="grid h-10 w-10 shrink-0 place-items-center rounded-[10px] bg-ok/10 text-ok">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
              strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M3 17l5-5 4 3 8-8" /><path d="M14 7h6v6" />
            </svg>
          </span>
          <span className="flex min-w-0 flex-1 flex-col gap-0.5">
            <span className="truncate text-[13px] font-medium text-t1">
              {preds.length ? `${preds.length} issue${preds.length === 1 ? '' : 's'} predicted` : 'No issues predicted'}
            </span>
            <span className="truncate text-xs text-t3">
              {top ? `${type} · ${Math.round(top.probability * 100)}%` : 'All trends nominal'}
            </span>
          </span>
          <svg className="h-[18px] w-[18px] shrink-0 text-t3" viewBox="0 0 24 24" fill="none"
            stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="m9 18 6-6-6-6" />
          </svg>
        </button>
      </CardContent>
    </Card>
  )
}

export function RecentAlerts({
  alerts, onAck, ackPending,
}: { alerts: Alert[]; onAck: (id: number) => void; ackPending: Set<number> }) {
  return (
    <Card className="flex min-h-0 flex-1 flex-col">
      <CardContent className="flex min-h-0 flex-1 flex-col pb-4 pt-4">
        <CardTitle>Recent Alerts</CardTitle>
        {alerts.length === 0 ? (
          <div className="flex flex-1 items-center justify-center py-5 text-[13px] text-t3">
            No recent alerts
          </div>
        ) : (
          <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
            {alerts.slice(0, 6).map((a) => {
              const crit = (a.severity || '').toLowerCase() === 'critical'
              const dev = a.device_id ? a.device_id.slice(a.device_id.indexOf(':') + 1) : ''
              const pending = ackPending.has(a.id)
              return (
                <div key={a.id} className="flex min-h-12 items-center gap-3">
                  <span className={cn('h-[9px] w-[9px] shrink-0 rounded-full', crit ? 'bg-crit' : 'bg-warn')} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[13px] font-medium leading-[1.3] text-t1">
                      {a.rule_name || 'Alert'}
                    </span>
                    <span className="block truncate text-xs leading-[1.3] text-t3">{dev}</span>
                  </span>
                  <span className="shrink-0 text-xs text-t3">{relTime(a.created_at)}</span>
                  <button
                    onClick={() => onAck(a.id)}
                    disabled={pending}
                    className="shrink-0 rounded-full border border-line px-3 py-1.5 text-xs font-medium text-t2 transition-colors hover:border-ok/45 hover:text-ok disabled:opacity-40"
                  >
                    {pending ? '···' : 'Ack'}
                  </button>
                </div>
              )
            })}
          </div>
        )}
      </CardContent>
    </Card>
  )
}

export function PredictionsList({ predictions }: { predictions: Prediction[] }) {
  const preds = [...predictions].sort((a, b) => b.probability - a.probability)
  if (preds.length === 0) {
    return <div className="py-8 text-center text-[13px] text-t3">No active predictions</div>
  }
  return (
    <div className="flex flex-col gap-2">
      {preds.map((p) => {
        const pct = Math.round((p.probability ?? 0) * 100)
        const type = (p.prediction_type || 'Unknown').replace(/([a-z])([A-Z])/g, '$1 $2')
        const dev = p.device_id ? p.device_id.slice(p.device_id.indexOf(':') + 1) : ''
        return (
          <div key={p.id} className="flex items-center gap-3.5 rounded-lg bg-row p-3.5">
            <span className="grid h-10 w-10 shrink-0 place-items-center rounded-[10px] bg-info/10 text-info">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M3 17l5-5 4 3 8-8" /><path d="M14 7h6v6" />
              </svg>
            </span>
            <div className="min-w-0 flex-1">
              <div className="text-[15px] font-medium">{type}</div>
              <div className="mt-1 font-mono text-xs text-t3">
                {dev} · {relTime(p.created_at)}
              </div>
            </div>
            <div className="h-1.5 w-[160px] shrink-0 overflow-hidden rounded-full bg-white/[0.055]">
              <div
                className="h-full rounded-full"
                style={{ width: `${pct}%`, background: pct >= 90 ? SEV_COLOR.crit : SEV_COLOR.warn }}
              />
            </div>
            <div className="w-[100px] shrink-0 text-right">
              <div className={cn('text-base font-bold', pct >= 90 ? 'text-crit' : 'text-warn')}>{pct}%</div>
              <div className="text-xs text-t3">{p.eta_minutes ? `ETA ~${p.eta_minutes}m` : ''}</div>
            </div>
          </div>
        )
      })}
    </div>
  )
}

export function SeverityBadge({ sev }: { sev: 'ok' | 'warn' | 'crit' | 'idle' }) {
  return <Badge variant={sev === 'idle' ? 'default' : sev}>{sev}</Badge>
}

export { Donut }
