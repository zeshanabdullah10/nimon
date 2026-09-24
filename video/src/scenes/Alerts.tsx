import React from 'react'
import { AbsoluteFill, interpolate, useCurrentFrame } from 'remotion'
import { C, FONT, MONO } from '../theme'
import { Background, BrowserFrame, Card, Headline, Icon, Kicker, Shot, useSpring } from '../ui'
import { Sfx } from '../sfx'

const STATES = [
  { name: 'Pending', color: C.info, at: 36, body: 'Condition must hold for its duration window' },
  { name: 'Firing', color: C.crit, at: 76, body: "Notify every channel and run the rule's action" },
  { name: 'Acknowledged', color: C.warn, at: 126, body: "Someone's on it: no repeat notifications" },
  { name: 'Resolved', color: C.ok, at: 176, body: 'Auto-resolves past hysteresis, even after restarts' },
]

const NODE_W = 360
const GAP = 70
const X_START = 135

const TOASTS = [
  { at: 86, icon: 'mail', color: C.crit, title: 'Email · Critical Temperature · PXI1Slot2', body: 'temperature 76.0 °C vs threshold 75 · Station A' },
  { at: 100, icon: 'zap', color: C.violet, title: 'Webhook · POST /hooks/nimon', body: '200 OK · alert.firing' },
  { at: 186, icon: 'check', color: C.ok, title: 'Resolved · PXI1Slot2', body: 'temperature back to 61.2 °C · channels notified' },
]

export const Alerts: React.FC = () => {
  const frame = useCurrentFrame()
  const active = STATES.reduce((acc, s, i) => (frame >= s.at ? i : acc), -1)
  const shot = useSpring(24)

  // Token that slides between state nodes
  // hold at node i, then glide to node i+1 over the 12 frames before it activates
  const inFrames = [STATES[0].at, ...STATES.slice(1).flatMap((s) => [s.at - 12, s.at])]
  const outIdx = [0, ...STATES.slice(1).flatMap((_, k) => [k, k + 1])]
  const tokenX = interpolate(
    frame,
    inFrames,
    outIdx.map((i) => X_START + i * (NODE_W + GAP) + NODE_W / 2),
    { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' },
  )

  return (
    <AbsoluteFill>
      <Background glow={C.crit} glow2={C.ok} />
      <Sfx at={36} name="tick" volume={0.4} /><Sfx at={76} name="ping" volume={0.45} /><Sfx at={86} name="pop" volume={0.35} /><Sfx at={100} name="pop" volume={0.35} /><Sfx at={126} name="tick" volume={0.45} /><Sfx at={176} name="chime" volume={0.45} /><Sfx at={186} name="pop" volume={0.3} />
      <div style={{ position: 'absolute', left: 135, top: 90, display: 'flex', flexDirection: 'column', gap: 18 }}>
        <Kicker color={C.crit}>Alert lifecycle</Kicker>
        <Headline size={64}>Alerts that open, and actually close.</Headline>
      </div>

      {STATES.map((s, i) => {
        const x = X_START + i * (NODE_W + GAP)
        const on = i === active
        const done = i < active
        const appear = useSpring(6 + i * 6)
        return (
          <React.Fragment key={s.name}>
            <div
              style={{
                position: 'absolute',
                left: x,
                top: 300,
                width: NODE_W,
                height: 120,
                borderRadius: 22,
                border: `2px solid ${on || done ? s.color : C.lineHi}`,
                background: on ? `${s.color}22` : 'linear-gradient(180deg,#1b1c1f,#141517)',
                boxShadow: on ? `0 0 50px ${s.color}55` : 'none',
                opacity: appear,
                transform: `translateY(${(1 - appear) * 30}px) scale(${on ? 1.04 : 1})`,
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                gap: 14,
                fontFamily: FONT,
                fontSize: 36,
                fontWeight: 700,
                color: on || done ? C.t1 : C.t3,
              }}
            >
              {done && <Icon name="check" size={30} color={s.color} stroke={3} />}
              {s.name}
            </div>
            <div
              style={{
                position: 'absolute',
                left: x,
                top: 440,
                width: NODE_W,
                textAlign: 'center',
                fontFamily: FONT,
                fontSize: 23,
                lineHeight: 1.35,
                color: on ? C.t1 : C.t3,
                opacity: appear,
              }}
            >
              {s.body}
            </div>
            {i < STATES.length - 1 && (
              <div
                style={{
                  position: 'absolute',
                  left: x + NODE_W + 12,
                  top: 338,
                  opacity: appear,
                  fontFamily: FONT,
                  fontSize: 40,
                  color: done ? STATES[i + 1].color : C.t3,
                }}
              >
                →
              </div>
            )}
          </React.Fragment>
        )
      })}

      {active >= 0 && (
        <div
          style={{
            position: 'absolute',
            left: tokenX - 9,
            top: 282,
            width: 18,
            height: 18,
            borderRadius: 9,
            background: STATES[active].color,
            boxShadow: `0 0 24px ${STATES[active].color}`,
          }}
        />
      )}

      <div style={{ position: 'absolute', left: 135, top: 600, width: 760, display: 'flex', flexDirection: 'column', gap: 18 }}>
        {TOASTS.map((t) => {
          const s = useSpring(t.at, 16)
          return (
            <Card
              key={t.title}
              glow={t.color}
              style={{
                padding: '20px 24px',
                display: 'flex',
                gap: 20,
                alignItems: 'center',
                opacity: s,
                transform: `translateX(${(1 - s) * -80}px)`,
              }}
            >
              <Icon name={t.icon} size={36} color={t.color} />
              <div>
                <div style={{ fontFamily: FONT, fontSize: 25, fontWeight: 700, color: C.t1 }}>{t.title}</div>
                <div style={{ fontFamily: MONO, fontSize: 19, color: C.t2, marginTop: 4 }}>{t.body}</div>
              </div>
            </Card>
          )
        })}
      </div>

      <div style={{ position: 'absolute', left: 960, top: 600, opacity: shot, transform: `translateY(${(1 - shot) * 50}px)` }}>
        <BrowserFrame width={860} height={390} url="127.0.0.1:9090/#/alerts">
          <div style={{ width: 860, transform: 'translate(-95px, -90px) scale(1.08)', transformOrigin: '0 0' }}>
            <Shot name="alerts" />
          </div>
        </BrowserFrame>
      </div>
    </AbsoluteFill>
  )
}
