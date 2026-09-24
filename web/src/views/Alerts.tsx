import { useMemo, useRef, useState } from 'react'
import { Check, CheckCheck, Loader2 } from 'lucide-react'
import { api, type Alert, type HistoryQuery } from '@/lib/api'
import {
  ackAlert, resolveAlert, useAlerts, useDevices, useEdgeFilter, useEdges, usePolled,
} from '@/hooks/nimon'
import { href, navigate, useRoute } from '@/hooks/route'
import { EdgeChips } from '@/components/EdgeChips'
import { Ago, Card, Empty, SevBadge, Updated } from '@/components/ui'
import { alertSev } from '@/lib/severity'
import { clockTs, deviceLabel, parseTs } from '@/lib/format'
import { cn } from '@/lib/utils'

export function Alerts() {
  const { sub } = useRoute()
  const tab = sub === 'history' ? 'history' : 'active'
  const tabs: [typeof tab, string][] = [['active', 'Active'], ['history', 'History']]
  const refs = useRef<(HTMLButtonElement | null)[]>([])
  return (
    <div className="flex flex-col gap-3.5">
      <div role="tablist" aria-label="Alert views" className="flex gap-1.5">
        {tabs.map(([id, label], i) => (
          <button key={id} ref={(el) => { refs.current[i] = el }} role="tab" id={`tab-${id}`} aria-selected={tab === id} aria-controls={`panel-${id}`}
            tabIndex={tab === id ? 0 : -1}
            onKeyDown={(e) => {
              if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
                const next = tabs[(i + 1) % 2][0]
                navigate({ view: 'alerts', sub: next === 'history' ? 'history' : null })
                refs.current[(i + 1) % 2]?.focus()
              }
            }}
            onClick={() => navigate({ view: 'alerts', sub: id === 'history' ? 'history' : null })}
            className={cn('btn', tab === id && 'border-info bg-info/20 font-semibold')}>{label}</button>
        ))}
      </div>
      <EdgeChips />
      <div role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`}>
        {tab === 'active' ? <ActiveAlerts /> : <AlertHistory />}
      </div>
    </div>
  )
}

/* ═══ active ══════════════════════════════════════════════════════════ */

function ActiveAlerts() {
  const alerts = useAlerts()
  const filter = useEdgeFilter()
  const list = useMemo(() => (alerts.data ?? []).filter((a) => !filter || a.edge_id === filter), [alerts.data, filter])

  const devices = useDevices().data
  const edges = useEdges().data
  const devById = useMemo(() => new Map((devices ?? []).map((d) => [d.device_id, d])), [devices])
  const edgeById = useMemo(() => new Map((edges ?? []).map((e) => [e.edge_id, e])), [edges])

  return (
    <Card title={`Active alerts · ${alerts.data ? list.length : '—'}`}
      actions={<Updated at={alerts.updatedAt} stale={alerts.stale} error={alerts.error?.message} />}>
      {alerts.error && (
        <div role="alert" className="mb-3 rounded-md border border-warn/50 bg-warn/10 px-3 py-2 text-[13px]">
          <strong>Alerts unavailable:</strong> {alerts.error.message}. {alerts.data ? 'The list below is the last one received and may be out of date.' : 'No alert data has been received.'}
        </div>
      )}
      {alerts.loading ? <Empty>Loading…</Empty> : list.length === 0 ? <Empty>{alerts.data ? 'No active alerts' : 'No alert data'}</Empty> : (
        <ul className="flex flex-col gap-2">
          {list.map((a) => (
            <AlertItem key={a.id} a={a}
              device={devById.get(a.device_id)} edgeName={edgeById.get(a.edge_id)?.name} />
          ))}
        </ul>
      )}
    </Card>
  )
}

function AlertItem({ a, device, edgeName }: {
  a: Alert; device?: { device_id: string; name: string; slot: number | null }; edgeName?: string
}) {
  const [busy, setBusy] = useState<null | 'ack' | 'resolve'>(null)
  const sev = alertSev(a.severity)
  const dl = deviceLabel(a.device_id, device?.name)
  const acked = a.status === 'acknowledged'
  const run = async (op: 'ack' | 'resolve') => {
    setBusy(op)
    try { await (op === 'ack' ? ackAlert(a.id) : resolveAlert(a.id)) } finally { setBusy(null) }
  }
  const value = a.metric_value !== null && a.metric_value !== undefined
    ? `${a.metric_name ?? 'value'} ${a.metric_value.toFixed(a.metric_name === 'temperature' ? 1 : 2)}${a.metric_name === 'temperature' ? ' °C' : ''}` +
      (a.threshold !== null && a.threshold !== undefined ? ` vs threshold ${a.threshold}` : '')
    : null
  return (
    <li className={cn('row flex flex-col gap-3 p-3.5 sm:flex-row sm:items-start', acked && 'border border-dashed border-fg/20')}>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-1.5">
          <SevBadge sev={sev} label={a.severity} />
          <span className={cn('rounded-full border px-2 py-0.5 text-[11px] font-semibold uppercase tracking-wide',
            acked ? 'border-fg/20 text-t2' : 'border-fg/25 text-t1')}>{a.status}</span>
          <h3 className="min-w-0 text-[15px] font-semibold">{a.title || a.rule_id}</h3>
        </div>
        {a.message && <p className="mt-1 break-words text-[13px] leading-relaxed text-t2">{a.message}</p>}
        <dl className="mt-2 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5 text-xs sm:grid-cols-[auto_minmax(0,1fr)_auto_minmax(0,1fr)]">
          <dt className="text-t2">Device</dt>
          <dd className="min-w-0 truncate">
            <a className="font-medium underline underline-offset-2" href={href({ device: a.device_id })}>
              {dl.name}{dl.tag ? ` #${dl.tag}` : ''}
            </a>{device?.slot != null ? ` · slot ${device.slot}` : ''}{edgeName ? ` · ${edgeName}` : ''}
          </dd>
          <dt className="text-t2">Value</dt><dd>{value ?? '—'}</dd>
          <dt className="text-t2">First fired</dt><dd>{clockTs(a.triggered_at)} (<Ago ms={parseTs(a.triggered_at)} />)</dd>
          <dt className="text-t2">Last fired</dt><dd>{a.last_fired_at ? <>{clockTs(a.last_fired_at)} (<Ago ms={parseTs(a.last_fired_at)} />)</> : '—'}</dd>
          <dt className="text-t2">Fired</dt><dd>{a.fired_count}× {a.notification_sent ? '· notified' : ''}</dd>
          {acked && <><dt className="text-t2">Acknowledged</dt><dd>{a.acknowledged_at ? clockTs(a.acknowledged_at) : 'yes'}</dd></>}
        </dl>
      </div>
      <div className="flex shrink-0 gap-2">
        <button type="button" className="btn" disabled={acked || busy !== null} onClick={() => run('ack')}
          aria-label={`Acknowledge alert ${a.title} on ${dl.name}`}>
          {busy === 'ack' ? <Loader2 aria-hidden className="h-4 w-4 motion-safe:animate-spin" /> : <Check aria-hidden className="h-4 w-4" />}
          {acked ? 'Acknowledged' : busy === 'ack' ? 'Acknowledging…' : 'Acknowledge'}
        </button>
        <button type="button" className="btn" disabled={busy !== null} onClick={() => run('resolve')}
          aria-label={`Resolve alert ${a.title} on ${dl.name}`}>
          {busy === 'resolve' ? <Loader2 aria-hidden className="h-4 w-4 motion-safe:animate-spin" /> : <CheckCheck aria-hidden className="h-4 w-4" />}
          {busy === 'resolve' ? 'Resolving…' : 'Resolve'}
        </button>
      </div>
    </li>
  )
}

/* ═══ history ═════════════════════════════════════════════════════════ */

const RANGES: [string, string, number][] = [
  ['1h', 'Last hour', 3600e3], ['6h', 'Last 6 hours', 6 * 3600e3], ['24h', 'Last 24 hours', 24 * 3600e3],
  ['7d', 'Last 7 days', 7 * 86400e3], ['30d', 'Last 30 days', 30 * 86400e3], ['all', 'All time', 0], ['custom', 'Custom…', 0],
]

function AlertHistory() {
  const edgeFilter = useEdgeFilter()
  const devices = useDevices().data ?? []
  const [severity, setSeverity] = useState('')
  const [status, setStatus] = useState('')
  const [device, setDevice] = useState('')
  const [range, setRange] = useState('24h')
  const [since, setSince] = useState('')
  const [until, setUntil] = useState('')
  const [limit, setLimit] = useState(200)

  const query: HistoryQuery = useMemo(() => {
    const r = RANGES.find((x) => x[0] === range)!
    const q: HistoryQuery = { severity, status, device_id: device, edge_id: edgeFilter ?? undefined, limit }
    // round to the minute so the key (and the poller) stay stable
    if (r[2]) q.since = new Date(Math.floor((Date.now() - r[2]) / 60e3) * 60e3).toISOString()
    if (range === 'custom') {
      if (since) q.since = new Date(since).toISOString()
      if (until) q.until = new Date(until).toISOString()
    }
    return q
  }, [severity, status, device, edgeFilter, range, since, until, limit])
  const key = JSON.stringify(query)
  const hist = usePolled(`history:${key}`, (signal) => api.alertHistory(query, { signal }), 30_000)
  const devOpts = devices.filter((d) => !edgeFilter || d.edge_id === edgeFilter)
  const rows = hist.data?.alerts ?? []

  return (
    <Card title={`Alert history · ${hist.data ? rows.length : '—'}${hist.data && rows.length >= limit ? '+' : ''}`}
      actions={<Updated at={hist.updatedAt} stale={hist.stale} error={hist.error?.message} />}>
      <form className="mb-3 grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-6" onSubmit={(e) => e.preventDefault()} aria-label="History filters">
        <Select id="h-sev" label="Severity" value={severity} onChange={setSeverity}
          options={[['', 'Any'], ['critical', 'Critical'], ['warning', 'Warning'], ['info', 'Info']]} />
        <Select id="h-status" label="Status" value={status} onChange={setStatus}
          options={[['', 'Any'], ['firing', 'Firing'], ['pending', 'Pending'], ['acknowledged', 'Acknowledged'], ['resolved', 'Resolved'], ['suppressed', 'Suppressed']]} />
        <Select id="h-dev" label="Device" value={device} onChange={setDevice}
          options={[['', 'Any'], ...devOpts.map((d) => [d.device_id, `${deviceLabel(d.device_id, d.name).name} (${d.edge_id})`] as [string, string])]} />
        <Select id="h-range" label="Time range" value={range} onChange={setRange} options={RANGES.map(([v, l]) => [v, l])} />
        <Select id="h-limit" label="Max rows" value={String(limit)} onChange={(v) => setLimit(Number(v))}
          options={[['100', '100'], ['200', '200'], ['500', '500'], ['1000', '1000']]} />
        <div className="flex items-end text-xs text-t2">Edge: {edgeFilter ?? 'all'} (use chips)</div>
        {range === 'custom' && <>
          <div><label className="label" htmlFor="h-since">From</label><input id="h-since" type="datetime-local" className="input" value={since} onChange={(e) => setSince(e.target.value)} /></div>
          <div><label className="label" htmlFor="h-until">To</label><input id="h-until" type="datetime-local" className="input" value={until} onChange={(e) => setUntil(e.target.value)} /></div>
        </>}
      </form>
      {hist.error && <p role="alert" className="mb-2 rounded-md border border-crit/50 bg-crit/10 px-2 py-1.5 text-xs">History unavailable — {hist.error.message}</p>}
      {hist.loading ? <Empty>Loading history…</Empty> : rows.length === 0 ? <Empty>No alerts match these filters</Empty> : (
        <div className="-mx-1 overflow-x-auto px-1">
          <table className="w-full min-w-[820px] border-collapse text-left text-xs">
            <caption className="sr-only">Alert history, newest first</caption>
            <thead className="text-t2">
              <tr className="border-b border-fg/10">
                {['Triggered', 'Severity', 'Status', 'Alert', 'Device', 'Value / threshold', 'Fired', 'Last fired', 'Resolved', 'Action'].map((h) => (
                  <th key={h} scope="col" className="px-2 py-2 font-semibold">{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.id} className="border-b border-fg/[0.06] align-top">
                  <td className="whitespace-nowrap px-2 py-2">{clockTs(r.triggered_at)}</td>
                  <td className="px-2 py-2"><SevBadge sev={alertSev(r.severity)} label={r.severity} /></td>
                  <td className="px-2 py-2">{r.status}{r.acknowledged_at ? <div className="text-t2">ack {clockTs(r.acknowledged_at, false)}</div> : null}</td>
                  <td className="max-w-[260px] px-2 py-2"><div className="font-medium">{r.title}</div><div className="break-words text-t2">{r.message}</div></td>
                  <td className="px-2 py-2">{r.device_id
                    ? <a className="underline underline-offset-2" href={href({ device: r.device_id })}>{deviceLabel(r.device_id).name}</a>
                    : '—'}<div className="text-t2">{r.edge_id}</div></td>
                  <td className="whitespace-nowrap px-2 py-2">{r.metric_value !== null ? r.metric_value.toFixed(1) : '—'}{r.threshold !== null ? ` / ${r.threshold}` : ''}</td>
                  <td className="px-2 py-2">{r.fired_count}×</td>
                  <td className="whitespace-nowrap px-2 py-2">{clockTs(r.last_fired_at)}</td>
                  <td className="whitespace-nowrap px-2 py-2">{clockTs(r.resolved_at)}</td>
                  <td className="max-w-[160px] break-words px-2 py-2">{r.action_taken ?? '—'}{r.action_result ? <div className="text-t2">{r.action_result}</div> : null}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  )
}

function Select({ id, label, value, onChange, options }: {
  id: string; label: string; value: string; onChange: (v: string) => void; options: [string, string][]
}) {
  return (
    <div className="min-w-0">
      <label className="label" htmlFor={id}>{label}</label>
      <select id={id} className="input" value={value} onChange={(e) => onChange(e.target.value)}>
        {options.map(([v, l]) => <option key={v} value={v}>{l}</option>)}
      </select>
    </div>
  )
}
