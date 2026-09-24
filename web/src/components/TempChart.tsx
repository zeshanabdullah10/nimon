import { memo, useEffect, useId, useMemo, useRef, useState } from 'react'
import type { MetricPoint, Thresholds } from '@/lib/api'
import { parseTs } from '@/lib/format'

const H = 200
const PAD = { l: 38, r: 10, t: 10, b: 22 }

/** Downsample to ≤ n points keeping each bucket's min and max (spikes survive). */
function envelope(pts: [number, number][], n: number): [number, number][] {
  if (pts.length <= n) return pts
  const size = Math.ceil(pts.length / (n / 2))
  const out: [number, number][] = []
  for (let i = 0; i < pts.length; i += size) {
    let lo = pts[i], hi = pts[i]
    for (let j = i; j < Math.min(i + size, pts.length); j++) {
      if (pts[j][1] < lo[1]) lo = pts[j]
      if (pts[j][1] > hi[1]) hi = pts[j]
    }
    if (lo[0] <= hi[0]) out.push(lo, hi); else out.push(hi, lo)
  }
  return out
}

function niceTicks(lo: number, hi: number, count = 5): number[] {
  const span = hi - lo || 1
  const raw = span / count
  const mag = 10 ** Math.floor(Math.log10(raw))
  const step = [1, 2, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? raw
  const out: number[] = []
  for (let v = Math.ceil(lo / step) * step; v <= hi + 1e-9; v += step) out.push(+v.toFixed(6))
  return out
}

export const TempChart = memo(function TempChart({ points, thresholds, from, to }: {
  points: MetricPoint[]; thresholds: Thresholds; from: number; to: number
}) {
  const wrap = useRef<HTMLDivElement>(null)
  const [w, setW] = useState(560)
  const titleId = useId()
  useEffect(() => {
    const el = wrap.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(([e]) => setW(Math.max(260, Math.floor(e.contentRect.width))))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  const model = useMemo(() => {
    const pts: [number, number][] = []
    for (const p of points) {
      const t = parseTs(p.timestamp)
      if (t !== null && Number.isFinite(p.value)) pts.push([t, p.value])
    }
    const vals = pts.map((p) => p[1])
    const dMin = vals.length ? Math.min(...vals) : thresholds.temperature_warning - 20
    const dMax = vals.length ? Math.max(...vals) : thresholds.temperature_critical
    const yLo = Math.floor(Math.min(dMin, thresholds.temperature_warning) - 3)
    const yHi = Math.ceil(Math.max(dMax, thresholds.temperature_critical) + 3)
    // gap detection: break the line where samples are > 3× the median spacing apart
    const gaps = pts.slice(1).map((p, i) => p[0] - pts[i][0]).sort((a, b) => a - b)
    const median = gaps.length ? gaps[Math.floor(gaps.length / 2)] : 0
    const gapMs = Math.max(median * 3, 30_000)
    return { pts, dMin, dMax, yLo, yHi, gapMs, latest: vals.length ? vals[vals.length - 1] : null }
  }, [points, thresholds])

  const iw = w - PAD.l - PAD.r
  const ih = H - PAD.t - PAD.b
  const x = (t: number) => PAD.l + ((t - from) / (to - from || 1)) * iw
  const y = (v: number) => PAD.t + (1 - (v - model.yLo) / (model.yHi - model.yLo || 1)) * ih

  const path = useMemo(() => {
    const pts = envelope(model.pts.filter((p) => p[0] >= from && p[0] <= to), Math.max(60, iw))
    let d = ''
    let prev: number | null = null
    for (const [t, v] of pts) {
      d += `${prev === null || t - prev > model.gapMs ? 'M' : 'L'}${x(t).toFixed(1)},${y(v).toFixed(1)}`
      prev = t
    }
    return d
  }, [model, from, to, iw])

  const yTicks = niceTicks(model.yLo, model.yHi)
  const xTicks = useMemo(() => {
    const n = Math.max(2, Math.min(6, Math.floor(iw / 90)))
    return Array.from({ length: n + 1 }, (_, i) => from + ((to - from) * i) / n)
  }, [from, to, iw])
  const fmt = (t: number) => {
    const d = new Date(t)
    return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
  }
  const summary = model.pts.length
    ? `${model.pts.length} samples; min ${model.dMin.toFixed(1)} °C, max ${model.dMax.toFixed(1)} °C, latest ${model.latest!.toFixed(1)} °C. Warning ${thresholds.temperature_warning} °C, critical ${thresholds.temperature_critical} °C.`
    : 'No temperature samples in this range.'

  const line = (v: number, stroke: string, fill: string, label: string) => (
    <g>
      <line x1={PAD.l} x2={w - PAD.r} y1={y(v)} y2={y(v)} className={stroke} strokeWidth={1.2} strokeDasharray="5 4" />
      <text x={w - PAD.r - 2} y={y(v) - 4} textAnchor="end" className={fill} fontSize={10} fontWeight={600}>{label} {v}°</text>
    </g>
  )

  return (
    <figure ref={wrap} className="m-0 w-full">
      <svg width={w} height={H} role="img" aria-labelledby={titleId} className="block max-w-full">
        <title id={titleId}>Temperature history. {summary}</title>
        {yTicks.map((v) => (
          <g key={v}>
            <line x1={PAD.l} x2={w - PAD.r} y1={y(v)} y2={y(v)} className="stroke-fg/10" />
            <text x={PAD.l - 6} y={y(v) + 3.5} textAnchor="end" fontSize={10} className="fill-t2">{v}</text>
          </g>
        ))}
        {xTicks.map((t, i) => (
          <text key={t} x={x(t)} y={H - 6} fontSize={10} className="fill-t2"
            textAnchor={i === 0 ? 'start' : i === xTicks.length - 1 ? 'end' : 'middle'}>{fmt(t)}</text>
        ))}
        {line(thresholds.temperature_warning, 'stroke-warn', 'fill-warn', 'warn')}
        {line(thresholds.temperature_critical, 'stroke-crit', 'fill-crit', 'crit')}
        {path && <path d={path} fill="none" className="stroke-info" strokeWidth={1.8} strokeLinejoin="round" strokeLinecap="round" />}
        {!model.pts.length && <text x={PAD.l + iw / 2} y={PAD.t + ih / 2} textAnchor="middle" fontSize={12} className="fill-t2">No samples in this range</text>}
      </svg>
      <figcaption className="mt-1 text-xs text-t2">{summary}</figcaption>
    </figure>
  )
})
