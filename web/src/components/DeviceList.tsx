import { memo } from 'react'
import { ChevronRight } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { DeviceRow } from '@/hooks/nimon'
import { SEV_LABEL } from '@/lib/severity'
import { clockTime, deviceLabel, numMetricEntries } from '@/lib/format'
import { href } from '@/hooks/route'
import { SEV_TEXT, SevIcon, SimBadge } from '@/components/ui'

/** One module row; a link that opens the device drawer. */
export const DeviceRowItem = memo(function DeviceRowItem({ row, wide, showEdge }: { row: DeviceRow; wide?: boolean; showEdge?: boolean }) {
  const { device: d, health: h, edge } = row
  const { name, tag } = deviceLabel(d.device_id, d.name)
  const stale = h.stale
  const sevText = stale ? `Stale — last known at ${clockTime(h.lastSeenMs)}` : SEV_LABEL[h.sev]
  const meta = [
    d.slot !== null && d.slot !== undefined ? `Slot ${d.slot}` : '',
    d.model ?? '',
    showEdge ? (edge?.name || d.edge_id) : '',
  ].filter(Boolean).join(' · ')
  const sensors = wide ? numMetricEntries(d.metrics, 'temperature[') : []
  const accent = stale ? 'transparent' : h.sev === 'critical' ? 'rgb(var(--crit))' : h.sev === 'warning' ? 'rgb(var(--warn))' : 'transparent'

  return (
    <li>
      <a href={href({ device: d.device_id })}
        aria-label={`${name}${tag ? ' #' + tag : ''}, ${sevText}${h.temp !== null && !stale ? `, ${h.temp.toFixed(1)} degrees` : ''}${d.is_simulated ? ', simulated' : ''}`}
        className={cn('row relative flex min-h-[56px] items-center gap-3 overflow-hidden py-2 pl-3.5 pr-2 transition-colors hover:bg-fg/[0.07]',
          stale && 'border border-dashed border-fg/25 bg-transparent')}>
        <span aria-hidden className="absolute bottom-0 left-0 top-0 w-[3px]" style={{ background: accent }} />
        <SevIcon sev={h.sev} stale={stale} />
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 flex-wrap items-center gap-x-1.5">
            <span className="max-w-full truncate text-sm font-medium">{name}{tag && <span className="text-t2"> #{tag}</span>}</span>
            {d.is_simulated && <SimBadge />}
          </div>
          <div className="truncate text-xs text-t2">{meta || d.device_id}</div>
          <div className={cn('truncate text-xs', stale ? 'text-t2' : SEV_TEXT[h.sev])}>
            {stale ? `${sevText} (${h.staleReason})` : h.sev === 'nolink' ? 'No link — device not reachable' : sevText}
            {!stale && h.statusSev !== 'unknown' && h.statusSev !== h.sev && h.sev !== 'nolink' ? ` · edge reports ${d.status}` : ''}
          </div>
          {sensors.length > 0 && (
            <div className="mt-1 flex flex-wrap gap-1">
              {sensors.map(([k, v]) => (
                <span key={k} className="rounded border border-fg/10 bg-fg/5 px-1.5 py-0.5 font-mono text-[11px] text-t2">{k} {v.toFixed(1)}°</span>
              ))}
            </div>
          )}
        </div>
        <div className="shrink-0 text-right">
          {h.temp !== null ? (
            <div className={cn('text-[17px] font-semibold leading-tight', stale ? 'text-t2' : SEV_TEXT[h.tempSev])}>
              {h.temp.toFixed(1)}<span className="text-xs font-medium"> °C</span>
            </div>
          ) : (
            <div className="text-[17px] font-semibold leading-tight text-t2">—</div>
          )}
          <div className="hidden text-[11px] text-t2 sm:block">
            {h.temp !== null ? `warn ${h.thresholds.temperature_warning}° · crit ${h.thresholds.temperature_critical}°` : 'no temperature'}
          </div>
        </div>
        <ChevronRight aria-hidden className="hidden h-4 w-4 shrink-0 text-t3 sm:block" />
      </a>
    </li>
  )
})

export function DeviceList({ rows, wide, showEdge }: { rows: DeviceRow[]; wide?: boolean; showEdge?: boolean }) {
  return (
    <ul className="flex flex-col gap-1.5" aria-label="Modules">
      {rows.map((r) => <DeviceRowItem key={r.device.device_id} row={r} wide={wide} showEdge={showEdge} />)}
    </ul>
  )
}
