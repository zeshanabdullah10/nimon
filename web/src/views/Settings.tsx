import { useState, type ReactNode } from 'react'
import { INTERVALS, setToken, themeStore, useHealth, useSettings, useTheme, useToken, type ThemePref } from '@/hooks/nimon'
import { HIDDEN_FACTOR } from '@/lib/store'
import { Card, Empty, SevBadge, Updated } from '@/components/ui'
import { duration, pct } from '@/lib/format'
import { cn } from '@/lib/utils'

export function Settings() {
  const settings = useSettings()
  const health = useHealth()
  const s = settings.data
  const h = health.data
  const overrides = Object.entries(s?.edge_thresholds ?? {})
  return (
    <div className="grid grid-cols-1 gap-3.5 lg:grid-cols-2">
      <Card title="Hub" actions={<Updated at={health.updatedAt} stale={health.stale} error={health.error?.message} />}>
        {health.error && !h ? <Empty>Hub health unavailable — {health.error.message}</Empty> : (
          <KV rows={[
            ['Endpoint', location.host || '—'],
            ['Health', h ? <SevBadge sev={h.status === 'healthy' ? 'ok' : 'warning'} label={h.status} /> : '—'],
            ['Database', h ? (h.db_ok ? 'ok' : 'unavailable') : '—'],
            ['Alert manager', h ? (h.alert_manager_ok ? 'ok' : 'unavailable') : '—'],
            ['Edges connected', h ? String(h.edges_connected) : '—'],
            ['Hub uptime', h ? duration(h.uptime_secs) : '—'],
            ['Hub version', h?.version ?? s?.version ?? '—'],
          ]} />
        )}
      </Card>

      <Card title="Alerting" actions={<Updated at={settings.updatedAt} stale={settings.stale} error={settings.error?.message} />}>
        {!s ? <Empty>{settings.error ? `Settings unavailable — ${settings.error.message}` : 'Loading…'}</Empty> : (
          <KV rows={[
            ['Temperature warning', `≥ ${s.thresholds.temperature_warning} °C`],
            ['Temperature critical', `≥ ${s.thresholds.temperature_critical} °C`],
            ['Prediction alert threshold', pct(s.prediction_alert_threshold)],
            ['Edge offline after', `${s.edge_offline_after_secs} s without a report`],
            ['Writes require token', s.auth.writes_require_token ? 'yes' : 'no'],
          ]} />
        )}
      </Card>

      <Card title="Per-edge thresholds" className="lg:col-span-2">
        {overrides.length === 0 ? <Empty>No edges known</Empty> : (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[480px] text-left text-xs">
              <caption className="sr-only">Effective thresholds per edge</caption>
              <thead className="text-t2"><tr className="border-b border-fg/10">
                {['Edge', 'Warning', 'Critical', 'Poll interval', ''].map((x) => <th key={x} scope="col" className="px-2 py-2 font-semibold">{x}</th>)}
              </tr></thead>
              <tbody>
                {overrides.map(([id, t]) => {
                  const custom = s && (t.temperature_warning !== s.thresholds.temperature_warning || t.temperature_critical !== s.thresholds.temperature_critical || t.poll_interval_secs !== null)
                  return (
                    <tr key={id} className="border-b border-fg/[0.06]">
                      <td className="px-2 py-2 font-mono">{id}</td>
                      <td className="px-2 py-2">{t.temperature_warning} °C</td>
                      <td className="px-2 py-2">{t.temperature_critical} °C</td>
                      <td className="px-2 py-2">{t.poll_interval_secs === null ? 'edge default' : `${t.poll_interval_secs} s`}</td>
                      <td className="px-2 py-2 text-t2">{custom ? 'override' : 'hub default'}</td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
        )}
        <p className="mt-2 text-xs text-t2">Change these per edge under Edges → Configure.</p>
      </Card>

      <TokenCard required={s?.auth.writes_require_token ?? null} />

      <Card title="Dashboard">
        <ThemeChoice />
        <KV rows={[
          ['Alerts & modules poll', `${INTERVALS.alerts / 1000} s`],
          ['Hub health poll', `${INTERVALS.health / 1000} s`],
          ['Predictions poll', `${INTERVALS.predictions / 1000} s`],
          ['Edges & settings poll', `${INTERVALS.edges / 1000} s`],
          ['Background tab', `polling slowed ×${HIDDEN_FACTOR}`],
        ]} />
      </Card>
    </div>
  )
}

function KV({ rows }: { rows: [string, ReactNode][] }) {
  return (
    <dl className="text-[13px]">
      {rows.map(([k, v]) => (
        <div key={k} className="flex items-center justify-between gap-3 border-b border-fg/[0.06] py-2 last:border-0">
          <dt className="text-t2">{k}</dt>
          <dd className="text-right font-mono text-xs">{v}</dd>
        </div>
      ))}
    </dl>
  )
}

function TokenCard({ required }: { required: boolean | null }) {
  const token = useToken()
  const [value, setValue] = useState('')
  const [shown, setShown] = useState(false)
  return (
    <Card title="API token">
      <p className="mb-2 text-[13px]">
        {required === null ? 'Unknown whether this hub requires a token.' : required
          ? 'This hub requires a bearer token for every change (acknowledge, resolve, device actions, edge config).'
          : 'This hub accepts changes without a token.'}
      </p>
      <p className="mb-3 text-xs text-t2">
        Stored token: {token ? <><code className="font-mono">{shown ? token : '•'.repeat(Math.min(12, token.length))}</code>{' '}
          <button type="button" className="underline" onClick={() => setShown((x) => !x)}>{shown ? 'hide' : 'show'}</button></> : 'none'}
      </p>
      <form className="flex flex-wrap gap-2" onSubmit={(e) => { e.preventDefault(); if (value.trim()) { setToken(value); setValue('') } }}>
        <label htmlFor="set-token" className="sr-only">API token</label>
        <input id="set-token" className="input min-w-0 flex-1 font-mono" type="password" autoComplete="off" placeholder="Paste token" value={value} onChange={(e) => setValue(e.target.value)} />
        <button type="submit" className="btn btn-primary" disabled={!value.trim()}>Save</button>
        <button type="button" className="btn" disabled={!token} onClick={() => setToken(null)}>Clear</button>
      </form>
    </Card>
  )
}

function ThemeChoice() {
  const pref = useTheme()
  const opts: [ThemePref, string][] = [['system', 'System'], ['dark', 'Dark'], ['light', 'Light']]
  return (
    <fieldset className="mb-2">
      <legend className="label">Theme</legend>
      <div className="flex gap-1.5">
        {opts.map(([v, l]) => (
          <label key={v} className={cn('btn btn-sm cursor-pointer has-[:focus-visible]:outline has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-info', pref === v && 'border-info/60 bg-info/15')}>
            <input type="radio" name="theme" className="sr-only" checked={pref === v} onChange={() => themeStore.set(v)} />{l}
          </label>
        ))}
      </div>
    </fieldset>
  )
}
