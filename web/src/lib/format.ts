/* Formatting helpers. Timestamps from the hub are RFC3339 (UTC). */

export function parseTs(ts?: string | null): number | null {
  if (!ts) return null
  let s = String(ts).trim().replace(' ', 'T')
  if (!/[zZ]$/.test(s) && !/[+-]\d{2}:?\d{2}$/.test(s)) s += 'Z'
  const t = Date.parse(s)
  return Number.isNaN(t) ? null : t
}

/* Browser-vs-hub clock offset, estimated from the HTTP Date header so
 * freshness checks don't break on a station with a skewed clock. */
let skewMs = 0
export function noteServerDate(dateHeader: string | null, sentAt: number, receivedAt: number) {
  if (!dateHeader) return
  const server = Date.parse(dateHeader)
  if (Number.isNaN(server)) return
  const local = (sentAt + receivedAt) / 2
  const est = server - local
  // Date has 1 s resolution: ignore offsets smaller than that
  skewMs = Math.abs(est) < 1500 ? 0 : est
}
/** "Now" on the hub's clock */
export const hubNow = () => Date.now() + skewMs

export function ago(ms: number | null, now = hubNow()): string {
  if (ms === null) return '—'
  const s = Math.max(0, Math.round((now - ms) / 1000))
  if (s < 5) return 'just now'
  if (s < 60) return `${s}s ago`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ago`
  const h = Math.floor(m / 60)
  if (h < 48) return `${h}h ${m % 60}m ago`
  return `${Math.floor(h / 24)}d ago`
}

export const agoTs = (ts?: string | null, now?: number) => ago(parseTs(ts), now)

const p2 = (n: number) => String(n).padStart(2, '0')

/** Local wall-clock time, with the date when it is not today */
export function clockTime(ms: number | null, withSeconds = true): string {
  if (ms === null) return '—'
  const d = new Date(ms)
  const t = `${p2(d.getHours())}:${p2(d.getMinutes())}${withSeconds ? ':' + p2(d.getSeconds()) : ''}`
  const today = new Date()
  if (d.toDateString() === today.toDateString()) return t
  return `${d.getFullYear()}-${p2(d.getMonth() + 1)}-${p2(d.getDate())} ${t}`
}

export const clockTs = (ts?: string | null, withSeconds = true) => clockTime(parseTs(ts), withSeconds)

export function duration(secs: number | null | undefined): string {
  if (secs === null || secs === undefined || !Number.isFinite(secs)) return '—'
  const s = Math.floor(secs)
  const d = Math.floor(s / 86400)
  const h = Math.floor((s % 86400) / 3600)
  const m = Math.floor((s % 3600) / 60)
  if (d > 0) return `${d}d ${h}h`
  if (h > 0) return `${h}h ${m}m`
  if (m > 0) return `${m}m ${s % 60}s`
  return `${s}s`
}

export const gb = (mb: number | null | undefined) =>
  mb === null || mb === undefined || !Number.isFinite(mb) ? '—' : `${(mb / 1024).toFixed(1)} GB`

export const temp = (v: number | null | undefined, digits = 1) =>
  v === null || v === undefined || !Number.isFinite(v) ? '—' : `${v.toFixed(digits)} °C`

export const pct = (p: number | null | undefined) =>
  p === null || p === undefined || !Number.isFinite(p) ? '—' : `${Math.round(p * 100)}%`

/** `edge:PXIe-6368#2` → { name: 'PXIe-6368', tag: '2' } */
export function deviceLabel(id: string, name?: string | null): { name: string; tag: string } {
  const i = id.lastIndexOf(':')
  const local = (name && name.trim()) || (i === -1 ? id : id.slice(i + 1))
  const h = local.lastIndexOf('#')
  return h === -1 ? { name: local, tag: '' } : { name: local.slice(0, h).trim(), tag: local.slice(h + 1) }
}

export const humanize = (s: string) =>
  s.replace(/([a-z])([A-Z])/g, '$1 $2').replace(/_/g, ' ').replace(/^\w/, (c) => c.toUpperCase())

export function metricText(v: unknown): string {
  if (typeof v === 'number') return Number.isInteger(v) ? String(v) : v.toFixed(2)
  if (typeof v === 'boolean') return v ? 'yes' : 'no'
  if (v === null || v === undefined || v === '') return '—'
  return String(v)
}

/** Numeric metrics whose key starts with `prefix` (e.g. `temperature[`) → [sensor, value] */
export function numMetricEntries(metrics: Record<string, unknown> | undefined, prefix: string): [string, number][] {
  if (!metrics) return []
  const out: [string, number][] = []
  for (const [k, v] of Object.entries(metrics)) {
    if (k.startsWith(prefix) && typeof v === 'number' && Number.isFinite(v)) {
      out.push([k.slice(prefix.length).replace(/\]$/, ''), v])
    }
  }
  return out
}
