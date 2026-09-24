import React from 'react'
import { AbsoluteFill, interpolate, useCurrentFrame } from 'remotion'
import { C, FONT } from '../theme'
import { Background, BrowserFrame, easeInOut, Headline, Kicker, Shot, useSpring } from '../ui'
import { Sfx } from '../sfx'

const FW = 1160
const IMG_H = FW / 1.6 // screenshots are 2400x1500

// Camera keyframes over the screenshot: [frame, scale, tx, ty] in frame px
const CAM: [number, number, number, number][] = [
  [0, 1, 0, 0],
  [50, 1, 0, 0],
  [110, 1.65, -198, -66],
  [150, 1.65, -198, -66],
  [200, 1.35, -189, -254],
  [240, 1.35, -189, -254],
  [290, 1.9, -1064, -342],
  [330, 1.9, -1064, -342],
]

function cam(frame: number) {
  const fs = CAM.map((k) => k[0])
  const at = (i: number) =>
    interpolate(frame, fs, CAM.map((k) => k[i]), {
      extrapolateLeft: 'clamp',
      extrapolateRight: 'clamp',
      easing: easeInOut,
    })
  return { s: at(1), x: at(2), y: at(3) }
}

const CALLOUTS: { at: number; color: string; title: string; body: string }[] = [
  { at: 56, color: C.crit, title: 'One honest station status', body: 'OK · WARN · CRIT · STALE · HUB DOWN, with the reasons' },
  { at: 150, color: C.ok, title: 'Every module, every edge', body: 'Live temperatures against thresholds shared by hub, dashboard and widget' },
  { at: 196, color: C.info, title: 'Never misleading', body: 'Simulated devices tagged SIM, stale data greyed as last-known' },
  { at: 246, color: C.warn, title: 'Triage at a glance', body: 'Active alerts and predictions ranked by severity' },
]

export const Dashboard: React.FC = () => {
  const frame = useCurrentFrame()
  const enter = useSpring(6, 20)
  const c = cam(frame)
  const active = CALLOUTS.reduce((acc, co, i) => (frame >= co.at ? i : acc), -1)

  return (
    <AbsoluteFill>
      <Background glow={C.info} glow2={C.crit} />
      <Sfx at={8} name="tick" volume={0.3} />{[56, 150, 196, 246].map((f) => <Sfx key={f} at={f} name="tick" volume={0.4} />)}
      <div style={{ position: 'absolute', left: 100, top: 110, width: 560, display: 'flex', flexDirection: 'column', gap: 22 }}>
        <Kicker>Live monitoring</Kicker>
        <Headline size={66}>
          Every module.
          <br />
          <span style={{ color: C.t2 }}>One honest status.</span>
        </Headline>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 18, marginTop: 30 }}>
          {CALLOUTS.map((co, i) => {
            const seen = frame >= co.at
            const p = interpolate(frame, [co.at, co.at + 16], [0, 1], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' })
            const on = i === active
            return (
              <div
                key={co.title}
                style={{
                  opacity: seen ? (on ? 1 : 0.45) : 0,
                  transform: `translateX(${(1 - p) * -24}px)`,
                  display: 'flex',
                  gap: 18,
                  padding: '16px 20px',
                  borderRadius: 16,
                  background: on ? `${co.color}14` : 'transparent',
                  border: `1.5px solid ${on ? `${co.color}77` : 'transparent'}`,
                }}
              >
                <span style={{ width: 6, borderRadius: 3, background: co.color, flexShrink: 0 }} />
                <div>
                  <div style={{ fontFamily: FONT, fontSize: 28, fontWeight: 700, color: C.t1 }}>{co.title}</div>
                  <div style={{ fontFamily: FONT, fontSize: 22, color: C.t2, marginTop: 4, lineHeight: 1.35 }}>{co.body}</div>
                </div>
              </div>
            )
          })}
        </div>
      </div>

      <div
        style={{
          position: 'absolute',
          left: 680,
          top: 150,
          perspective: 2000,
        }}
      >
        <div
          style={{
            opacity: enter,
            transform: `translateX(${(1 - enter) * 160}px) rotateY(${(1 - enter) * -14}deg)`,
          }}
        >
          <BrowserFrame width={FW} height={IMG_H + 46}>
            <div
              style={{
                width: FW,
                transformOrigin: '0 0',
                transform: `translate(${c.x}px, ${c.y}px) scale(${c.s})`,
              }}
            >
              <Shot name="overview" />
            </div>
          </BrowserFrame>
        </div>
      </div>
    </AbsoluteFill>
  )
}
