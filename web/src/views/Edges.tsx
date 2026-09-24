import { useState, type FormEvent, type ReactNode } from 'react'
import { Loader2 } from 'lucide-react'
import { api, type Edge, type EdgeConfigPush } from '@/lib/api'
import { errText, pollers, toast, useDeviceRows, useEdgeFilter, useEdges, useSettings, withAuth } from '@/hooks/nimon'
import { EdgeChips } from '@/components/EdgeChips'
import { Ago, Card, Empty, SevBadge, Updated } from '@/components/ui'
import { clockTs, duration, parseTs } from '@/lib/format'
import { thresholdsFor } from '@/lib/severity'
import { cn } from '@/lib/utils'

export function Edges() {
  const edges = useEdges()
  const filter = useEdgeFilter()
  const list = (edges.data ?? []).filter((e) => !filter || e.edge_id === filter)
  return (
    <div className="flex flex-col gap-3.5">
      <EdgeChips />
      <Card title={`Edges · ${edges.data ? list.length : '—'}`}
        actions={<Updated at={edges.updatedAt} stale={edges.stale} error={edges.error?.message} />}>
        {edges.error && <p role="alert" className="mb-2 rounded-md border border-warn/50 bg-warn/10 px-2 py-1.5 text-xs">Edge list unavailable — {edges.error.message}</p>}
        {edges.loading ? <Empty>Loading…</Empty> : list.length === 0 ? <Empty>No edges known to this hub</Empty> : (
          <ul className="flex flex-col gap-3">{list.map((e) => <EdgeItem key={e.edge_id} e={e} />)}</ul>
        )}
      </Card>
    </div>
  )
}

function EdgeItem({ e }: { e: Edge }) {
  const rows = useDeviceRows().filter((r) => r.device.edge_id === e.edge_id)
  const problems = rows.filter((r) => !r.health.stale && r.health.sev !== 'ok').length
  const [open, setOpen] = useState(false)
  const fields: [string, ReactNode][] = [
    ['Edge id', <code className="font-mono">{e.edge_id}</code>],
    ['Host', `${e.hostname ?? '—'}${e.ip_address ? ` (${e.ip_address})` : ''}`],
    ['Version', e.version ?? '—'],
    ['Protocol', e.protocol_version ?? '—'],
    ['Uptime', e.live ? duration(e.uptime_secs) : '—'],
    ['Connected', e.connected_at ? clockTs(e.connected_at) : '—'],
    ['Last seen', e.last_seen ? <>{clockTs(e.last_seen)} (<Ago ms={parseTs(e.last_seen)} />)</> : '—'],
    ['Devices', `${e.device_count}${problems ? ` · ${problems} need attention` : ''}`],
  ]
  return (
    <li className={cn('row p-3.5', !e.live && 'border border-dashed border-fg/25 bg-transparent')}>
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-[15px] font-semibold">{e.name || e.edge_id}</h3>
        {e.live ? <SevBadge sev="ok" label="Online · live" /> : <SevBadge sev="offline" label="Offline" />}
        <button type="button" className="btn btn-sm ml-auto" aria-expanded={open} aria-controls={`cfg-${e.edge_id}`} onClick={() => setOpen((o) => !o)}>
          {open ? 'Close configuration' : 'Configure'}
        </button>
      </div>
      <dl className="mt-2 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5 text-xs sm:grid-cols-[auto_minmax(0,1fr)_auto_minmax(0,1fr)]">
        {fields.map(([k, v]) => <FieldPair key={k} k={k} v={v} />)}
      </dl>
      {open && <EdgeConfigForm edge={e} id={`cfg-${e.edge_id}`} />}
    </li>
  )
}

function FieldPair({ k, v }: { k: string; v: ReactNode }) {
  return <><dt className="text-t2">{k}</dt><dd className="min-w-0 truncate">{v}</dd></>
}

function EdgeConfigForm({ edge, id }: { edge: Edge; id: string }) {
  const settings = useSettings().data
  const eff = thresholdsFor(settings, edge.edge_id)
  const poll = settings?.edge_thresholds?.[edge.edge_id]?.poll_interval_secs ?? null
  const [pollS, setPoll] = useState(poll === null ? '' : String(poll))
  const [warn, setWarn] = useState(String(eff.temperature_warning))
  const [crit, setCrit] = useState(String(eff.temperature_critical))
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null)

  const w = warn.trim() === '' ? null : Number(warn)
  const c = crit.trim() === '' ? null : Number(crit)
  const p = pollS.trim() === '' ? null : Number(pollS)
  const errors: string[] = []
  if (p !== null && (!Number.isInteger(p) || p < 1 || p > 86400)) errors.push('Poll interval must be a whole number of seconds (1–86400).')
  if (w !== null && !Number.isFinite(w)) errors.push('Warning threshold must be a number.')
  if (c !== null && !Number.isFinite(c)) errors.push('Critical threshold must be a number.')
  const effW = w ?? eff.temperature_warning
  const effC = c ?? eff.temperature_critical
  if (Number.isFinite(effW) && Number.isFinite(effC) && effW >= effC) errors.push(`Warning (${effW} °C) must be below critical (${effC} °C).`)

  const body: EdgeConfigPush = {}
  if (p !== null && p !== poll) body.poll_interval_secs = p
  if (w !== null && w !== eff.temperature_warning) body.temperature_warning = w
  if (c !== null && c !== eff.temperature_critical) body.temperature_critical = c
  const changed = Object.keys(body).length > 0

  const submit = async (ev: FormEvent) => {
    ev.preventDefault()
    if (errors.length || !changed) return
    setBusy(true)
    setResult(null)
    try {
      const r = await withAuth((t) => api.pushEdgeConfig(edge.edge_id, body, t))
      const text = r.status === 'pushed'
        ? 'Pushed to the edge — it applies the change immediately.'
        : 'Stored on the hub — it will be pushed when the edge reconnects.'
      setResult({ ok: true, text })
      toast('ok', `Configuration ${r.status} for ${edge.name || edge.edge_id}`)
      void pollers.settings.refresh()
    } catch (e) {
      setResult({ ok: false, text: errText(e) })
      toast('error', 'Configuration not applied', errText(e))
    } finally {
      setBusy(false)
    }
  }
  const errId = `${id}-err`
  return (
    <form id={id} onSubmit={submit} noValidate className="mt-3 rounded-lg border border-fg/10 p-3" aria-describedby={errors.length ? errId : undefined}>
      <p className="mb-2 text-xs text-t2">Effective now: warning {eff.temperature_warning} °C · critical {eff.temperature_critical} °C · poll {poll === null ? 'edge default' : `${poll} s`}. Only changed fields are sent.</p>
      <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
        <Num id={`${id}-poll`} label="Poll interval (s)" value={pollS} onChange={setPoll} placeholder="edge default" step="1" />
        <Num id={`${id}-warn`} label="Warning (°C)" value={warn} onChange={setWarn} step="0.5" />
        <Num id={`${id}-crit`} label="Critical (°C)" value={crit} onChange={setCrit} step="0.5" />
      </div>
      {errors.length > 0 && (
        <ul id={errId} className="mt-2 list-disc pl-5 text-xs text-crit">{errors.map((e) => <li key={e}>{e}</li>)}</ul>
      )}
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <button type="submit" className="btn btn-primary" disabled={busy || errors.length > 0 || !changed}>
          {busy && <Loader2 aria-hidden className="h-4 w-4 motion-safe:animate-spin" />}
          {busy ? 'Sending…' : `Apply to ${edge.name || edge.edge_id}`}
        </button>
        {!changed && !result && <span className="text-xs text-t2">No changes</span>}
        {result && <span role="status" className={cn('text-xs', result.ok ? 'text-ok' : 'text-crit')}>{result.text}</span>}
      </div>
    </form>
  )
}

function Num({ id, label, value, onChange, placeholder, step }: {
  id: string; label: string; value: string; onChange: (v: string) => void; placeholder?: string; step?: string
}) {
  return (
    <div>
      <label className="label" htmlFor={id}>{label}</label>
      <input id={id} className="input" type="number" inputMode="decimal" step={step} value={value} placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)} />
    </div>
  )
}
