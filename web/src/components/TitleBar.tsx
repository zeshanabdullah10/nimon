import { memo, useState, useEffect } from 'react'
import { Monitor, Moon, RefreshCw, Sun } from 'lucide-react'
import { cn } from '@/lib/utils'
import { refreshAll, themeStore, useTheme, type ThemePref } from '@/hooks/nimon'
import { HeadlinePill, useStation } from '@/components/station'

function LogoMark() {
  return (
    <svg aria-hidden width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
      <rect x="3" y="5" width="18" height="14" rx="2.6" />
      <path d="M7.6 9.2v5.6M12 9.2v5.6M16.4 9.2v5.6" />
    </svg>
  )
}

/** The only per-second render in the app. */
const Clock = memo(function Clock() {
  const [t, setT] = useState(() => new Date())
  useEffect(() => {
    const id = setInterval(() => setT(new Date()), 1000)
    return () => clearInterval(id)
  }, [])
  const p = (n: number) => String(n).padStart(2, '0')
  return (
    <time aria-hidden className="hidden font-mono text-xs tracking-wide text-t2 md:inline">
      {`${t.getFullYear()}-${p(t.getMonth() + 1)}-${p(t.getDate())} ${p(t.getHours())}:${p(t.getMinutes())}:${p(t.getSeconds())}`}
    </time>
  )
})

const NEXT: Record<ThemePref, ThemePref> = { system: 'dark', dark: 'light', light: 'system' }
const THEME_ICON = { system: Monitor, dark: Moon, light: Sun }

function ThemeToggle() {
  const pref = useTheme()
  const Icon = THEME_ICON[pref]
  return (
    <button type="button" className="btn btn-ghost h-9 w-9 px-0" onClick={() => themeStore.set(NEXT[pref])}
      aria-label={`Theme: ${pref}. Switch to ${NEXT[pref]}`} title={`Theme: ${pref}`}>
      <Icon aria-hidden className="h-[18px] w-[18px]" />
    </button>
  )
}

function RefreshButton() {
  const [busy, setBusy] = useState(false)
  return (
    <button type="button" className="btn btn-ghost h-9 w-9 px-0" aria-label="Refresh all data now" title="Refresh now"
      disabled={busy}
      onClick={async () => { setBusy(true); try { await refreshAll() } finally { setBusy(false) } }}>
      <RefreshCw aria-hidden className={cn('h-[18px] w-[18px]', busy && 'motion-safe:animate-spin')} />
    </button>
  )
}

function StationPill() {
  const st = useStation(false)
  return (
    <a href="#/overview" className="rounded-full" aria-label={`Station status ${st.level}. Open overview`}>
      <HeadlinePill level={st.level} />
    </a>
  )
}

export const TitleBar = memo(function TitleBar() {
  return (
    <header className="flex h-[52px] shrink-0 items-center justify-between gap-2 border-b border-fg/[0.07] px-3 sm:px-4">
      <div className="flex min-w-0 items-center gap-2.5">
        <div aria-hidden className="grid h-7 w-7 shrink-0 place-items-center rounded-md border-[1.5px] border-ok text-ok"><LogoMark /></div>
        <span className="text-[17px] font-semibold">NIMon</span>
        <span className="hidden truncate text-xs text-t2 sm:inline">{location.host}</span>
      </div>
      <div className="flex items-center gap-1.5 sm:gap-3">
        <StationPill />
        <Clock />
        <RefreshButton />
        <ThemeToggle />
      </div>
    </header>
  )
})
