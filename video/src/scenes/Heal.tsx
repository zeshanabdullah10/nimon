import React from 'react'
import { AbsoluteFill, interpolate, useCurrentFrame } from 'remotion'
import { C, FONT, MONO } from '../theme'
import { Background, BrowserFrame, FadeUp, Headline, Icon, Kicker, Pill, Shot, useSpring } from '../ui'
import { Sfx } from '../sfx'

const STEPS = [
  { at: 34, icon: 'bell', color: C.crit, title: 'Rule fires', a: 'Critical Temperature', b: 'PXI1Slot2 · 76.0 °C ≥ 75' },
  { at: 74, icon: 'server', color: C.info, title: 'Hub dispatches', a: 'execute_action', b: 'power_cycle → Station A' },
  { at: 114, icon: 'cpu', color: C.violet, title: 'Edge executes', a: 'allowlisted · locked', b: 'timeout · process-tree kill' },
  { at: 154, icon: 'check', color: C.ok, title: 'Result returns', a: 'action_result', b: 'correlated by reply_to' },
  { at: 190, icon: 'database', color: C.info, title: 'Recorded', a: 'action history', b: 'linked to the alert' },
]

const CW = 310
const GAP = 38
const X0 = 120
const Y = 340

export const Heal: React.FC = () => {
  const frame = useCurrentFrame()
  const active = STEPS.reduce((acc, s, i) => (frame >= s.at ? i : acc), -1)
  const trackX0 = X0 + CW / 2
  const trackX1 = X0 + 4 * (CW + GAP) + CW / 2
  const packetX = interpolate(
    frame,
    STEPS.map((s) => s.at),
    STEPS.map((_, i) => trackX0 + i * (CW + GAP)),
    { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' },
  )
  const ms = interpolate(frame, [STEPS[0].at, STEPS[3].at], [0, 100], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' })
  const shot = useSpring(214)

  return (
    <AbsoluteFill>
      <Background glow={C.ok} glow2={C.violet} />
      <Sfx at={34} name="ping" volume={0.45} /><Sfx at={74} name="tick" volume={0.45} /><Sfx at={114} name="tick" volume={0.45} /><Sfx at={154} name="chime" volume={0.55} /><Sfx at={190} name="pop" volume={0.35} /><Sfx at={214} name="pop" volume={0.3} />
      <div style={{ position: 'absolute', left: X0, top: 90, display: 'flex', flexDirection: 'column', gap: 18 }}>
        <Kicker color={C.ok}>Self-healing</Kicker>
        <Headline size={64}>From alert to fix, automatically.</Headline>
      </div>

      <FadeUp delay={30} style={{ position: 'absolute', right: 120, top: 150, textAlign: 'right' }}>
        <div style={{ fontFamily: MONO, fontSize: 20, color: C.t3, letterSpacing: 2 }}>ROUND TRIP</div>
        <div style={{ fontFamily: MONO, fontSize: 64, fontWeight: 700, color: frame >= STEPS[3].at ? C.ok : C.t1 }}>
          {ms.toFixed(0).padStart(3, '0')} ms
        </div>
      </FadeUp>

      <svg width={1920} height={1080} style={{ position: 'absolute', inset: 0 }}>
        <line x1={trackX0} x2={trackX1} y1={Y - 34} y2={Y - 34} stroke={C.lineHi} strokeWidth={3} strokeDasharray="2 10" strokeLinecap="round" />
        <line x1={trackX0} x2={packetX} y1={Y - 34} y2={Y - 34} stroke={C.ok} strokeWidth={4} strokeLinecap="round" />
        {active >= 0 && (
          <circle cx={packetX} cy={Y - 34} r={12} fill={STEPS[active].color} style={{ filter: `drop-shadow(0 0 14px ${STEPS[active].color})` }} />
        )}
      </svg>

      {STEPS.map((s, i) => {
        const on = i <= active
        const cur = i === active
        const pop = useSpring(s.at - 4, 14)
        return (
          <div
            key={s.title}
            style={{
              position: 'absolute',
              left: X0 + i * (CW + GAP),
              top: Y,
              width: CW,
              height: 250,
              borderRadius: 22,
              padding: '26px 24px',
              background: on ? `linear-gradient(180deg, ${s.color}1c, #141517)` : 'linear-gradient(180deg,#18191c,#131416)',
              border: `2px solid ${on ? s.color : C.line}`,
              boxShadow: cur ? `0 0 50px ${s.color}44` : 'none',
              transform: `scale(${on ? 0.96 + 0.04 * pop : 0.96})`,
              opacity: on ? 1 : 0.45,
              display: 'flex',
              flexDirection: 'column',
              gap: 12,
            }}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
              <span style={{ fontFamily: MONO, fontSize: 20, color: C.t3 }}>{i + 1}</span>
              <Icon name={s.icon} size={34} color={on ? s.color : C.t3} />
            </div>
            <div style={{ fontFamily: FONT, fontSize: 32, fontWeight: 700, color: C.t1 }}>{s.title}</div>
            <div style={{ fontFamily: MONO, fontSize: 20, color: on ? s.color : C.t3 }}>{s.a}</div>
            <div style={{ fontFamily: FONT, fontSize: 20, color: C.t2 }}>{s.b}</div>
          </div>
        )
      })}

      <div style={{ position: 'absolute', left: X0, top: 650, width: 820, display: 'flex', flexDirection: 'column', gap: 22 }}>
        <FadeUp delay={200}>
          <div style={{ fontFamily: FONT, fontSize: 28, color: C.t1, fontWeight: 600 }}>Actions you can wire to any rule</div>
        </FadeUp>
        <FadeUp delay={210}>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 12 }}>
            {['power_cycle', 'reset_driver', 'restart_services', 'custom_script', 'hub script', 'edge command'].map((a) => (
              <Pill key={a} color={C.ok}>
                {a}
              </Pill>
            ))}
          </div>
        </FadeUp>
        <FadeUp delay={222}>
          <div style={{ fontFamily: FONT, fontSize: 23, color: C.t2, lineHeight: 1.5 }}>
            Retries with backoff · never runs twice at once · <span style={{ color: C.t1 }}>on_failure</span> fallback ·
            one-click manual runs from the dashboard and CLI
          </div>
        </FadeUp>
      </div>

      <div style={{ position: 'absolute', left: 1010, top: 650, opacity: shot, transform: `translateY(${(1 - shot) * 40}px)` }}>
        <BrowserFrame width={790} height={300} url="Device · PXI1Slot2 · manual actions">
          <div style={{ width: 1756, transform: 'translate(-966px, -866px)' }}>
            <Shot name="device" />
          </div>
        </BrowserFrame>
      </div>
    </AbsoluteFill>
  )
}
