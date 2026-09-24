import { useMemo } from 'react'
import { useDevices, useEdgeFilter, usePredictions, useSettings } from '@/hooks/nimon'
import { href } from '@/hooks/route'
import { EdgeChips } from '@/components/EdgeChips'
import { Ago, Card, Empty, Meter, SevBadge, Updated } from '@/components/ui'
import { deviceLabel, humanize, parseTs, pct } from '@/lib/format'

export function Predictions() {
  const preds = usePredictions()
  const threshold = useSettings().data?.prediction_alert_threshold ?? null
  const filter = useEdgeFilter()
  const devices = useDevices().data
  const byId = useMemo(() => new Map((devices ?? []).map((d) => [d.device_id, d])), [devices])
  const list = useMemo(() => [...(preds.data ?? [])]
    .filter((p) => !filter || p.edge_id === filter)
    .sort((a, b) => b.probability - a.probability), [preds.data, filter])

  return (
    <div className="flex flex-col gap-3.5">
      <EdgeChips />
      <Card title={`Predictions · ${preds.data ? list.length : '—'}`}
        actions={<Updated at={preds.updatedAt} stale={preds.stale} error={preds.error?.message} />}>
        <p className="mb-3 text-xs text-t2">
          The hub raises an alert when a prediction reaches <strong className="text-t1">{threshold === null ? '—' : pct(threshold)}</strong> (prediction_alert_threshold). Lower-probability predictions are early indicators only.
        </p>
        {preds.error && <p role="alert" className="mb-2 rounded-md border border-warn/50 bg-warn/10 px-2 py-1.5 text-xs">Predictions unavailable — {preds.error.message}</p>}
        {preds.loading ? <Empty>Loading…</Empty> : list.length === 0 ? <Empty>No active predictions</Empty> : (
          <ul className="flex flex-col gap-2">
            {list.map((p) => {
              const above = threshold !== null && p.probability >= threshold
              const d = byId.get(p.device_id)
              const dl = deviceLabel(p.device_id, d?.name)
              return (
                <li key={p.id} className="row flex flex-wrap items-center gap-x-4 gap-y-2 p-3.5">
                  <div className="min-w-0 flex-[1_1_200px]">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span className="text-[15px] font-medium">{humanize(p.prediction_type)}</span>
                      {above ? <SevBadge sev="warning" label="Above alert threshold" /> : <SevBadge sev="unknown" label="Early indicator" />}
                    </div>
                    <div className="mt-1 text-xs text-t2">
                      <a className="underline underline-offset-2" href={href({ device: p.device_id })}>{dl.name}{dl.tag ? ` #${dl.tag}` : ''}</a>
                      {d?.slot != null ? ` · slot ${d.slot}` : ''} · {p.edge_id} · <Ago ms={parseTs(p.created_at)} />
                    </div>
                  </div>
                  <div className="flex min-w-[160px] flex-[1_1_220px] items-center gap-3">
                    <div className="min-w-0 flex-1"><Meter value={p.probability * 100} label={`${humanize(p.prediction_type)} probability`} sev={above ? 'warning' : 'unknown'} /></div>
                    <div className="w-[88px] shrink-0 text-right">
                      <div className="text-base font-bold">{pct(p.probability)}</div>
                      <div className="text-xs text-t2">{p.eta_minutes ? `ETA ~${p.eta_minutes} min` : 'no ETA'}</div>
                    </div>
                  </div>
                </li>
              )
            })}
          </ul>
        )}
      </Card>
    </div>
  )
}
