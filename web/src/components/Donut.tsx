import { SEV_COLOR, sevPct } from '@/hooks/nimon'

/** Donut ring: track at idle gray, arc from 12 o'clock, clockwise */
export function Donut({
  pct, size = 76, stroke = 6,
}: { pct: number | null; size?: number; stroke?: number }) {
  const r = 35
  const CIRC = 219.9
  const p = pct === null ? 0 : Math.max(0, Math.min(100, pct))
  const color = pct === null ? SEV_COLOR.idle : SEV_COLOR[sevPct(pct)]

  return (
    <svg width={size} height={size} viewBox="0 0 76 76">
      <circle cx="38" cy="38" r={r} fill="none"
        stroke="rgba(255,255,255,0.28)" strokeWidth={stroke} opacity={0.45} />
      <circle
        cx="38" cy="38" r={r} fill="none"
        stroke={color}
        strokeWidth={stroke}
        strokeLinecap="round"
        strokeDasharray={CIRC}
        strokeDashoffset={CIRC * (1 - p / 100)}
        transform="rotate(-90 38 38)"
        style={{ transition: 'stroke-dashoffset 0.5s ease-out, stroke 0.25s' }}
      />
    </svg>
  )
}
