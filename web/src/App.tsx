import { usePolling, useAlerts, useHealth } from '@/hooks/nimon'
import { useRoute, type ViewId } from '@/hooks/route'
import { TitleBar } from '@/components/TitleBar'
import { NavRail } from '@/components/NavRail'
import { DeviceDrawer } from '@/components/DeviceDrawer'
import { LiveRegion, Toaster, TokenDialog } from '@/components/Overlays'
import { StationAnnouncer } from '@/components/station'
import { TopLayer } from '@/components/ui'
import { Overview } from '@/views/Overview'
import { Devices } from '@/views/Devices'
import { Alerts } from '@/views/Alerts'
import { Edges } from '@/views/Edges'
import { Predictions } from '@/views/Predictions'
import { Settings } from '@/views/Settings'
import { useEffect } from 'react'

const TITLES: Record<ViewId, string> = {
  overview: 'Overview', devices: 'Modules', alerts: 'Alerts', edges: 'Edges', predictions: 'Predictions', settings: 'Settings',
}

export default function App() {
  usePolling()
  const { view } = useRoute()
  useEffect(() => { document.title = `${TITLES[view]} · NIMon` }, [view])
  return (
    <div className="app-shell flex flex-col overflow-hidden bg-window">
      <a href="#main" onClick={(e) => { e.preventDefault(); document.getElementById('main')?.focus() }}
        className="sr-only z-50 rounded-md bg-card px-3 py-2 focus:not-sr-only focus:fixed focus:left-2 focus:top-2">Skip to main content</a>
      <TitleBar />
      <div className="flex min-h-0 flex-1 flex-col sm:flex-row">
        <NavRail view={view} />
        <main id="main" tabIndex={-1} aria-label={TITLES[view]} className="relative min-w-0 flex-1 overflow-y-auto p-3 outline-none sm:p-4">
          <h1 className="sr-only">{TITLES[view]}</h1>
          <HubBanners />
          {view === 'overview' && <Overview />}
          {view === 'devices' && <Devices />}
          {view === 'alerts' && <Alerts />}
          {view === 'edges' && <Edges />}
          {view === 'predictions' && <Predictions />}
          {view === 'settings' && <Settings />}
        </main>
      </div>
      <DeviceDrawer />
      <TokenDialog />
      <StationAnnouncer />
      <TopLayer>
        <Toaster />
        <LiveRegion />
      </TopLayer>
    </div>
  )
}

/** Global banners: hub unreachable / degraded / alerts unavailable. */
function HubBanners() {
  const health = useHealth()
  const alerts = useAlerts()
  const banners: { key: string; tone: 'crit' | 'warn'; text: string }[] = []
  if (health.error?.unreachable) {
    banners.push({ key: 'down', tone: 'crit', text: `Hub not responding (${health.error.message}). All values are last known and may be out of date.` })
  } else if (health.data?.status === 'degraded') {
    const parts = [!health.data.db_ok && 'database', !health.data.alert_manager_ok && 'alert manager'].filter(Boolean).join(' and ')
    banners.push({ key: 'degraded', tone: 'warn', text: `Hub degraded: ${parts} unavailable.` })
  }
  if (alerts.error && !health.error?.unreachable) {
    banners.push({ key: 'alerts', tone: 'warn', text: `Alerts unavailable (${alerts.error.message}). ${alerts.data ? 'Showing the last alert list received — it may be out of date.' : 'No alert data received yet.'}` })
  }
  if (!banners.length) return null
  return (
    <div className="mb-3 flex flex-col gap-2">
      {banners.map((b) => (
        <div key={b.key} role="alert"
          className={b.tone === 'crit' ? 'rounded-lg border border-crit/50 bg-crit/10 px-3 py-2 text-[13px] font-medium' : 'rounded-lg border border-warn/50 bg-warn/10 px-3 py-2 text-[13px] font-medium'}>
          {b.text}
        </div>
      ))}
    </div>
  )
}
