import React from 'react'
import { AbsoluteFill, interpolate, useCurrentFrame } from 'remotion'
import { C, FONT, MONO } from '../theme'
import { Background, Card, FadeUp, Headline, Icon, Kicker, useProgress, useSpring } from '../ui'
import { Sfx } from '../sfx'

const CW = 1160
const CH = 640
const X0 = 96
const X1 = CW - 60
const YT = 70
const YB = CH - 90
const VMIN = 40
const VMAX = 85
const NOW = 0.72

const xOf = (t: number) => X0 + t * (X1 - X0)
const yOf = (v: number) => YB - ((v - VMIN) / (VMAX - VMIN)) * (YB - YT)

function temp(t: number): number {
  const noise = Math.sin(t * 91) * 0.55 + Math.sin(t * 37) * 0.45 + Math.sin(t * 13) * 0.3
  const ramp = Math.max(0, (t - 0.32) / 0.4) * 24
  return 47 + ramp + noise
}

const SLOPE = 24 / 0.4 // °C per unit t
const CROSS = NOW + (75 - (47 + 24)) / SLOPE

const MODELS = [
  { icon: 'gauge', color: C.info, title: 'Threshold bands', body: 'Transition-aware, so no alert storms' },
  { icon: 'activity', color: C.violet, title: 'EWMA anomaly', body: "Learns each module's normal baseline" },
  { icon: 'trend', color: C.warn, title: 'Trend forecast', body: 'Time-windowed slope gives an ETA to critical' },
]

export const Predict: React.FC = () => {
  const frame = useCurrentFrame()
  const card = useSpring(4)
  const prog = interpolate(frame, [14, 160], [0, 1], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' })
  const head = NOW * prog
  const proj = useProgress(160, 28)
  const badge = useSpring(182, 14)

  const pts: string[] = []
  const n = Math.max(2, Math.round(200 * prog))
  for (let i = 0; i <= n; i++) {
    const t = (head * i) / n
    pts.push(`${xOf(t).toFixed(1)},${yOf(temp(t)).toFixed(1)}`)
  }
  const line = `M${pts.join(' L')}`
  const area = `${line} L${xOf(head)},${YB} L${xOf(0)},${YB} Z`
  const hv = temp(head)
  const hc = hv >= 75 ? C.crit : hv >= 65 ? C.warn : C.ok

  const projEndT = NOW + (0.95 - NOW) * proj
  const projEndV = 47 + 24 + (projEndT - NOW) * SLOPE

  return (
    <AbsoluteFill>
      <Background glow={C.warn} glow2={C.info} />
      {[40, 56, 72].map((f) => <Sfx key={f} at={f} name="pop" volume={0.35} />)}<Sfx at={182} name="ping" volume={0.5} />
      <div style={{ position: 'absolute', left: 100, top: 100, display: 'flex', flexDirection: 'column', gap: 18 }}>
        <Kicker color={C.warn}>Prediction</Kicker>
        <Headline size={66}>See failures coming.</Headline>
      </div>

      <div style={{ position: 'absolute', left: 100, top: 300, opacity: card, transform: `translateY(${(1 - card) * 50}px)` }}>
        <Card style={{ width: CW, height: CH, position: 'relative' }}>
          <div style={{ position: 'absolute', left: 36, top: 26, display: 'flex', gap: 22, alignItems: 'baseline' }}>
            <span style={{ fontFamily: FONT, fontSize: 26, fontWeight: 700, color: C.t1 }}>PXIe-6368 · Slot 2</span>
            <span style={{ fontFamily: MONO, fontSize: 30, fontWeight: 700, color: hc }}>{hv.toFixed(1)} °C</span>
          </div>
          <svg width={CW} height={CH} style={{ position: 'absolute', inset: 0 }}>
            <defs>
              <linearGradient id="area" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0" stopColor={C.info} stopOpacity="0.35" />
                <stop offset="1" stopColor={C.info} stopOpacity="0" />
              </linearGradient>
            </defs>
            {[40, 50, 60, 70, 80].map((v) => (
              <g key={v}>
                <line x1={X0} x2={X1} y1={yOf(v)} y2={yOf(v)} stroke={C.line} />
                <text x={X0 - 18} y={yOf(v) + 7} fill={C.t3} fontFamily={MONO} fontSize={18} textAnchor="end">
                  {v}
                </text>
              </g>
            ))}
            {[
              [65, C.warn, 'warn 65 °C'],
              [75, C.crit, 'crit 75 °C'],
            ].map(([v, col, label]) => (
              <g key={label as string}>
                <line x1={X0} x2={X1} y1={yOf(v as number)} y2={yOf(v as number)} stroke={col as string} strokeWidth={2} strokeDasharray="10 8" opacity={0.85} />
                <text x={X1} y={yOf(v as number) - 10} fill={col as string} fontFamily={MONO} fontSize={18} textAnchor="end">
                  {label}
                </text>
              </g>
            ))}
            <line x1={xOf(NOW)} x2={xOf(NOW)} y1={YT - 10} y2={YB} stroke={C.lineHi} strokeDasharray="4 6" opacity={prog > 0.98 ? 1 : 0} />
            {[
              [0, '−20 min'],
              [0.36, '−10 min'],
              [NOW, 'now'],
              [1, '+8 min'],
            ].map(([t, l]) => (
              <text key={l as string} x={xOf(t as number)} y={YB + 38} fill={C.t3} fontFamily={MONO} fontSize={18} textAnchor="middle">
                {l}
              </text>
            ))}
            <path d={area} fill="url(#area)" />
            <path d={line} fill="none" stroke={C.info} strokeWidth={4} strokeLinejoin="round" style={{ filter: `drop-shadow(0 0 8px ${C.info}88)` }} />
            {proj > 0 && (
              <path
                d={`M${xOf(NOW)},${yOf(temp(NOW))} L${xOf(projEndT)},${yOf(projEndV)}`}
                stroke={C.warn}
                strokeWidth={4}
                strokeDasharray="3 11"
                strokeLinecap="round"
              />
            )}
            {projEndT >= CROSS && (
              <g opacity={interpolate(projEndT, [CROSS, CROSS + 0.03], [0, 1], { extrapolateRight: 'clamp' })}>
                <line x1={xOf(CROSS)} x2={xOf(CROSS)} y1={yOf(75)} y2={YB} stroke={C.crit} strokeDasharray="4 6" />
                <circle cx={xOf(CROSS)} cy={yOf(75)} r={10} fill={C.crit} style={{ filter: `drop-shadow(0 0 10px ${C.crit})` }} />
              </g>
            )}
            <circle cx={xOf(head)} cy={yOf(hv)} r={9} fill={hc} stroke="#000" strokeWidth={2} style={{ filter: `drop-shadow(0 0 10px ${hc})` }} />
          </svg>

          <div
            style={{
              position: 'absolute',
              left: 130,
              top: 300,
              opacity: badge,
              transform: `scale(${0.85 + 0.15 * badge})`,
              transformOrigin: 'top center',
              padding: '18px 24px',
              borderRadius: 16,
              background: '#2a2312',
              border: `2px solid ${C.warn}`,
              boxShadow: `0 0 40px ${C.warn}44`,
              display: 'flex',
              gap: 16,
              alignItems: 'center',
            }}
          >
            <Icon name="alert" size={38} color={C.warn} />
            <div>
              <div style={{ fontFamily: FONT, fontSize: 26, fontWeight: 700, color: C.t1 }}>Overheating predicted</div>
              <div style={{ fontFamily: MONO, fontSize: 20, color: C.warn, marginTop: 4 }}>91% · ETA ~2 min · trend-v2</div>
            </div>
          </div>
        </Card>
      </div>

      <div style={{ position: 'absolute', left: 1330, top: 300, width: 500, display: 'flex', flexDirection: 'column', gap: 22 }}>
        {MODELS.map((m, i) => {
          const s = useSpring(40 + i * 16)
          return (
            <Card
              key={m.title}
              style={{
                padding: '24px 26px',
                display: 'flex',
                gap: 20,
                alignItems: 'center',
                opacity: s,
                transform: `translateX(${(1 - s) * 60}px)`,
              }}
            >
              <div style={{ width: 62, height: 62, borderRadius: 16, background: `${m.color}1f`, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                <Icon name={m.icon} size={34} color={m.color} />
              </div>
              <div>
                <div style={{ fontFamily: FONT, fontSize: 28, fontWeight: 700, color: C.t1 }}>{m.title}</div>
                <div style={{ fontFamily: FONT, fontSize: 21, color: C.t2, marginTop: 4 }}>{m.body}</div>
              </div>
            </Card>
          )
        })}
        <FadeUp delay={96}>
          <div style={{ fontFamily: MONO, fontSize: 21, color: C.t3, lineHeight: 1.6, marginTop: 10 }}>
            Runs on the edge, per device, every sweep.
            <br />
            Predictions above threshold raise alerts.
          </div>
        </FadeUp>
      </div>
    </AbsoluteFill>
  )
}
