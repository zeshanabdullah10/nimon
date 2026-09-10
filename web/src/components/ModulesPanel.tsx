import { Card } from '@/components/ui/card'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Separator } from '@/components/ui/separator'
import { ModuleRow } from '@/components/ModuleRow'
import { cn } from '@/lib/utils'
import {
  gb, num, SEV_RANK, sevTemp, type Device,
} from '@/hooks/nimon'

const SEV_TEXT: Record<string, string> = {
  ok: 'text-ok', warn: 'text-warn', crit: 'text-crit', idle: 'text-idle',
}

export function sortDevices(devices: Device[]): Device[] {
  return [...devices].sort((a, b) => {
    const sevOf = (d: Device) =>
      d.metrics?.is_reachable === false
        ? 'idle'
        : sevTemp(num(d.metrics?.temperature))
    const ra = sevOf(a), rb = sevOf(b)
    if (SEV_RANK[ra] !== SEV_RANK[rb]) return SEV_RANK[ra] - SEV_RANK[rb]
    const sa = num(a.metrics?.slot) ?? 9999
    const sb = num(b.metrics?.slot) ?? 9999
    if (sa !== sb) return sa - sb
    return a.device_id.localeCompare(b.device_id)
  })
}

export function ModulesPanel({
  devices, station, temps, wide, alertCount, predCount, up = true,
}: {
  devices: Device[]
  station?: Device
  /** device_id -> smoothed temperature */
  temps: Record<string, number>
  wide?: boolean
  alertCount: number
  predCount: number
  up?: boolean
}) {
  const online = devices.filter((d) =>
    ['healthy', 'warning'].includes(d.status)).length
  const offline = devices.length - online
  const peak = Math.max(-Infinity, ...devices.map((d) => num(d.metrics?.temperature) ?? -Infinity))
  const hasPeak = peak !== -Infinity

  const sys = station?.metrics

  return (
    <Card className="flex h-full min-h-0 flex-col overflow-hidden">
      {/* header */}
      <div className="flex items-center justify-between gap-3 px-4 pb-3.5 pt-4">
        <h2 className="min-w-0 truncate text-[19px] font-semibold leading-tight">
          {typeof sys?.product === 'string' && sys.product ? sys.product : 'NI Test Station'}
        </h2>
        <span className="flex shrink-0 items-center gap-2 text-xs text-t2">
          <span className={cn('h-[9px] w-[9px] rounded-full', up ? 'bg-ok' : 'animate-pulse bg-crit')} />
          {up ? 'Online' : 'Offline'}
        </span>
      </div>
      <Separator />

      {/* sub-header */}
      <div className="flex items-center justify-between px-5 py-3">
        <span className="text-xs font-medium uppercase tracking-wide text-t2">Modules</span>
        <span className="text-xs font-medium text-t2">
          {online} active · {offline === 0 ? 'All modules online' : `${offline} offline`}
        </span>
      </div>

      {/* rows */}
      <ScrollArea className="min-h-0 flex-1">
        <div className="flex flex-col gap-1.5 px-2.5 pb-2.5">
          {devices.length === 0 ? (
            <div className="py-8 text-center text-[13px] text-t3">No modules detected</div>
          ) : (
            sortDevices(devices).map((d) => (
              <ModuleRow key={d.device_id} device={d} temp={temps[d.device_id] ?? null} wide={wide} />
            ))
          )}
        </div>
      </ScrollArea>

      <Separator />

      {/* today footer */}
      <div className="flex h-16 items-stretch">
        <div className="flex flex-1 flex-col justify-center px-5">
          <span className={cn('text-[28px] font-bold leading-[1.1]', hasPeak ? SEV_TEXT[sevTemp(peak)] : 'text-idle')}>
            {hasPeak ? peak.toFixed(1) + '°' : '—'}
          </span>
          <span className="mt-0.5 text-xs font-medium text-t2">Station</span>
        </div>
        <Separator orientation="vertical" />
        <div className="flex flex-1 flex-col justify-center px-5">
          <span className="text-[28px] font-bold leading-[1.1]">
            {sys ? gb(num(sys.mem_free_mb)) : '—'}
          </span>
          <span className="mt-0.5 text-xs font-medium text-t2">RAM free</span>
        </div>
        <Separator orientation="vertical" />
        <div className="flex flex-1 flex-col items-end justify-center gap-0.5 px-5 text-xs leading-[1.5] text-t2">
          <span>{alertCount} alerts</span>
          <span>{predCount} predicted</span>
          <span>{devices.length} modules</span>
        </div>
      </div>
    </Card>
  )
}
