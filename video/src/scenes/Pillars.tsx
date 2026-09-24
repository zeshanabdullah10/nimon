import React from 'react'
import { AbsoluteFill } from 'remotion'
import { C, FONT } from '../theme'
import { Background, Card, Headline, Icon, Kicker, LogoMark, useSpring } from '../ui'
import { Sfx } from '../sfx'

const PILLARS = [
  {
    icon: 'eye',
    color: C.info,
    title: 'Monitor',
    body: 'Live health of every PXI module, cDAQ chassis and instrument, from every station.',
  },
  {
    icon: 'trend',
    color: C.warn,
    title: 'Predict',
    body: 'Per-device models spot anomalies and forecast overheating before it happens.',
  },
  {
    icon: 'wrench',
    color: C.ok,
    title: 'Heal',
    body: 'Alert rules trigger safe, audited fixes: power cycle, driver reset, service restart.',
  },
]

export const Pillars: React.FC = () => {
  const logo = useSpring(0)
  return (
    <AbsoluteFill>
      <Background glow={C.ok} glow2={C.info} />
      <Sfx at={2} name="impact" volume={0.35} /><Sfx at={34} name="pop" volume={0.45} /><Sfx at={44} name="pop" volume={0.45} /><Sfx at={54} name="pop" volume={0.45} />
      <AbsoluteFill style={{ alignItems: 'center', justifyContent: 'center', flexDirection: 'column', gap: 70 }}>
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 24, textAlign: 'center' }}>
          <div style={{ transform: `scale(${0.7 + 0.3 * logo})`, opacity: logo }}>
            <LogoMark size={96} />
          </div>
          <Kicker color={C.ok}>Meet NIMon</Kicker>
          <Headline size={76} style={{ textAlign: 'center' }}>
            It watches your NI hardware,
            <br />
            <span style={{ color: C.ok }}>and fixes it before tests fail.</span>
          </Headline>
        </div>
        <div style={{ display: 'flex', gap: 40 }}>
          {PILLARS.map((p, i) => {
            const s = useSpring(34 + i * 10)
            return (
              <Card
                key={p.title}
                glow={p.color}
                style={{
                  width: 470,
                  padding: '38px 40px',
                  opacity: s,
                  transform: `translateY(${(1 - s) * 70}px) scale(${0.94 + 0.06 * s})`,
                  display: 'flex',
                  flexDirection: 'column',
                  gap: 18,
                }}
              >
                <div
                  style={{
                    width: 76,
                    height: 76,
                    borderRadius: 20,
                    background: `${p.color}1f`,
                    border: `1.5px solid ${p.color}66`,
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                  }}
                >
                  <Icon name={p.icon} size={42} color={p.color} />
                </div>
                <div style={{ fontFamily: FONT, fontSize: 44, fontWeight: 700, color: C.t1 }}>{p.title}</div>
                <div style={{ fontFamily: FONT, fontSize: 26, lineHeight: 1.45, color: C.t2 }}>{p.body}</div>
              </Card>
            )
          })}
        </div>
      </AbsoluteFill>
    </AbsoluteFill>
  )
}
