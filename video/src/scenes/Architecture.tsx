import React from 'react'
import { AbsoluteFill, interpolate, useCurrentFrame } from 'remotion'
import { C, FONT, MONO, W } from '../theme'
import { Background, FadeUp, Headline, Icon, Kicker, useProgress, useSpring } from '../ui'
import { Sfx } from '../sfx'

type Pt = { x: number; y: number }

function bez(a: Pt, b: Pt, t: number): Pt {
  const dx = (b.x - a.x) * 0.5
  const p1 = { x: a.x + dx, y: a.y }
  const p2 = { x: b.x - dx, y: b.y }
  const u = 1 - t
  return {
    x: u * u * u * a.x + 3 * u * u * t * p1.x + 3 * u * t * t * p2.x + t * t * t * b.x,
    y: u * u * u * a.y + 3 * u * u * t * p1.y + 3 * u * t * t * p2.y + t * t * t * b.y,
  }
}

function pathD(a: Pt, b: Pt): string {
  const dx = (b.x - a.x) * 0.5
  return `M${a.x} ${a.y} C${a.x + dx} ${a.y} ${b.x - dx} ${b.y} ${b.x} ${b.y}`
}

function approxLen(a: Pt, b: Pt): number {
  let len = 0
  let prev = a
  for (let i = 1; i <= 40; i++) {
    const p = bez(a, b, i / 40)
    len += Math.hypot(p.x - prev.x, p.y - prev.y)
    prev = p
  }
  return len
}

// Layout (px, 1920x1080 canvas)
const HW = { x: 90, w: 330, ys: [372, 532, 692], h: 116 }
const EDGE = { x: 520, w: 380, ys: [362, 640], h: 236 }
const HUB = { x: 1020, y: 352, w: 390, h: 530 }
const OUT = { x: 1530, w: 330, ys: [352, 492, 632, 772], h: 108 }

const mid = (y: number, h: number) => y + h / 2
const hwOut = (i: number): Pt => ({ x: HW.x + HW.w, y: mid(HW.ys[i], HW.h) })
const edgeIn = (i: number): Pt => ({ x: EDGE.x, y: mid(EDGE.ys[i], EDGE.h) })
const edgeOut = (i: number): Pt => ({ x: EDGE.x + EDGE.w, y: mid(EDGE.ys[i], EDGE.h) })
const hubIn = (i: number): Pt => ({ x: HUB.x, y: HUB.y + 190 + i * 150 })
const hubOut: Pt = { x: HUB.x + HUB.w, y: HUB.y + HUB.h / 2 }
const outIn = (i: number): Pt => ({ x: OUT.x, y: mid(OUT.ys[i], OUT.h) })

const LINKS = [
  { a: hwOut(0), b: edgeIn(0), color: C.ok, start: 30, flow: 62 },
  { a: hwOut(1), b: edgeIn(0), color: C.ok, start: 34, flow: 62 },
  { a: hwOut(1), b: edgeIn(1), color: C.ok, start: 38, flow: 62 },
  { a: hwOut(2), b: edgeIn(1), color: C.ok, start: 42, flow: 62 },
  { a: edgeOut(0), b: hubIn(0), color: C.info, start: 74, flow: 104 },
  { a: edgeOut(1), b: hubIn(1), color: C.info, start: 78, flow: 104 },
  ...[0, 1, 2, 3].map((i) => ({ a: hubOut, b: outIn(i), color: C.violet, start: 122 + i * 4, flow: 156 })),
]

const Box: React.FC<{
  x: number
  y: number
  w: number
  h: number
  delay: number
  color: string
  children: React.ReactNode
}> = ({ x, y, w, h, delay, color, children }) => {
  const s = useSpring(delay)
  return (
    <div
      style={{
        position: 'absolute',
        left: x,
        top: y,
        width: w,
        height: h,
        borderRadius: 20,
        background: 'linear-gradient(180deg, #1b1c1f, #141517)',
        border: `1.5px solid ${color}66`,
        boxShadow: `0 16px 50px rgba(0,0,0,0.5), 0 0 40px ${color}1c`,
        opacity: s,
        transform: `translateY(${(1 - s) * 40}px) scale(${0.92 + 0.08 * s})`,
        padding: '18px 22px',
        display: 'flex',
        flexDirection: 'column',
        justifyContent: 'center',
        gap: 8,
      }}
    >
      {children}
    </div>
  )
}

const BoxTitle: React.FC<{ icon: string; color: string; children: React.ReactNode }> = ({ icon, color, children }) => (
  <div style={{ display: 'flex', alignItems: 'center', gap: 14, fontFamily: FONT, fontSize: 27, fontWeight: 700, color: C.t1, whiteSpace: 'nowrap' }}>
    <Icon name={icon} size={32} color={color} />
    {children}
  </div>
)

const Line: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div style={{ fontFamily: MONO, fontSize: 17, color: C.t2, paddingLeft: 46, whiteSpace: 'nowrap' }}>{children}</div>
)

/** Pinned above the Edge↔Hub gap (the gap itself is too narrow for a label). */
const RoundTripTag: React.FC<{ color: string; bg: string; text: string }> = ({ color, bg, text }) => (
  <div
    style={{
      position: 'absolute',
      left: (EDGE.x + EDGE.w + HUB.x) / 2,
      top: 244,
      transform: 'translateX(-50%)',
      whiteSpace: 'nowrap',
      fontFamily: MONO,
      fontSize: 20,
      padding: '6px 14px',
      borderRadius: 10,
      background: bg,
      border: `1.5px solid ${color}`,
      color,
      boxShadow: `0 8px 30px rgba(0,0,0,0.6), 0 0 24px ${color}44`,
    }}
  >
    {text}
  </div>
)

export const Architecture: React.FC = () => {
  const frame = useCurrentFrame()
  const hubRows = [
    ['bell', C.warn, 'Rules engine'],
    ['alert', C.crit, 'Alert lifecycle'],
    ['wrench', C.ok, 'Action executor'],
    ['database', C.info, 'SQLite · WAL'],
    ['globe', C.violet, 'REST + WebSocket API'],
  ] as const

  // Self-healing round trip on the Station A link
  const cmd = interpolate(frame, [212, 246], [1, 0], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' })
  const res = interpolate(frame, [256, 290], [0, 1], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' })
  const link = LINKS[4]
  const cmdPt = bez(link.a, link.b, cmd)
  const resPt = bez(link.a, link.b, res)
  const cmdVis = frame >= 208 && frame < 250
  const resVis = frame >= 252 && frame < 296

  return (
    <AbsoluteFill>
      <Background glow={C.info} glow2={C.violet} />
      {[10, 16, 22, 48, 58, 88, 132, 139, 146, 153].map((f) => <Sfx key={f} at={f} name="pop" volume={0.3} />)}<Sfx at={208} name="tick" volume={0.45} /><Sfx at={252} name="chime" volume={0.45} />
      <div style={{ position: 'absolute', left: 100, top: 90, display: 'flex', flexDirection: 'column', gap: 18 }}>
        <Kicker>How it works</Kicker>
        <Headline size={64}>
          Edge <span style={{ color: C.t3 }}>→</span> Hub <span style={{ color: C.t3 }}>→</span> everywhere
        </Headline>
      </div>

      <svg width={W} height={1080} style={{ position: 'absolute', inset: 0 }}>
        {LINKS.map((l, i) => {
          const len = approxLen(l.a, l.b)
          const draw = interpolate(frame, [l.start, l.start + 28], [0, 1], {
            extrapolateLeft: 'clamp',
            extrapolateRight: 'clamp',
          })
          return (
            <path
              key={i}
              d={pathD(l.a, l.b)}
              fill="none"
              stroke={l.color}
              strokeOpacity={0.55}
              strokeWidth={2.5}
              strokeDasharray={len}
              strokeDashoffset={len * (1 - draw)}
            />
          )
        })}
        {LINKS.map((l, i) =>
          frame < l.flow
            ? null
            : [0, 0.5].map((phase) => {
                const t = (((frame - l.flow) / 48 + phase + i * 0.13) % 1 + 1) % 1
                const p = bez(l.a, l.b, t)
                const fadeIn = interpolate(frame, [l.flow, l.flow + 10], [0, 1], { extrapolateRight: 'clamp' })
                return (
                  <circle
                    key={`${i}-${phase}`}
                    cx={p.x}
                    cy={p.y}
                    r={6}
                    fill={l.color}
                    opacity={fadeIn * Math.sin(t * Math.PI)}
                    style={{ filter: `drop-shadow(0 0 8px ${l.color})` }}
                  />
                )
              }),
        )}
        {cmdVis && <circle cx={cmdPt.x} cy={cmdPt.y} r={11} fill={C.warn} style={{ filter: `drop-shadow(0 0 14px ${C.warn})` }} />}
        {resVis && <circle cx={resPt.x} cy={resPt.y} r={11} fill={C.ok} style={{ filter: `drop-shadow(0 0 14px ${C.ok})` }} />}
      </svg>

      {/* link labels */}
      <FadeUp delay={40} style={{ position: 'absolute', left: 430, top: 300 }}>
        <span style={{ fontFamily: MONO, fontSize: 17, color: C.ok }}>health sweep</span>
      </FadeUp>
      <FadeUp delay={84} style={{ position: 'absolute', left: 900, top: 300 }}>
        <span style={{ fontFamily: MONO, fontSize: 17, color: C.info }}>WebSocket 1.1 · wss</span>
      </FadeUp>
      <FadeUp delay={130} style={{ position: 'absolute', left: 1414, top: 300 }}>
        <span style={{ fontFamily: MONO, fontSize: 17, color: C.violet }}>live API</span>
      </FadeUp>

      {[
        ['cpu', 'PXI chassis'],
        ['cpu', 'CompactDAQ'],
        ['activity', 'VISA instruments'],
      ].map(([icon, label], i) => (
        <Box key={label} x={HW.x} y={HW.ys[i]} w={HW.w} h={HW.h} delay={10 + i * 6} color={C.ok}>
          <BoxTitle icon={icon} color={C.ok}>
            {label}
          </BoxTitle>
        </Box>
      ))}

      {['Station A', 'Station B'].map((s, i) => (
        <Box key={s} x={EDGE.x} y={EDGE.ys[i]} w={EDGE.w} h={EDGE.h} delay={48 + i * 10} color={C.info}>
          <BoxTitle icon="server" color={C.info}>
            Edge · {s}
          </BoxTitle>
          <Line>NI-SysCfg · DAQmx · VISA</Line>
          <Line>per-device prediction</Line>
          <Line>offline buffer · actions</Line>
          <Line>beside LabVIEW & TestStand</Line>
        </Box>
      ))}

      <Box x={HUB.x} y={HUB.y} w={HUB.w} h={HUB.h} delay={88} color={C.violet}>
        <div style={{ fontFamily: FONT, fontSize: 40, fontWeight: 800, color: C.t1, marginBottom: 12 }}>NIMon Hub</div>
        {hubRows.map(([icon, color, label], i) => {
          const p = useProgress(98 + i * 5, 18)
          return (
            <div
              key={label}
              style={{
                opacity: p,
                transform: `translateX(${(1 - p) * 20}px)`,
                display: 'flex',
                alignItems: 'center',
                gap: 16,
                padding: '14px 16px',
                borderRadius: 14,
                background: 'rgba(255,255,255,0.04)',
                border: `1px solid ${C.line}`,
                fontFamily: FONT,
                fontSize: 25,
                color: C.t1,
              }}
            >
              <Icon name={icon} size={28} color={color} />
              {label}
            </div>
          )
        })}
      </Box>

      {[
        ['globe', 'Web dashboard'],
        ['monitor', 'Desktop widget'],
        ['terminal', 'nimon-cli'],
        ['mail', 'Email · webhook'],
      ].map(([icon, label], i) => (
        <Box key={label} x={OUT.x} y={OUT.ys[i]} w={OUT.w} h={OUT.h} delay={132 + i * 7} color={C.violet}>
          <BoxTitle icon={icon} color={C.violet}>
            {label}
          </BoxTitle>
        </Box>
      ))}

      {cmdVis && <RoundTripTag color={C.warn} bg="#2b2413" text="hub → edge: execute_action · power_cycle" />}
      {resVis && <RoundTripTag color={C.ok} bg="#10281a" text="edge → hub: action_result ✓ 0.1 s" />}

      <FadeUp delay={176} style={{ position: 'absolute', left: 0, right: 0, top: 950, textAlign: 'center' }}>
        <span style={{ fontFamily: FONT, fontSize: 30, color: C.t2 }}>
          Edges run passively on each test station. <span style={{ color: C.t1 }}>One hub watches the whole fleet.</span>
        </span>
      </FadeUp>
    </AbsoluteFill>
  )
}
