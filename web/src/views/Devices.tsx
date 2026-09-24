import { useMemo, useState } from 'react'
import { useDevices, useEdges } from '@/hooks/nimon'
import { EdgeChips } from '@/components/EdgeChips'
import { DeviceList } from '@/components/DeviceList'
import { Card, Empty, Updated } from '@/components/ui'
import { useScopedRows } from '@/views/Overview'

type Filter = 'all' | 'problems' | 'stale' | 'simulated'

export function Devices() {
  const rows = useScopedRows()
  const devices = useDevices()
  const multiEdge = (useEdges().data?.length ?? 0) > 1
  const [filter, setFilter] = useState<Filter>('all')
  const [q, setQ] = useState('')
  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase()
    return rows.filter((r) => {
      if (filter === 'problems' && (r.health.stale || r.health.sev === 'ok')) return false
      if (filter === 'stale' && !r.health.stale) return false
      if (filter === 'simulated' && !r.device.is_simulated) return false
      if (!needle) return true
      const d = r.device
      return [d.device_id, d.name, d.model, d.serial_number, d.device_type].some((v) => v?.toLowerCase().includes(needle))
    })
  }, [rows, filter, q])
  const FILTERS: [Filter, string][] = [['all', 'All'], ['problems', 'Needs attention'], ['stale', 'Not reporting'], ['simulated', 'Simulated']]

  return (
    <div className="flex flex-col gap-3.5">
      <EdgeChips />
      <Card title={`Modules · ${shown.length} of ${rows.length}`}
        actions={<Updated at={devices.updatedAt} stale={devices.stale} error={devices.error?.message} />}>
        <div className="mb-3 flex flex-wrap items-end gap-2">
          <div role="group" aria-label="Filter modules" className="flex flex-wrap gap-1.5">
            {FILTERS.map(([id, label]) => (
              <button key={id} type="button" aria-pressed={filter === id} onClick={() => setFilter(id)}
                className={`btn btn-sm ${filter === id ? 'border-info/60 bg-info/15' : ''}`}>{label}</button>
            ))}
          </div>
          <div className="min-w-[180px] flex-1 sm:max-w-[280px]">
            <label htmlFor="dev-search" className="sr-only">Search modules</label>
            <input id="dev-search" type="search" className="input" placeholder="Search name, model, serial…" value={q} onChange={(e) => setQ(e.target.value)} />
          </div>
        </div>
        {devices.error && <p className="mb-2 rounded-md border border-warn/40 bg-warn/10 px-2 py-1.5 text-xs">Device list unavailable — {devices.error.message}. Showing last known values.</p>}
        {devices.loading ? <Empty>Loading modules…</Empty> : shown.length === 0 ? <Empty>No modules match</Empty> : <DeviceList rows={shown} wide showEdge={multiEdge} />}
      </Card>
    </div>
  )
}
