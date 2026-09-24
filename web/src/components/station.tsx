import { memo, useEffect, useMemo, useRef } from 'react'
import { CircleCheck, Clock, Loader2, OctagonAlert, ServerCrash, TriangleAlert } from 'lucide-react'
import { cn } from '@/lib/utils'
import {
  announce, useAlerts, useDeviceRows, useDevices, useEdgeFilter, useEdges, useHealth,
} from '@/hooks/nimon'
import { stationStatus, type StationStatus } from '@/lib/station'
import type { Headline } from '@/lib/severity'
import { Updated } from '@/components/ui'

export function useStation(scoped = true): StationStatus {
  const health = useHealth()
  const devices = useDevices()
  const alerts = useAlerts()
  const edges = useEdges()
  const rows = useDeviceRows()
  const filter = useEdgeFilter()
  const f = scoped ? filter : null
  return useMemo(
    () => stationStatus(health, devices, alerts, edges, rows, f),
    // Res objects change identity on every poll; the result is cheap to compute
    [health, devices, alerts, edges, rows, f],
  )
}

export const HEADLINE_STYLE: Record<Headline, { cls: string; icon: typeof CircleCheck; text: string }> = {
  OK: { cls: 'border-ok/40 bg-ok/10 text-ok', icon: CircleCheck, text: 'All monitored modules within limits' },
  WARN: { cls: 'border-warn/45 bg-warn/10 text-warn', icon: TriangleAlert, text: 'Attention needed' },
  CRIT: { cls: 'border-crit/50 bg-crit/10 text-crit', icon: OctagonAlert, text: 'Critical — act now' },
  STALE: { cls: 'border-fg/25 bg-fg/5 text-t1', icon: Clock, text: 'Some data is not current' },
  'HUB DOWN': { cls: 'border-crit/50 bg-crit/10 text-crit', icon: ServerCrash, text: 'Hub not responding' },
  LOADING: { cls: 'border-fg/15 bg-fg/5 text-t2', icon: Loader2, text: 'Connecting to hub…' },
}

/** Compact pill for the title bar */
export const HeadlinePill = memo(function HeadlinePill({ level }: { level: Headline }) {
  const s = HEADLINE_STYLE[level]
  const Icon = s.icon
  return (
    <span className={cn('inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs font-bold tracking-wide', s.cls)}>
      <Icon aria-hidden className={cn('h-3.5 w-3.5', level === 'LOADING' && 'motion-safe:animate-spin')} />
      <span><span className="sr-only">Station status: </span>{level}</span>
    </span>
  )
})

/** Big headline card with reasons */
export function StationHeadline({ status }: { status: StationStatus }) {
  const s = HEADLINE_STYLE[status.level]
  const Icon = s.icon
  const devices = useDevices()
  return (
    <section aria-labelledby="station-headline" className={cn('flex flex-col gap-3 rounded-xl border p-4 sm:flex-row sm:items-start', s.cls)}>
      <div className="flex items-center gap-3 sm:w-[220px] sm:shrink-0">
        <Icon aria-hidden className={cn('h-9 w-9 shrink-0', status.level === 'LOADING' && 'motion-safe:animate-spin')} strokeWidth={2.2} />
        <div>
          <h2 id="station-headline" className="text-2xl font-extrabold leading-none tracking-wide">
            <span className="sr-only">Station status: </span>{status.level}
          </h2>
          <div className="mt-1 text-xs font-medium text-t2">{s.text}</div>
        </div>
      </div>
      <div className="min-w-0 flex-1">
        {status.reasons.length === 0 ? (
          <p className="text-[13px] text-t1">
            {status.counts.total} module{status.counts.total === 1 ? '' : 's'} reporting, none above warning thresholds, no active alerts.
          </p>
        ) : (
          <ul className="flex flex-col gap-1 text-[13px] text-t1">
            {status.reasons.map((r, i) => (
              <li key={i} className="flex gap-2">
                <span className="w-[62px] shrink-0 font-mono text-[11px] font-bold leading-5 text-t2">{r.level}</span>
                <span className="min-w-0">{r.text}</span>
              </li>
            ))}
          </ul>
        )}
        <div className="mt-2"><Updated at={devices.updatedAt} stale={devices.stale} error={devices.error?.message} /></div>
      </div>
    </section>
  )
}

/** Announces headline changes to screen readers (polite; assertive for CRIT/HUB DOWN) */
export function StationAnnouncer() {
  const st = useStation(false)
  const prev = useRef<string | null>(null)
  useEffect(() => {
    if (st.level === 'LOADING') return
    const key = st.level
    if (prev.current !== null && prev.current !== key) {
      announce(`Station status ${st.level}. ${st.reasons[0]?.text ?? ''}`, st.level === 'CRIT' || st.level === 'HUB DOWN')
    }
    prev.current = key
  }, [st])
  return null
}
