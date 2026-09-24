import { memo } from 'react'
import { Bell, Cpu, LayoutGrid, Network, Settings2, TrendingUp } from 'lucide-react'
import { cn } from '@/lib/utils'
import { href, type ViewId } from '@/hooks/route'
import { useAlerts } from '@/hooks/nimon'

const ITEMS: { id: ViewId; label: string; icon: typeof LayoutGrid }[] = [
  { id: 'overview', label: 'Overview', icon: LayoutGrid },
  { id: 'devices', label: 'Modules', icon: Cpu },
  { id: 'alerts', label: 'Alerts', icon: Bell },
  { id: 'edges', label: 'Edges', icon: Network },
  { id: 'predictions', label: 'Predictions', icon: TrendingUp },
  { id: 'settings', label: 'Settings', icon: Settings2 },
]

function AlertCount() {
  const a = useAlerts()
  const n = a.data?.filter((x) => x.status !== 'acknowledged').length ?? 0
  if (a.error && !a.data) return <span className="ml-auto text-[10px] font-bold text-warn" aria-label="alerts unavailable">?</span>
  if (!n) return null
  const crit = a.data!.some((x) => x.severity === 'critical' && x.status !== 'acknowledged')
  return (
    <span className={cn('absolute right-1 top-1 grid h-[17px] min-w-[17px] place-items-center rounded-full px-1 text-[10px] font-bold text-onaccent sm:static sm:ml-auto',
      crit ? 'bg-crit' : 'bg-warn')}>
      {n}<span className="sr-only"> unacknowledged {n === 1 ? 'alert' : 'alerts'}{a.stale ? ' (stale)' : ''}</span>
    </span>
  )
}

/** Horizontal bar on phones, icon rail on tablets, labelled rail on desktop. */
export const NavRail = memo(function NavRail({ view }: { view: ViewId }) {
  return (
    <nav aria-label="Primary" className="relative shrink-0 border-b border-fg/[0.07] sm:border-b-0 sm:border-r">
      <ul className="flex gap-1 overflow-x-auto p-1.5 sm:w-16 sm:flex-col sm:p-2 lg:w-[176px]">
        {ITEMS.map(({ id, label, icon: Icon }) => {
          const on = view === id
          return (
            <li key={id} className="shrink-0">
              <a href={href({ view: id, sub: null, device: null })} aria-current={on ? 'page' : undefined} title={label}
                className={cn('relative flex h-10 items-center gap-2.5 rounded-lg px-3 text-sm font-medium transition-colors sm:justify-center lg:justify-start',
                  on ? 'bg-ok/10 text-t1' : 'text-t2 hover:bg-fg/5 hover:text-t1')}>
                {on && <span aria-hidden className="absolute bottom-0 left-2 right-2 h-[3px] rounded-sm bg-ok sm:bottom-2 sm:left-0 sm:right-auto sm:top-2 sm:h-auto sm:w-[3px]" />}
                <Icon aria-hidden className={cn('h-[17px] w-[17px] shrink-0', on && 'text-ok')} strokeWidth={2} />
                <span className="text-xs sm:sr-only lg:not-sr-only lg:text-sm">{label}</span>
                {id === 'alerts' && <AlertCount />}
              </a>
            </li>
          )
        })}
      </ul>
    </nav>
  )
})
