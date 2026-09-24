import React from 'react'
import { AbsoluteFill, Img, interpolate, staticFile, useCurrentFrame } from 'remotion'
import { C, FONT, MONO } from '../theme'
import { Background, Card, easeInOut, Headline, Icon, Kicker, Sub, useProgress, useSpring } from '../ui'
import { Sfx } from '../sfx'

// Real widget UI captured by scripts/capture-widget.mjs: 450x860 CSS px at 2x.
const S = 1.2
const WW = 450 * S
const WX = 1920 - WW - 18
const WY = 22
const at = (x: number, y: number) => ({ x: WX + x * S, y: WY + y * S })

const HUB = at(420, 36)
const SA = at(420, 97)
const ACK = at(279, 421)
const LOCK = at(420, 248)
const REST = { x: 1000, y: 760 }
const AWAY = { x: 1250, y: 620 }
const DRAG = 80 // px the rail is dragged down before locking
const GRAB = at(420, 226) // rail background between tiles and lock
const down = (p: { x: number; y: number }) => ({ x: p.x, y: p.y + DRAG })
const PARK = { x: 1640, y: 640 }

// Cursor path: [frame, point]
const PATH: [number, { x: number; y: number }][] = [
  [0, REST], [96, REST], [120, HUB], [180, HUB], [196, SA], [238, SA],
  [262, ACK], [300, ACK], [330, AWAY], [344, GRAB], [350, GRAB], [366, down(GRAB)],
  [374, down(LOCK)], [388, down(LOCK)], [408, PARK], [420, PARK],
]
const CLICKS = [270, 380]
const DRAG_FROM = 350
const DRAG_TO = 366

// Which captured state is on screen when: [image, from, to]
const STATES: [string, number, number][] = [
  ['strip', -20, 124],
  ['panel-all', 124, 196],
  ['panel-a', 196, 272],
  ['panel-a-acked', 272, 338],
  ['strip', 338, 380],
  ['strip-locked', 380, 999],
]

const isPanel = (name: string) => name.startsWith('panel')

/** Every capture contains the rail, so never cross-dissolve two of them (the
 * rail would dip): an opening panel fades in on top of a held state, and a
 * collapsing panel fades out underneath the (already opaque) strip. */
function stateOpacity(frame: number, i: number): number {
  const [name, a, b] = STATES[i]
  const next = STATES[i + 1]
  const ramp = (f0: number, f1: number, v0: number, v1: number) =>
    interpolate(frame, [f0, f1], [v0, v1], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' })
  const fadeIn = i === 0 || !isPanel(name) ? (frame >= a - 4 ? 1 : 0) : ramp(a - 4, a + 4, 0, 1)
  const fadeOut = !next ? 1 : isPanel(next[0]) ? (frame < b + 4 ? 1 : 0) : ramp(b - 4, b + 4, 1, 0)
  return Math.min(fadeIn, fadeOut)
}

const STEPS: { from: number; title: string; body: string; color: string }[] = [
  { from: 30, color: C.ok, title: 'Always on, never in the way', body: 'A slim rail docked at the screen edge. Below-normal priority, quiet until something needs you.' },
  { from: 124, color: C.info, title: 'Hover to see everything', body: 'Every module, alert and prediction across the whole fleet, or one station at a time.' },
  { from: 196, color: C.crit, title: 'The station at a glance', body: 'Temperature vs. threshold, link state, peak, RAM and disk, and fleet health meters.' },
  { from: 262, color: C.warn, title: 'Act without switching apps', body: 'Acknowledge or resolve alerts right from the panel; the change is saved on the hub.' },
  { from: 340, color: C.violet, title: 'Drag it, dock it, lock it', body: 'Place it on any monitor and lock it there. Hub, edge and UI ship as one exe.' },
]

const STEP_ROWS = [
  { name: 'Initialize instruments', t: '0.8 s', done: 0 },
  { name: 'Calibrate RF path', t: '2.4 s', done: 0 },
  { name: 'Measure gain', t: '1.1 s', done: 0 },
  { name: 'Measure noise figure', t: '', done: 1 },
  { name: 'Measure spurious emissions', t: '', done: 2 },
]

const TestStand: React.FC = () => {
  const frame = useCurrentFrame()
  const enter = useSpring(0, 20)
  const prog = ((frame % 150) / 150) * 100
  return (
    <div style={{ opacity: enter, transform: `translateY(${(1 - enter) * 30}px)` }}>
      <Card style={{ width: 1060, height: 400, overflow: 'hidden' }}>
        <div style={{ height: 46, display: 'flex', alignItems: 'center', gap: 14, padding: '0 20px', background: '#0c0d0e', borderBottom: `1px solid ${C.line}` }}>
          <span style={{ width: 14, height: 14, borderRadius: 4, background: '#2f6fdc' }} />
          <span style={{ fontFamily: FONT, fontSize: 19, color: C.t2 }}>TestStand · RF_Final_Test.seq · Station A</span>
          <span style={{ marginLeft: 'auto', fontFamily: MONO, fontSize: 16, color: C.t3 }}>UUT 1287 · 1 failed</span>
        </div>
        <div style={{ padding: '14px 22px', display: 'flex', flexDirection: 'column', gap: 8 }}>
          {STEP_ROWS.map((s) => (
            <div
              key={s.name}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 16,
                padding: '11px 16px',
                borderRadius: 10,
                background: s.done === 1 ? 'rgba(90,200,255,0.08)' : 'rgba(255,255,255,0.03)',
                border: `1px solid ${s.done === 1 ? `${C.info}44` : 'transparent'}`,
                fontFamily: FONT,
                fontSize: 21,
                color: s.done === 2 ? C.t3 : C.t1,
              }}
            >
              {s.done === 0 && <Icon name="check" size={22} color={C.ok} stroke={2.6} />}
              {s.done === 1 && <span style={{ width: 22, textAlign: 'center', color: C.info, fontSize: 18 }}>▶</span>}
              {s.done === 2 && <span style={{ width: 22 }} />}
              <span>{s.name}</span>
              {s.done === 1 ? (
                <span style={{ marginLeft: 'auto', width: 220, height: 8, borderRadius: 4, background: 'rgba(255,255,255,0.08)', overflow: 'hidden' }}>
                  <span style={{ display: 'block', width: `${prog}%`, height: '100%', background: C.info }} />
                </span>
              ) : (
                <span style={{ marginLeft: 'auto', fontFamily: MONO, fontSize: 17, color: C.t3 }}>{s.done === 0 ? `Passed · ${s.t}` : 'Pending'}</span>
              )}
            </div>
          ))}
        </div>
      </Card>
    </div>
  )
}

/** Magnified view of the collapsed rail (CSS box 396..444 x 12..266 in the capture). */
const LOUPE_SCALE = 2.3
const RAIL = { x: 396, y: 12, w: 48, h: 254 }
const Loupe: React.FC = () => {
  const frame = useCurrentFrame()
  const o = interpolate(frame, [36, 50, 108, 122], [0, 1, 1, 0], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' })
  if (o <= 0) return null
  const lw = RAIL.w * LOUPE_SCALE
  const lh = RAIL.h * LOUPE_SCALE
  const left = 1182
  const top = 96
  const railMid = at(RAIL.x, RAIL.y + RAIL.h / 2)
  return (
    <>
      <svg width={1920} height={1080} style={{ position: 'absolute', inset: 0, opacity: o * 0.8 }}>
        <line x1={left + lw + 24} y1={top + lh / 2 + 24} x2={railMid.x - 6} y2={railMid.y} stroke={C.ok} strokeWidth={2} strokeDasharray="4 6" />
      </svg>
      <div style={{ position: 'absolute', left, top, opacity: o, transform: `scale(${0.9 + 0.1 * o})`, transformOrigin: 'right center' }}>
        <Card glow={C.ok} style={{ padding: 24 }}>
          <div style={{ width: lw, height: lh, overflow: 'hidden', position: 'relative' }}>
            <Img
              src={staticFile('widget/strip.png')}
              style={{ position: 'absolute', width: 450 * LOUPE_SCALE, left: -RAIL.x * LOUPE_SCALE, top: -RAIL.y * LOUPE_SCALE }}
            />
          </div>
        </Card>
        <div style={{ fontFamily: MONO, fontSize: 16, color: C.t3, textAlign: 'center', marginTop: 10 }}>actual rail · 48 px</div>
      </div>
    </>
  )
}

const Cursor: React.FC = () => {
  const frame = useCurrentFrame()
  const fs = PATH.map((p) => p[0])
  const x = interpolate(frame, fs, PATH.map((p) => p[1].x), { easing: easeInOut, extrapolateRight: 'clamp' })
  const y = interpolate(frame, fs, PATH.map((p) => p[1].y), { easing: easeInOut, extrapolateRight: 'clamp' })
  const show = interpolate(frame, [70, 90], [0, 1], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' })
  const press = CLICKS.some((c) => frame >= c && frame < c + 5) || (frame >= DRAG_FROM && frame <= DRAG_TO)
  return (
    <>
      {CLICKS.map((c) => {
        const t = frame - c
        if (t < 0 || t > 18) return null
        return (
          <div
            key={c}
            style={{
              position: 'absolute',
              left: x - 30,
              top: y - 30,
              width: 60,
              height: 60,
              borderRadius: 30,
              border: `3px solid ${C.info}`,
              opacity: 1 - t / 18,
              transform: `scale(${0.3 + t / 14})`,
            }}
          />
        )
      })}
      <svg
        width={34}
        height={34}
        viewBox="0 0 24 24"
        style={{
          position: 'absolute',
          left: x - 4,
          top: y - 2,
          opacity: show,
          transform: `scale(${press ? 0.88 : 1})`,
          filter: 'drop-shadow(0 3px 6px rgba(0,0,0,0.6))',
        }}
      >
        <path d="M4 2l15 10.5-6.6 1.2 3.9 7.3-2.8 1.5-3.9-7.3L4 20z" fill="#fff" stroke="#111" strokeWidth={1.2} strokeLinejoin="round" />
      </svg>
    </>
  )
}

export const Widget: React.FC = () => {
  const frame = useCurrentFrame()
  const slide = useProgress(10, 30)
  const dragY = interpolate(frame, [DRAG_FROM, DRAG_TO], [0, DRAG], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp', easing: easeInOut })
  const step = STEPS.reduce((acc, s, i) => (frame >= s.from ? i : acc), -1)
  const cur = step >= 0 ? STEPS[step] : null
  const stepP = cur ? interpolate(frame, [cur.from, cur.from + 14], [0, 1], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' }) : 0

  return (
    <AbsoluteFill>
      <Background glow={C.ok} glow2={C.info} />
      <Sfx at={16} name="pop" volume={0.4} />
      <Sfx at={120} name="tick" volume={0.45} />
      <Sfx at={196} name="tick" volume={0.45} />
      <Sfx at={270} name="tick" volume={0.55} />
      <Sfx at={278} name="chime" volume={0.4} />
      <Sfx at={338} name="tick" volume={0.3} />
      <Sfx at={350} name="tick" volume={0.3} />
      <Sfx at={380} name="tick" volume={0.55} />

      <div style={{ position: 'absolute', left: 100, top: 84, width: 1060, display: 'flex', flexDirection: 'column', gap: 16 }}>
        <Kicker color={C.ok}>The desktop widget</Kicker>
        <Headline size={58}>Your whole rack, one glance away.</Headline>
        <Sub delay={16} style={{ fontSize: 27 }}>
          It lives at the edge of the screen, beside LabVIEW and TestStand, and shows real hardware health the moment it changes.
        </Sub>
      </div>

      <div style={{ position: 'absolute', left: 100, top: 340 }}>
        <TestStand />
      </div>

      <div style={{ position: 'absolute', left: 100, top: 776, width: 1060 }}>
        <Card glow={cur?.color} style={{ height: 196, padding: '28px 34px', display: 'flex', gap: 28, alignItems: 'center', opacity: step >= 0 ? 1 : 0 }}>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
            {STEPS.map((s, i) => (
              <span
                key={s.title}
                style={{
                  width: 12,
                  height: 12,
                  borderRadius: 6,
                  background: i === step ? s.color : i < step ? C.t3 : 'rgba(255,255,255,0.12)',
                  boxShadow: i === step ? `0 0 12px ${s.color}` : 'none',
                }}
              />
            ))}
          </div>
          {cur && (
            <div style={{ opacity: stepP, transform: `translateY(${(1 - stepP) * 14}px)` }}>
              <div style={{ fontFamily: MONO, fontSize: 19, color: cur.color, letterSpacing: 2 }}>
                {String(step + 1).padStart(2, '0')} / {String(STEPS.length).padStart(2, '0')}
              </div>
              <div style={{ fontFamily: FONT, fontSize: 38, fontWeight: 700, color: C.t1, marginTop: 6 }}>{cur.title}</div>
              <div style={{ fontFamily: FONT, fontSize: 25, color: C.t2, marginTop: 6, lineHeight: 1.4 }}>{cur.body}</div>
            </div>
          )}
        </Card>
      </div>

      <div style={{ position: 'absolute', left: WX, top: WY, width: WW, transform: `translate(${(1 - slide) * 140}px, ${dragY}px)`, opacity: slide }}>
        {STATES.map(([name, a, b], i) => {
          const o = stateOpacity(frame, i)
          if (o <= 0) return null
          return (
            <Img
              key={`${name}-${i}`}
              src={staticFile(`widget/${name}.png`)}
              style={{ position: 'absolute', left: 0, top: 0, width: WW, opacity: o }}
            />
          )
        })}
      </div>

      <Loupe />
      <Cursor />
    </AbsoluteFill>
  )
}
