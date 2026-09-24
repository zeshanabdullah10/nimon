import React from 'react'
import { AbsoluteFill, interpolate, interpolateColors, useCurrentFrame } from 'remotion'
import { C, FONT, MONO } from '../theme'
import { Background, Card, FadeUp, Headline, Icon, Kicker, useProgress, useSpring } from '../ui'
import { Sfx } from '../sfx'

const SLOTS = [
  { name: 'PXIe-8880', w: 118, ctrl: true },
  { name: 'PXIe-4081', w: 70 },
  { name: 'PXIe-5171', w: 70 },
  { name: 'PXIe-6368', w: 70, hot: true },
  { name: 'PXIe-4139', w: 70 },
  { name: 'PXIe-2527', w: 70 },
  { name: 'PXIe-6570', w: 70 },
  { name: 'PXIe-5840', w: 70 },
]

const HEAT_START = 40
const HEAT_END = 140
const FAIL_AT = 150

export const Problem: React.FC = () => {
  const frame = useCurrentFrame()
  const chassis = useSpring(4)
  const temp = interpolate(frame, [HEAT_START, HEAT_END], [52, 81.4], {
    extrapolateLeft: 'clamp',
    extrapolateRight: 'clamp',
  })
  const heat = interpolate(temp, [52, 65, 75, 81.4], [0, 0.35, 0.75, 1])
  const hotColor = interpolateColors(heat, [0, 0.35, 0.75, 1], ['#3a3d42', C.warn, C.crit, C.crit])
  const tempColor = temp >= 75 ? C.crit : temp >= 65 ? C.warn : C.ok
  const failed = frame >= FAIL_AT
  const failP = useProgress(FAIL_AT, 18)
  const shake = failed ? Math.sin(frame * 2.2) * 6 * (1 - failP) : 0

  return (
    <AbsoluteFill>
      <Background glow={failed ? C.crit : C.info} glow2={C.warn} />
      <Sfx at={32} name="tick" volume={0.35} /><Sfx at={84} name="ping" volume={0.35} /><Sfx at={150} name="alarm" volume={0.5} />
      <AbsoluteFill style={{ padding: '120px 110px', flexDirection: 'row', gap: 60 }}>
        <div style={{ width: 800, display: 'flex', flexDirection: 'column', gap: 28, paddingTop: 70 }}>
          <Kicker color={C.warn}>The problem</Kicker>
          <Headline size={70}>
            Test stations run 24/7.
            <br />
            <span style={{ color: C.t2 }}>Hardware fails quietly.</span>
          </Headline>
          <FadeUp delay={20}>
            <div style={{ fontFamily: FONT, fontSize: 30, lineHeight: 1.45, color: C.t2, maxWidth: 640 }}>
              A module creeps hotter, a driver session wedges, a chassis drops off the bus. Nobody sees it while
              LabVIEW and TestStand keep running.
            </div>
          </FadeUp>

          <div
            style={{
              marginTop: 18,
              opacity: failP,
              transform: `translateY(${(1 - failP) * 20}px)`,
              display: 'flex',
              flexDirection: 'column',
              gap: 14,
            }}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: 16, fontFamily: FONT, fontSize: 38, fontWeight: 700, color: C.crit }}>
              <Icon name="alert" size={42} color={C.crit} stroke={2.4} />
              You find out when the test fails.
            </div>
            <div style={{ fontFamily: MONO, fontSize: 24, color: C.t2, paddingLeft: 58 }}>
              Station down · UUTs scrapped · hours lost
            </div>
          </div>
        </div>

        <div
          style={{
            flex: 1,
            display: 'flex',
            flexDirection: 'column',
            justifyContent: 'center',
            gap: 26,
            opacity: chassis,
            transform: `translateY(${(1 - chassis) * 60}px) translateX(${shake}px)`,
          }}
        >
          <div style={{ fontFamily: MONO, fontSize: 22, color: C.t3, letterSpacing: 2, textAlign: 'right' }}>PXIe-1085 · STATION 3</div>
          <Card style={{ padding: 22, position: 'relative' }}>
            <div style={{ display: 'flex', gap: 10, height: 430 }}>
              {SLOTS.map((s, i) => {
                const border = s.hot ? hotColor : C.lineHi
                const blink = (Math.floor((frame + i * 7) / 12) % 3) !== 0
                const led = s.hot && heat > 0.7 ? (frame % 10 < 5 ? C.crit : '#3a1c1d') : blink ? C.ok : '#1c3a26'
                return (
                  <div
                    key={s.name}
                    style={{
                      width: s.w,
                      borderRadius: 10,
                      border: `2px solid ${border}`,
                      background: s.hot
                        ? `linear-gradient(180deg, ${hotColor}${Math.round(heat * 60).toString(16).padStart(2, '0')}, #16171a)`
                        : 'linear-gradient(180deg, #1e2023, #141517)',
                      boxShadow: s.hot ? `0 0 ${heat * 60}px ${hotColor}` : 'none',
                      display: 'flex',
                      flexDirection: 'column',
                      alignItems: 'center',
                      padding: '14px 0',
                      gap: 14,
                    }}
                  >
                    <span style={{ width: 12, height: 12, borderRadius: 6, background: led, boxShadow: `0 0 10px ${led}` }} />
                    {Array.from({ length: s.ctrl ? 4 : 5 }).map((_, k) => (
                      <span
                        key={k}
                        style={{
                          width: s.ctrl ? 70 : 34,
                          height: s.ctrl ? 22 : 34,
                          borderRadius: s.ctrl ? 5 : 17,
                          border: '2px solid #34373c',
                          background: '#0e0f11',
                        }}
                      />
                    ))}
                    <span
                      style={{
                        marginTop: 'auto',
                        writingMode: 'vertical-rl',
                        transform: 'rotate(180deg)',
                        fontFamily: MONO,
                        fontSize: 17,
                        color: s.hot ? hotColor : C.t3,
                        letterSpacing: 1,
                      }}
                    >
                      {s.name}
                    </span>
                  </div>
                )
              })}
            </div>

            <div
              style={{
                position: 'absolute',
                top: -58,
                left: 22 + 288 + 35 - 78,
                padding: '8px 16px',
                borderRadius: 12,
                background: '#0d0e10',
                border: `2px solid ${tempColor}`,
                fontFamily: MONO,
                fontSize: 28,
                fontWeight: 700,
                color: tempColor,
                opacity: interpolate(frame, [HEAT_START - 10, HEAT_START + 5], [0, 1], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' }),
                boxShadow: `0 0 30px ${tempColor}55`,
              }}
            >
              {temp.toFixed(1)} °C
            </div>
          </Card>

          <Card style={{ padding: '18px 24px', display: 'flex', alignItems: 'center', gap: 18 }}>
            <span style={{ fontFamily: MONO, fontSize: 22, color: C.t3 }}>TestStand</span>
            <span style={{ fontFamily: MONO, fontSize: 22, color: C.t1 }}>RF_Final_Test · UUT 1284</span>
            <span
              style={{
                marginLeft: 'auto',
                fontFamily: MONO,
                fontSize: 22,
                fontWeight: 700,
                color: failed ? C.crit : C.ok,
                display: 'flex',
                alignItems: 'center',
                gap: 10,
                whiteSpace: 'nowrap',
              }}
            >
              <span style={{ width: 12, height: 12, borderRadius: 6, background: failed ? C.crit : C.ok }} />
              {failed ? 'FAILED · DAQ timeout' : 'Running'}
            </span>
          </Card>
        </div>
      </AbsoluteFill>
    </AbsoluteFill>
  )
}
