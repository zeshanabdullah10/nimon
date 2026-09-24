import React from 'react'
import { AbsoluteFill } from 'remotion'
import { C, FONT } from '../theme'
import { Background, Card, Headline, Icon, Kicker, useSpring } from '../ui'
import { Sfx } from '../sfx'

const FEATURES = [
  { icon: 'gauge', color: C.info, title: 'Passive by design', body: 'Below-normal priority, one sweep timer, reused NI sessions, quiet logs' },
  { icon: 'shield', color: C.violet, title: 'Secure by default', body: 'Bearer tokens, wss://, CORS allowlist, allowlisted scripts & services' },
  { icon: 'server', color: C.ok, title: 'Runs as a service', body: 'Windows service and systemd units, rotating log files' },
  { icon: 'clock', color: C.warn, title: 'Survives outages', body: 'Offline buffering, backoff reconnects, alerts restored after restart' },
  { icon: 'flask', color: C.info, title: 'Test without hardware', body: 'Simulator with seven scenarios: overheat, flap, offline, reconnect…' },
  { icon: 'check', color: C.ok, title: 'Proven', body: '553 automated tests, bindings checked against the real NI drivers' },
]

export const BuiltFor: React.FC = () => (
  <AbsoluteFill>
    <Background glow={C.info} glow2={C.ok} />
    {[14, 21, 28, 35, 42, 49].map((f) => <Sfx key={f} at={f} name="pop" volume={0.28} />)}
    <div style={{ position: 'absolute', left: 120, top: 90, display: 'flex', flexDirection: 'column', gap: 18 }}>
      <Kicker color={C.info}>Built for the test floor</Kicker>
      <Headline size={64}>Passive. Safe. Proven.</Headline>
    </div>
    <div
      style={{
        position: 'absolute',
        left: 120,
        right: 120,
        top: 300,
        display: 'grid',
        gridTemplateColumns: 'repeat(3, 1fr)',
        gap: 30,
      }}
    >
      {FEATURES.map((f, i) => {
        const s = useSpring(14 + i * 7)
        return (
          <Card
            key={f.title}
            style={{
              padding: '34px 34px',
              height: 300,
              display: 'flex',
              flexDirection: 'column',
              gap: 16,
              opacity: s,
              transform: `translateY(${(1 - s) * 50}px) scale(${0.95 + 0.05 * s})`,
            }}
          >
            <div style={{ width: 68, height: 68, borderRadius: 18, background: `${f.color}1f`, border: `1.5px solid ${f.color}55`, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
              <Icon name={f.icon} size={38} color={f.color} />
            </div>
            <div style={{ fontFamily: FONT, fontSize: 34, fontWeight: 700, color: C.t1 }}>{f.title}</div>
            <div style={{ fontFamily: FONT, fontSize: 24, lineHeight: 1.45, color: C.t2 }}>{f.body}</div>
          </Card>
        )
      })}
    </div>
  </AbsoluteFill>
)
