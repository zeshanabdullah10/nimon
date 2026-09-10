import { useEffect, useRef, useState } from 'react'
import { RefreshCw, Settings } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'

function LogoMark() {
  return (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none"
      stroke="currentColor" strokeWidth="2" strokeLinecap="round">
      <rect x="3" y="5" width="18" height="14" rx="2.6" />
      <path d="M7.6 9.2v5.6M12 9.2v5.6M16.4 9.2v5.6" />
    </svg>
  )
}

function clock() {
  const d = new Date()
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}.${p(d.getMonth() + 1)}.${p(d.getDate())} ` +
    `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`
}

export function TitleBar({
  up, busy, edges, onRefresh,
}: { up: boolean; busy: boolean; edges: number; onRefresh: () => void }) {
  const [time, setTime] = useState(clock)
  const [menu, setMenu] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const t = setInterval(() => setTime(clock()), 1000)
    return () => clearInterval(t)
  }, [])

  useEffect(() => {
    if (!menu) return
    const close = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) setMenu(false)
    }
    window.addEventListener('mousedown', close)
    return () => window.removeEventListener('mousedown', close)
  }, [menu])

  return (
    <div className="flex h-[52px] shrink-0 items-center justify-between border-b border-linecard px-4">
      <div className="flex items-center gap-3">
        <div className="grid h-7 w-7 place-items-center rounded-md border-[1.5px] border-ok text-ok">
          <LogoMark />
        </div>
        <span className="text-[17px] font-semibold">NIMon</span>
      </div>

      <div className="flex items-center gap-3.5">
        <span className={cn('h-2.5 w-2.5 rounded-full', up ? 'bg-ok' : 'bg-crit animate-pulse')}
          title={up ? 'Hub online' : 'Hub offline'} />
        <Button variant="ghost" size="icon" onClick={onRefresh} title="Refresh now">
          <RefreshCw className={cn('h-[18px] w-[18px]', busy && 'animate-spin')} />
        </Button>
        <span className="hidden font-mono text-xs tracking-wide text-t3 sm:inline">{time}</span>

        <div className="relative" ref={menuRef}>
          <Button variant="ghost" size="icon" onClick={() => setMenu((m) => !m)} title="Settings">
            <Settings className="h-[18px] w-[18px]" />
          </Button>
          {menu && (
            <div className="absolute right-0 top-11 z-50 w-64 rounded-lg border border-line bg-window p-3.5 text-[13px] shadow-2xl">
              <div className="mb-2 text-[11px] font-semibold uppercase tracking-widest text-t3">
                Connection
              </div>
              <div className="flex justify-between py-1">
                <span className="text-t2">Hub</span>
                <span className="font-mono text-t1">{location.host}</span>
              </div>
              <div className="flex justify-between py-1">
                <span className="text-t2">Poll</span>
                <span className="font-mono">5s</span>
              </div>
              <div className="flex justify-between py-1">
                <span className="text-t2">Edges</span>
                <span className="font-mono">{edges}</span>
              </div>
              <a href="/api/v1/status" className="mt-2 block rounded-md bg-white/5 px-2 py-1.5 text-center text-ok hover:bg-white/10">
                Raw API
              </a>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
