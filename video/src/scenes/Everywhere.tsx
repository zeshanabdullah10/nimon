import React from 'react'
import { AbsoluteFill, interpolate, useCurrentFrame } from 'remotion'
import { C, FONT, MONO } from '../theme'
import { Background, Card, FadeUp, Headline, Icon, Kicker, useProgress, useSpring } from '../ui'
import { Sfx, Typing } from '../sfx'

type Block = { cmd: string; start: number; out: { text: string; color?: string }[] }

// Real output captured from nimon-cli 0.3.0 against a live hub + simulator.
const BLOCKS: Block[] = [
  {
    cmd: 'nimon-cli status',
    start: 18,
    out: [
      { text: 'Status       healthy', color: C.ok },
      { text: 'Version      0.3.0' },
      { text: 'Components   database ok, alert manager ok' },
      { text: 'Edges        2 online / 2 known' },
      { text: 'Devices      12 live / 12 known' },
      { text: 'Alerts       4 active (1 critical, 3 warning)', color: C.warn },
      { text: 'Predictions  1 active' },
    ],
  },
  {
    cmd: 'nimon-cli alerts ack 01M39J8VQYCEQZ0DFT9TWGDRHP',
    start: 104,
    out: [{ text: 'Alert 01M39J8VQYCEQZ0DFT9TWGDRHP acknowledged', color: C.ok }],
  },
  {
    cmd: 'nimon-cli actions run sim-edge-01:PXI1Slot2 power_cycle --yes',
    start: 168,
    out: [
      { text: 'Action accepted: manual-01M39JA3WV6B7P24WMTY6749BX', color: C.ok },
      { text: 'EXECUTED  DEVICE                 TYPE         RESULT  TIME', color: C.t3 },
      { text: 'just now  sim-edge-01:PXI1Slot2  power_cycle  ok      0.1s' },
    ],
  },
]

const TYPE_SPEED = 1.6 // chars per frame

const Terminal: React.FC = () => {
  const frame = useCurrentFrame()
  const enter = useSpring(4)
  const rows: React.ReactNode[] = []
  BLOCKS.forEach((b, bi) => {
    if (frame < b.start) return
    const typed = Math.min(b.cmd.length, Math.floor((frame - b.start) * TYPE_SPEED))
    const doneAt = b.start + Math.ceil(b.cmd.length / TYPE_SPEED)
    const typing = typed < b.cmd.length
    const cursor = typing || (frame - doneAt < 8 && Math.floor(frame / 8) % 2 === 0)
    rows.push(
      <div key={`c${bi}`} style={{ color: C.t1, marginTop: bi ? 18 : 0 }}>
        <span style={{ color: C.ok }}>PS&gt; </span>
        {b.cmd.slice(0, typed)}
        {cursor && <span style={{ background: C.t1, color: C.t1 }}>_</span>}
      </div>,
    )
    b.out.forEach((o, oi) => {
      const at = doneAt + 6 + oi * 3
      if (frame >= at) {
        rows.push(
          <div key={`o${bi}-${oi}`} style={{ color: o.color ?? C.t2, whiteSpace: 'pre' }}>
            {o.text}
          </div>,
        )
      }
    })
  })

  return (
    <Card style={{ width: 980, height: 640, overflow: 'hidden', opacity: enter, transform: `translateY(${(1 - enter) * 50}px)` }}>
      <div style={{ height: 50, display: 'flex', alignItems: 'center', gap: 12, padding: '0 20px', borderBottom: `1px solid ${C.line}`, background: '#0c0d0e' }}>
        <Icon name="terminal" size={24} color={C.t2} />
        <span style={{ fontFamily: MONO, fontSize: 18, color: C.t2 }}>nimon-cli · --hub http://hub:9090</span>
      </div>
      <div style={{ padding: '22px 26px', fontFamily: MONO, fontSize: 21, lineHeight: 1.5 }}>{rows}</div>
    </Card>
  )
}

// Real response shape from GET /api/v1/devices (trimmed), captured from a live hub.
const JSON_LINES: { t: string; c?: string }[] = [
  { t: 'GET /api/v1/devices?edge_id=sim-edge-01', c: C.violet },
  { t: '{' },
  { t: '  "devices": [{' },
  { t: '    "device_id": "sim-edge-01:PXI1Slot2",', c: C.info },
  { t: '    "model": "PXIe-6368",  "slot": 2,' },
  { t: '    "status": "error",', c: C.crit },
  { t: '    "metrics": { "temperature": 79.96 },', c: C.warn },
  { t: '    "live": true,  "is_simulated": true' },
  { t: '  }, …],' },
  { t: '  "total": 6' },
  { t: '}' },
]

const ENDPOINTS = ['/health', '/settings', '/edges', '/devices', '/devices/:id/metrics', '/alerts', '/alerts/history', '/predictions', '/actions']

const Api: React.FC = () => {
  const frame = useCurrentFrame()
  const enter = useSpring(24)
  return (
    <Card style={{ width: 740, height: 640, overflow: 'hidden', opacity: enter, transform: `translateY(${(1 - enter) * 50}px)` }}>
      <div style={{ height: 50, display: 'flex', alignItems: 'center', gap: 12, padding: '0 20px', borderBottom: `1px solid ${C.line}`, background: '#0c0d0e' }}>
        <Icon name="globe" size={24} color={C.violet} />
        <span style={{ fontFamily: MONO, fontSize: 18, color: C.t2 }}>REST API · /api/v1</span>
      </div>
      <div style={{ padding: '20px 26px', fontFamily: MONO, fontSize: 19, lineHeight: 1.55 }}>
        {JSON_LINES.map((l, i) => {
          const show = interpolate(frame, [40 + i * 4, 48 + i * 4], [0, 1], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' })
          return (
            <div key={i} style={{ whiteSpace: 'pre', color: l.c ?? C.t2, opacity: show }}>
              {l.t}
            </div>
          )
        })}
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8, marginTop: 18 }}>
          {ENDPOINTS.map((e, i) => {
            const p = interpolate(frame, [100 + i * 5, 110 + i * 5], [0, 1], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' })
            return (
              <span
                key={e}
                style={{
                  opacity: p,
                  fontSize: 16,
                  padding: '4px 10px',
                  borderRadius: 8,
                  border: `1px solid ${C.violet}55`,
                  background: `${C.violet}14`,
                  color: C.t1,
                }}
              >
                {e}
              </span>
            )
          })}
        </div>
      </div>
    </Card>
  )
}

export const Everywhere: React.FC = () => (
  <AbsoluteFill>
    <Background glow={C.violet} glow2={C.info} />
    <Typing start={18} chars={16} speed={1.6} />
    <Typing start={104} chars={47} speed={1.6} />
    <Typing start={168} chars={62} speed={1.6} />
    <Sfx at={34} name="tick" volume={0.35} />
    <Sfx at={140} name="chime" volume={0.3} />
    <Sfx at={213} name="chime" volume={0.35} />
    <Sfx at={26} name="pop" volume={0.35} />
    <div style={{ position: 'absolute', left: 100, top: 90, display: 'flex', flexDirection: 'column', gap: 18 }}>
      <Kicker color={C.violet}>CLI &amp; REST API</Kicker>
      <Headline size={64}>Script it. Automate it.</Headline>
    </div>
    <div style={{ position: 'absolute', left: 100, top: 290 }}>
      <Terminal />
    </div>
    <div style={{ position: 'absolute', left: 1100, top: 290 }}>
      <Api />
    </div>
    <FadeUp delay={60} style={{ position: 'absolute', left: 100, right: 100, top: 962, display: 'flex', justifyContent: 'space-between' }}>
      <span style={{ fontFamily: FONT, fontSize: 24, color: C.t2 }}>
        <span style={{ color: C.t1, fontWeight: 600 }}>nimon-cli</span> · every API resource, --json for scripts
      </span>
      <span style={{ fontFamily: FONT, fontSize: 24, color: C.t2 }}>
        <span style={{ color: C.t1, fontWeight: 600 }}>REST + WebSocket</span> · bearer-token protected writes
      </span>
    </FadeUp>
  </AbsoluteFill>
)
