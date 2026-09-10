import { cn } from '@/lib/utils'
import { num, relTime, sevTemp, T_MAX, type Device } from '@/hooks/nimon'

const SEV_TINT: Record<string, string> = {
  ok: 'linear-gradient(90deg, rgba(47,212,99,0.22) 0%, rgba(47,212,99,0) 62%)',
  warn: 'linear-gradient(90deg, rgba(232,180,55,0.24) 0%, rgba(232,180,55,0) 62%)',
  crit: 'linear-gradient(90deg, rgba(229,72,77,0.28) 0%, rgba(229,72,77,0) 62%)',
  idle: 'transparent',
}
const SEV_ACCENT: Record<string, string> = {
  ok: '#2FD463', warn: '#E8B437', crit: '#E5484D', idle: 'transparent',
}
const SEV_TEXT: Record<string, string> = {
  ok: 'text-ok', warn: 'text-warn', crit: 'text-crit', idle: 'text-idle',
}

export function ModuleRow({
  device, temp, wide,
}: { device: Device; temp: number | null; wide?: boolean }) {
  const reachable = device.metrics?.is_reachable !== false
  const hasTemp = reachable && temp !== null
  const sev = hasTemp ? sevTemp(temp) : 'idle'

  const id = device.device_id
  const i = id.indexOf(':')
  const short = i === -1 ? id : id.slice(i + 1)
  const h = short.lastIndexOf('#')
  const name = (h === -1 ? short : short.slice(0, h)).trim()
  const tag = h === -1 ? '' : short.slice(h + 1)

  const slot = num(device.metrics?.slot)
  const product = typeof device.metrics?.product === 'string' ? device.metrics.product : ''

  const meta = [
    [slot !== null ? `Slot ${slot}` : '', product].filter(Boolean).join(' · '),
    reachable ? 'link ok' : 'no link',
    relTime(device.last_seen),
  ].filter(Boolean).join(' · ')

  const dotCls = !reachable
    ? 'border-[1.5px] border-idle bg-transparent'
    : hasTemp && sev !== 'ok'
      ? cn('bg-current', SEV_TEXT[sev])
      : 'bg-ok'

  const sensors = wide
    ? Object.entries(device.metrics ?? {})
        .filter(([k]) => k.startsWith('temperature['))
        .map(([k, v]) => `${k.slice(12, -1)} ${Number(v).toFixed(1)}°`)
    : []

  return (
    <div className="group relative flex min-h-[56px] items-center rounded-lg bg-row py-2 transition-colors hover:bg-white/[0.06]">
      {/* accent bar — rows with data only */}
      <span
        className="absolute bottom-0 left-0 top-0 w-[3px] rounded-l-lg transition-opacity duration-200"
        style={{
          background: hasTemp ? SEV_ACCENT[sev] : 'transparent',
          opacity: hasTemp ? 1 : 0,
        }}
      />
      {/* tint layer — fades right */}
      <span
        className="pointer-events-none absolute inset-0 rounded-lg transition-opacity duration-200"
        style={{ background: SEV_TINT[sev], opacity: hasTemp ? 1 : 0 }}
      />

      <div className="relative flex w-full items-center justify-between gap-3 px-3.5">
        <div className="flex min-w-0 items-center gap-3">
          <span className={cn('h-[9px] w-[9px] shrink-0 rounded-full', dotCls)} />
          <div className="min-w-0">
            <div className="truncate text-sm font-medium leading-[1.4] text-t1">
              {name}{tag && <span className="text-t3"> · #{tag}</span>}
            </div>
            <div className="truncate text-xs leading-[1.4] text-t3">{meta}</div>
            {wide && sensors.length > 0 && (
              <div className="mt-1 flex flex-wrap gap-1">
                {sensors.map((s) => (
                  <span key={s} className="rounded border border-linecard bg-white/5 px-1.5 py-0.5 font-mono text-[11px] text-t2">
                    {s}
                  </span>
                ))}
              </div>
            )}
          </div>
        </div>

        <div className={cn('shrink-0 text-right', wide ? 'w-[150px]' : 'w-[110px]')}>
          {hasTemp ? (
            <>
              <div className={cn('text-[17px] font-semibold leading-[1.3]', SEV_TEXT[sev])}>
                {temp.toFixed(1)}
                <span className="text-xs font-medium opacity-70">°C</span>
              </div>
              <div className="text-xs leading-[1.3] text-t2">
                {Math.round(Math.max(0, T_MAX - temp))}° headroom
              </div>
            </>
          ) : (
            <>
              <div className="text-[17px] font-semibold leading-[1.3] text-idle">—</div>
              <div className="text-xs leading-[1.3] text-t3">no data</div>
            </>
          )}
        </div>
      </div>
    </div>
  )
}
