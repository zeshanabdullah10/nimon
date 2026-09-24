import { useMemo, useState, type ReactNode } from 'react'
import { Loader2, Power, RotateCcw, Terminal, Wrench, X } from 'lucide-react'
import { api, type Device, type Edge, type ManualActionType } from '@/lib/api'
import { errText, toast, useDeviceRows, useDevices, usePolled, withAuth } from '@/hooks/nimon'
import { closeDevice, useRoute } from '@/hooks/route'
import { Ago, Empty, Modal, SevBadge, SimBadge, Updated, useModalDialog } from '@/components/ui'
import { TempChart } from '@/components/TempChart'
import { clockTime, clockTs, deviceLabel, metricText, parseTs, hubNow } from '@/lib/format'
import { SEV_LABEL } from '@/lib/severity'
import { cn } from '@/lib/utils'

/** Device detail drawer (native modal dialog, closes on Esc / back button). */
export function DeviceDrawer() {
  const { device: id } = useRoute()
  const ref = useModalDialog(!!id)
  return (
    <dialog ref={ref} aria-labelledby="drawer-title"
      onCancel={(e) => { e.preventDefault(); closeDevice() }}
      onClick={(e) => { if (e.target === ref.current) closeDevice() }}
      className="fixed inset-y-0 left-auto right-0 m-0 h-full max-h-none w-full max-w-[720px] border-l border-fg/15 bg-window p-0 text-t1 shadow-2xl">
      {id && <DrawerBody id={id} />}
    </dialog>
  )
}

const RANGES: [string, number][] = [['1h', 3600e3], ['6h', 6 * 3600e3], ['24h', 24 * 3600e3]]

function DrawerBody({ id }: { id: string }) {
  const rows = useDeviceRows()
  const devices = useDevices()
  const row = rows.find((r) => r.device.device_id === id)
  const d = row?.device
  const h = row?.health
  const dl = deviceLabel(id, d?.name)
  const [range, setRange] = useState('1h')
  const rangeMs = RANGES.find((r) => r[0] === range)![1]

  const metrics = usePolled(`metrics:${id}:${range}`, async (signal) => {
    const since = new Date(hubNow() - rangeMs).toISOString()
    return (await api.metrics(id, { metric: 'temperature', since, limit: 5000 }, { signal })).points
  }, 15_000)
  const actions = usePolled(`actions:${id}`, async (signal) => (await api.actions({ device_id: id, limit: 25 }, { signal })).actions, 10_000)
  // x-axis ends at the time of the latest fetch
  const to = useMemo(() => hubNow(), [metrics.data, range])

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-start gap-3 border-b border-fg/10 px-4 py-3">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h2 id="drawer-title" className="text-lg font-semibold">{dl.name}{dl.tag && <span className="text-t2"> #{dl.tag}</span>}</h2>
            {d?.is_simulated && <SimBadge />}
            {h && <SevBadge sev={h.sev} stale={h.stale} />}
          </div>
          <div className="truncate font-mono text-xs text-t2">{id}</div>
        </div>
        <button className="btn btn-ghost h-9 w-9 px-0" onClick={closeDevice} aria-label="Close device details" autoFocus>
          <X aria-hidden className="h-5 w-5" />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto px-4 py-4">
        {!d ? (
          <Empty>{devices.loading ? 'Loading…' : 'This device is not in the current device list (removed, or the hub is unreachable).'}</Empty>
        ) : (
          <div className="flex flex-col gap-5">
            {h?.stale && (
              <p role="status" className="rounded-md border border-fg/20 bg-fg/5 px-3 py-2 text-[13px]">
                <strong>Not live:</strong> {h.staleReason}. Values below are last known at {clockTime(h.lastSeenMs)}.
              </p>
            )}
            {d.is_simulated && (
              <p className="rounded-md border border-info/40 bg-info/10 px-3 py-2 text-[13px]">This is a <strong>simulated</strong> device, not real hardware.</p>
            )}

            <Section title="Details">
              <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 text-[13px] sm:grid-cols-[auto_minmax(0,1fr)_auto_minmax(0,1fr)]">
                <Pair k="Edge" v={`${row?.edge?.name || d.edge_id}${row?.edge ? (row.edge.live ? ' (online)' : ' (offline)') : ''}`} />
                <Pair k="Host" v={row?.edge ? `${row.edge.hostname ?? '—'}${row.edge.ip_address ? ` · ${row.edge.ip_address}` : ''}` : '—'} />
                <Pair k="Type" v={d.device_type ?? '—'} />
                <Pair k="Model" v={d.model ?? '—'} />
                <Pair k="Serial" v={d.serial_number ?? '—'} />
                <Pair k="Slot" v={d.slot ?? '—'} />
                <Pair k="Edge status" v={d.status || '—'} />
                <Pair k="Link" v={d.is_reachable ? 'reachable' : 'no link'} />
                <Pair k="Severity" v={h ? (h.stale ? `stale (last ${SEV_LABEL[h.sev]})` : SEV_LABEL[h.sev]) : '—'} />
                <Pair k="Last seen" v={d.last_seen ? <>{clockTs(d.last_seen)} (<Ago ms={parseTs(d.last_seen)} />)</> : '—'} />
                <Pair k="Thresholds" v={h ? `warn ${h.thresholds.temperature_warning} °C · crit ${h.thresholds.temperature_critical} °C` : '—'} />
              </dl>
            </Section>

            <Section title="Current metrics">
              <MetricsTable metrics={d.metrics} />
              <div className="mt-1"><Updated at={devices.updatedAt} stale={devices.stale} error={devices.error?.message} /></div>
            </Section>

            <Section title="Temperature history" actions={
              <div role="group" aria-label="Time range" className="flex gap-1">
                {RANGES.map(([r]) => (
                  <button key={r} type="button" aria-pressed={range === r} onClick={() => setRange(r)}
                    className={cn('btn btn-sm', range === r && 'border-info/60 bg-info/15')}>{r}</button>
                ))}
              </div>
            }>
              {metrics.error && <p role="alert" className="mb-2 text-xs text-crit">History unavailable — {metrics.error.message}</p>}
              {metrics.loading ? <Empty>Loading history…</Empty> : h && (
                <TempChart points={metrics.data ?? []} thresholds={h.thresholds} from={to - rangeMs} to={to} />
              )}
              <div className="mt-1"><Updated at={metrics.updatedAt} stale={metrics.stale} error={metrics.error?.message} /></div>
            </Section>

            <Section title="Manual actions">
              <ActionsPanel device={d} edge={row?.edge} onDone={actions.refresh} />
            </Section>

            <Section title="Action history" actions={<Updated at={actions.updatedAt} stale={actions.stale} error={actions.error?.message} />}>
              {actions.loading ? <Empty>Loading…</Empty> : !actions.data?.length ? <Empty>{actions.error ? `Unavailable — ${actions.error.message}` : 'No actions recorded for this device'}</Empty> : (
                <ul className="flex flex-col gap-1.5">
                  {actions.data.map((a) => (
                    <li key={String(a.id)} className="row px-3 py-2 text-xs">
                      <div className="flex flex-wrap items-center gap-2">
                        {a.success === null ? <SevBadge sev="unknown" label="pending" />
                          : a.success ? <SevBadge sev="ok" label="success" /> : <SevBadge sev="critical" label="failed" />}
                        <span className="font-semibold">{a.action_type}</span>
                        <span className="text-t2">{clockTs(a.executed_at)}</span>
                        {a.alert_id ? <span className="text-t2">· auto (alert)</span> : <span className="text-t2">· manual</span>}
                        {a.duration_ms !== null && <span className="text-t2">· {a.duration_ms} ms</span>}
                        {a.retry_count ? <span className="text-t2">· {a.retry_count} retries</span> : null}
                      </div>
                      {a.command && <div className="mt-1 break-words font-mono text-t2">{a.command}</div>}
                      {a.output && <pre className="mt-1 max-h-24 overflow-auto whitespace-pre-wrap break-words font-mono text-[11px] text-t2">{a.output}</pre>}
                    </li>
                  ))}
                </ul>
              )}
            </Section>
          </div>
        )}
      </div>
    </div>
  )
}

function Section({ title, actions, children }: { title: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <section aria-label={title}>
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <h3 className="card-title">{title}</h3>
        {actions}
      </div>
      {children}
    </section>
  )
}

function Pair({ k, v }: { k: string; v: ReactNode }) {
  return <><dt className="text-t2">{k}</dt><dd className="min-w-0 break-words">{v}</dd></>
}

function MetricsTable({ metrics }: { metrics: Record<string, unknown> }) {
  const entries = useMemo(() => Object.entries(metrics ?? {}).sort(([a], [b]) => a.localeCompare(b)), [metrics])
  if (!entries.length) return <Empty>No metrics reported</Empty>
  return (
    <table className="w-full text-left text-xs">
      <caption className="sr-only">Latest metrics reported by the edge</caption>
      <tbody>
        {entries.map(([k, v]) => (
          <tr key={k} className="border-b border-fg/[0.06] last:border-0">
            <th scope="row" className="py-1.5 pr-3 font-mono font-normal text-t2">{k}</th>
            <td className="py-1.5 text-right font-mono">{metricText(v)}{/temperature/.test(k) && typeof v === 'number' ? ' °C' : ''}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

/* ═══ manual actions (explicit confirmation) ══════════════════════════ */

const ACTIONS: { type: ManualActionType; label: string; icon: typeof Power; verb: string; effect: string }[] = [
  { type: 'power_cycle', label: 'Power cycle', icon: Power, verb: 'Power-cycle',
    effect: 'The module loses power and restarts. Any test using it is interrupted and its task state is lost.' },
  { type: 'reset_driver', label: 'Reset driver', icon: RotateCcw, verb: 'Reset the driver for',
    effect: 'The NI driver session for this device is reset. Open tasks on the device are aborted.' },
  { type: 'restart_services', label: 'Restart services', icon: Wrench, verb: 'Restart services for',
    effect: 'The listed Windows services on the station are restarted. Other software using them may be interrupted.' },
  { type: 'custom_script', label: 'Custom script', icon: Terminal, verb: 'Run a custom script for',
    effect: 'The script runs on the edge station with the edge service\'s privileges.' },
]

function ActionsPanel({ device, edge, onDone }: { device: Device; edge: Edge | undefined; onDone: () => void }) {
  const [pending, setPending] = useState<ManualActionType | null>(null)
  const offline = !edge?.live
  return (
    <>
      {offline && <p className="mb-2 text-xs text-warn">The edge for this device is offline — actions cannot be delivered until it reconnects.</p>}
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
        {ACTIONS.map((a) => (
          <button key={a.type} type="button" className="btn" disabled={offline} onClick={() => setPending(a.type)}>
            <a.icon aria-hidden className="h-4 w-4" />{a.label}
          </button>
        ))}
      </div>
      {pending && <ConfirmAction device={device} edge={edge} type={pending} onClose={() => setPending(null)} onDone={onDone} />}
    </>
  )
}

function ConfirmAction({ device, edge, type, onClose, onDone }: {
  device: Device; edge: Edge | undefined; type: ManualActionType; onClose: () => void; onDone: () => void
}) {
  const spec = ACTIONS.find((a) => a.type === type)!
  const dl = deviceLabel(device.device_id, device.name)
  const [delay, setDelay] = useState('')
  const [services, setServices] = useState('')
  const [script, setScript] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const params: Record<string, string> = {}
  let invalid: string | null = null
  if (type === 'power_cycle' && delay.trim()) {
    if (!/^\d+$/.test(delay.trim())) invalid = 'Delay must be a whole number of seconds.'
    else params.delay_secs = delay.trim()
  }
  if (type === 'restart_services') {
    if (!services.trim()) invalid = 'Enter at least one service name.'
    else params.services = services.split(',').map((s) => s.trim()).filter(Boolean).join(',')
  }
  if (type === 'custom_script') {
    if (!script.trim()) invalid = 'Enter the script to run.'
    else params.script = script
  }

  const station = edge ? `${edge.name || edge.edge_id}${edge.hostname ? ` (${edge.hostname}${edge.ip_address ? ', ' + edge.ip_address : ''})` : ''}` : device.edge_id
  const submit = async () => {
    if (invalid) return
    setBusy(true)
    setError(null)
    try {
      const r = await withAuth((t) => api.deviceAction(device.device_id, type, params, t))
      toast('ok', `${spec.label} queued for ${dl.name}`, `Action id ${r.action_id} — result appears in Action history.`)
      onClose()
      setTimeout(onDone, 1500)
    } catch (e) {
      setError(errText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal open onClose={onClose} wide title={`${spec.label}: confirm`}
      footer={<>
        <button type="button" className="btn" onClick={onClose}>Cancel</button>
        <button type="button" className="btn btn-danger" disabled={busy || !!invalid} onClick={submit}>
          {busy && <Loader2 aria-hidden className="h-4 w-4 motion-safe:animate-spin" />}
          {busy ? 'Sending…' : `${spec.label} now`}
        </button>
      </>}>
      <p className="text-[14px]">
        {spec.verb} <strong>{dl.name}{dl.tag ? ` #${dl.tag}` : ''}</strong>
        {device.slot !== null ? <> in <strong>slot {device.slot}</strong></> : null}
        {device.model ? <> ({device.model})</> : null} on station <strong>{station}</strong>?
      </p>
      <p className="mt-2 text-t2">{spec.effect}</p>
      <p className="mt-1 font-mono text-xs text-t2">{device.device_id}</p>
      {device.is_simulated && <p className="mt-2 text-xs text-info">Simulated device — no real hardware is affected.</p>}
      {!device.is_simulated && <p className="mt-2 text-xs font-semibold text-warn">This is real hardware on a production test station.</p>}

      <div className="mt-3">
        {type === 'power_cycle' && (
          <div><label className="label" htmlFor="act-delay">Off time before power-on (seconds, optional)</label>
            <input id="act-delay" className="input" inputMode="numeric" value={delay} onChange={(e) => setDelay(e.target.value)} placeholder="0" /></div>
        )}
        {type === 'restart_services' && (
          <div><label className="label" htmlFor="act-svc">Services (comma separated)</label>
            <input id="act-svc" className="input font-mono" value={services} onChange={(e) => setServices(e.target.value)} placeholder="NiSvcLoc, nimdnsResponder" /></div>
        )}
        {type === 'custom_script' && (
          <div><label className="label" htmlFor="act-script">Script</label>
            <textarea id="act-script" className="input min-h-[96px] py-2 font-mono" value={script} onChange={(e) => setScript(e.target.value)} /></div>
        )}
        {invalid && (type !== 'power_cycle' || delay.trim()) && <p className="mt-1 text-xs text-crit">{invalid}</p>}
      </div>
      {error && <p role="alert" className="mt-3 rounded-md border border-crit/50 bg-crit/10 px-3 py-2 text-[13px]"><strong>Not sent:</strong> {error}</p>}
    </Modal>
  )
}
