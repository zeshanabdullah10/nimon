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

const Widget: React.FC = () => {
  const frame = useCurrentFrame()
  const strip = useSpring(26)
  const panel = useProgress(80, 26)
  const acked = frame >= 196
  const click = interpolate(frame, [190, 196, 204], [1, 0.9, 1], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' })

  const tiles = [
    { label: 'Station A', value: '80.0°', color: C.crit },
    { label: 'Station B', value: '48.5°', color: C.ok },
  ]
  const alerts = [
    { sev: 'CRIT', color: C.crit, title: 'Critical Temperature', where: 'PXI1Slot2 · slot 2 · 76.0 °C vs 75' },
    { sev: 'WARN', color: C.warn, title: 'Overheating predicted', where: 'PXI1Slot2 · 91% · ETA ~2 min' },
    { sev: 'WARN', color: C.warn, title: 'High Temperature', where: 'PXI1Slot2 · 65.8 °C vs 65' },
  ]

  return (
    <div
      style={{
        width: 740,
        height: 640,
        borderRadius: 22,
        overflow: 'hidden',
        position: 'relative',
        border: `1.5px solid ${C.lineHi}`,
        background: 'radial-gradient(circle at 20% 110%, #1d3a5c 0%, transparent 55%), radial-gradient(circle at 90% -10%, #3b2a5e 0%, transparent 50%), #0b0d12',
        boxShadow: '0 30px 90px rgba(0,0,0,0.55)',
      }}
    >
      <div style={{ position: 'absolute', left: 24, bottom: 20, fontFamily: MONO, fontSize: 16, color: 'rgba(255,255,255,0.35)' }}>
        Windows desktop · beside LabVIEW
      </div>
      <div style={{ position: 'absolute', left: 60, right: 60, top: 18, opacity: strip, transform: `translateY(${(1 - strip) * -40}px)` }}>
        <div
          style={{
            height: 58,
            borderRadius: 18,
            background: 'rgba(16,17,18,0.92)',
            border: `1.5px solid ${C.lineHi}`,
            display: 'flex',
            alignItems: 'center',
            gap: 12,
            padding: '0 14px',
            boxShadow: '0 10px 40px rgba(0,0,0,0.5)',
          }}
        >
          <span style={{ fontFamily: MONO, fontSize: 18, fontWeight: 700, color: C.crit, padding: '4px 10px', borderRadius: 10, border: `1.5px solid ${C.crit}88`, background: `${C.crit}22` }}>
            CRIT
          </span>
          {tiles.map((t) => (
            <span key={t.label} style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '4px 12px', borderRadius: 10, background: 'rgba(255,255,255,0.05)' }}>
              <span style={{ width: 9, height: 9, borderRadius: 5, background: t.color }} />
              <span style={{ fontFamily: FONT, fontSize: 18, color: C.t2 }}>{t.label}</span>
              <span style={{ fontFamily: MONO, fontSize: 19, fontWeight: 700, color: t.color }}>{t.value}</span>
            </span>
          ))}
          <span style={{ marginLeft: 'auto', display: 'flex', gap: 10 }}>
            <Icon name="check" size={20} color={C.ok} />
            <Icon name="lock" size={20} color={C.t3} />
          </span>
        </div>

        <div style={{ height: 470 * panel, overflow: 'hidden', marginTop: 10 }}>
          <div
            style={{
              borderRadius: 18,
              background: 'rgba(16,17,18,0.95)',
              border: `1.5px solid ${C.lineHi}`,
              padding: '18px 18px 20px',
              display: 'flex',
              flexDirection: 'column',
              gap: 12,
              opacity: panel,
            }}
          >
            <div style={{ display: 'flex', justifyContent: 'space-between', fontFamily: FONT, fontSize: 19, color: C.t2 }}>
              <span>
                <span style={{ color: C.crit, fontWeight: 700 }}>CRIT</span> · 1 module critical
              </span>
              <span style={{ color: C.t3 }}>updated 2 s ago</span>
            </div>
            {alerts.map((a, i) => {
              const isAcked = i === 0 && acked
              return (
                <div key={a.title} style={{ display: 'flex', alignItems: 'center', gap: 12, padding: '12px 12px', borderRadius: 14, background: 'rgba(255,255,255,0.04)', border: `1px solid ${isAcked ? C.lineHi : 'transparent'}` }}>
                  <span style={{ fontFamily: MONO, fontSize: 15, fontWeight: 700, color: a.color, width: 50 }}>{a.sev}</span>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontFamily: FONT, fontSize: 19, fontWeight: 600, color: C.t1 }}>{a.title}</div>
                    <div style={{ fontFamily: FONT, fontSize: 15, color: C.t2 }}>{a.where}</div>
                  </div>
                  <span
                    style={{
                      fontFamily: FONT,
                      fontSize: 15,
                      padding: '6px 10px',
                      borderRadius: 9,
                      border: `1px solid ${C.lineHi}`,
                      color: isAcked ? C.t3 : C.t1,
                      transform: i === 0 ? `scale(${click})` : undefined,
                    }}
                  >
                    {isAcked ? 'Acked' : 'Ack'}
                  </span>
                  <span style={{ fontFamily: FONT, fontSize: 15, padding: '6px 10px', borderRadius: 9, border: `1px solid ${C.lineHi}`, color: C.t1 }}>Resolve</span>
                </div>
              )
            })}
            <div style={{ display: 'flex', gap: 10, marginTop: 4 }}>
              <span style={{ fontFamily: FONT, fontSize: 17, padding: '8px 14px', borderRadius: 10, background: `${C.info}22`, border: `1px solid ${C.info}77`, color: C.info }}>
                Open dashboard
              </span>
              <span style={{ fontFamily: FONT, fontSize: 17, padding: '8px 14px', borderRadius: 10, border: `1px solid ${C.lineHi}`, color: C.t2 }}>Close</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

export const Everywhere: React.FC = () => (
  <AbsoluteFill>
    <Background glow={C.violet} glow2={C.info} />
    <Typing start={18} chars={16} speed={1.6} /><Typing start={104} chars={47} speed={1.6} /><Typing start={168} chars={62} speed={1.6} /><Sfx at={34} name="tick" volume={0.35} /><Sfx at={140} name="chime" volume={0.3} /><Sfx at={213} name="chime" volume={0.35} /><Sfx at={26} name="pop" volume={0.35} /><Sfx at={80} name="pop" volume={0.3} /><Sfx at={192} name="tick" volume={0.5} />
    <div style={{ position: 'absolute', left: 100, top: 90, display: 'flex', flexDirection: 'column', gap: 18 }}>
      <Kicker color={C.violet}>Everywhere you work</Kicker>
      <Headline size={64}>Dashboard, desktop, terminal.</Headline>
    </div>
    <div style={{ position: 'absolute', left: 100, top: 290 }}>
      <Terminal />
    </div>
    <div style={{ position: 'absolute', left: 1100, top: 290 }}>
      <Widget />
    </div>
    <FadeUp delay={60} style={{ position: 'absolute', left: 100, right: 100, top: 962, display: 'flex', justifyContent: 'space-between' }}>
      <span style={{ fontFamily: FONT, fontSize: 24, color: C.t2 }}>
        <span style={{ color: C.t1, fontWeight: 600 }}>nimon-cli</span> · every API resource, --json for scripts
      </span>
      <span style={{ fontFamily: FONT, fontSize: 24, color: C.t2 }}>
        <span style={{ color: C.t1, fontWeight: 600 }}>Desktop widget</span> · hub + edge + UI in one exe
      </span>
    </FadeUp>
  </AbsoluteFill>
)
