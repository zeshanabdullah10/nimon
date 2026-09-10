import { Bell, Gauge, Inspect, LayoutGrid, Settings2, Thermometer } from 'lucide-react'
import { cn } from '@/lib/utils'

export type ViewId = 'overview' | 'modules' | 'alerts' | 'predictions' | 'resources' | 'settings'

const ITEMS: { id: ViewId; label: string; icon: typeof LayoutGrid }[] = [
  { id: 'overview', label: 'Overview', icon: LayoutGrid },
  { id: 'modules', label: 'Modules', icon: Thermometer },
  { id: 'alerts', label: 'Alerts', icon: Bell },
  { id: 'predictions', label: 'Predictions', icon: Inspect },
  { id: 'resources', label: 'Resources', icon: Gauge },
  { id: 'settings', label: 'Settings', icon: Settings2 },
]

export function NavRail({
  view, alertCount, onSelect,
}: { view: ViewId; alertCount: number; onSelect: (v: ViewId) => void }) {
  return (
    <nav className="flex w-16 shrink-0 flex-col gap-1 border-r border-linecard p-2 lg:w-[168px] lg:border-r-0">
      {ITEMS.map(({ id, label, icon: Icon }) => (
        <button
          key={id}
          onClick={() => onSelect(id)}
          title={label}
          className={cn(
            'relative flex h-10 items-center justify-center gap-3 rounded-lg px-3 text-sm font-medium transition-colors lg:justify-start',
            view === id
              ? 'bg-navactive text-t1'
              : 'text-t3 hover:bg-white/[0.03] hover:text-t2',
          )}
        >
          {view === id && (
            <span className="absolute left-0 top-2 h-[22px] w-[3px] rounded-sm bg-ok" />
          )}
          <Icon className={cn('h-[17px] w-[17px] shrink-0', view === id && 'text-ok')} strokeWidth={2} />
          <span className="hidden lg:inline">{label}</span>
          {id === 'alerts' && alertCount > 0 && (
            <span className="absolute right-2 top-1.5 grid h-[17px] min-w-[17px] place-items-center rounded-full bg-crit px-1 text-[10px] font-semibold text-white lg:static lg:ml-auto">
              {alertCount}
            </span>
          )}
        </button>
      ))}
    </nav>
  )
}
