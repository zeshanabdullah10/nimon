import React from 'react'
import { AbsoluteFill, interpolate, useCurrentFrame } from 'remotion'
import { C, FONT, MONO, W } from '../theme'
import { Background, FadeUp, LogoMark, useProgress, useSpring } from '../ui'
import { Sfx } from '../sfx'

/** A heartbeat trace across the full width: flat line with periodic beats. */
export function ecgPath(width: number, y: number, period = 320): string {
  let d = `M0 ${y}`
  for (let x = 0; x < width; x += period) {
    const m = x + period * 0.45
    d += ` L${m} ${y} L${m + 14} ${y - 18} L${m + 26} ${y} L${m + 38} ${y + 26} L${m + 52} ${y - 120} L${m + 68} ${y + 60} L${m + 82} ${y} L${m + 104} ${y - 22} L${m + 126} ${y} L${x + period} ${y}`
  }
  return d
}

export const Intro: React.FC = () => {
  const frame = useCurrentFrame()
  const draw = useProgress(0, 70)
  const logo = useSpring(8)
  const fadeLine = interpolate(frame, [70, 120], [0.9, 0.28], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' })
  const total = 12000
  const letters = 'NIMon'.split('')

  return (
    <AbsoluteFill>
      <Background glow={C.ok} glow2={C.info} />
      <Sfx at={4} name="beep" volume={0.3} /><Sfx at={12} name="impact" volume={0.7} /><Sfx at={24} name="beep" volume={0.25} /><Sfx at={44} name="beep" volume={0.22} /><Sfx at={48} name="tick" volume={0.35} /><Sfx at={74} name="tick" volume={0.3} />
      <svg width={W} height={1080} style={{ position: 'absolute', inset: 0 }}>
        <defs>
          <linearGradient id="ecg" x1="0" x2="1">
            <stop offset="0" stopColor={C.info} stopOpacity="0" />
            <stop offset="0.3" stopColor={C.info} />
            <stop offset="1" stopColor={C.ok} />
          </linearGradient>
        </defs>
        <path
          d={ecgPath(W, 930)}
          fill="none"
          stroke="url(#ecg)"
          strokeWidth={4}
          strokeLinejoin="round"
          strokeDasharray={total}
          strokeDashoffset={total * (1 - draw)}
          opacity={fadeLine}
          style={{ filter: `drop-shadow(0 0 12px ${C.ok})` }}
        />
      </svg>

      <AbsoluteFill style={{ alignItems: 'center', justifyContent: 'center', flexDirection: 'column', gap: 26 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 44, marginTop: -120 }}>
          <div style={{ transform: `scale(${0.6 + 0.4 * logo})`, opacity: logo }}>
            <LogoMark size={190} draw={useProgress(14, 40)} />
          </div>
          <div style={{ display: 'flex' }}>
            {letters.map((l, i) => {
              const p = useProgress(18 + i * 4, 26)
              return (
                <span
                  key={i}
                  style={{
                    fontFamily: FONT,
                    fontWeight: 800,
                    fontSize: 220,
                    letterSpacing: -6,
                    color: C.t1,
                    opacity: p,
                    transform: `translateY(${(1 - p) * 60}px)`,
                    display: 'inline-block',
                    textShadow: '0 20px 80px rgba(90,200,255,0.25)',
                  }}
                >
                  {l}
                </span>
              )
            })}
          </div>
        </div>

        <FadeUp delay={46} y={24}>
          <div style={{ fontFamily: FONT, fontSize: 46, color: C.t1, fontWeight: 500, textAlign: 'center' }}>
            Health monitoring, prediction &amp; self-healing
            <br />
            <span style={{ color: C.t2 }}>for National Instruments test stations</span>
          </div>
        </FadeUp>

        <FadeUp delay={72} y={14}>
          <div style={{ fontFamily: MONO, fontSize: 22, color: C.t3, letterSpacing: 3, marginTop: 30 }}>
            PXI · cDAQ · DAQmx · VISA · NI-SysCfg
          </div>
        </FadeUp>
      </AbsoluteFill>
    </AbsoluteFill>
  )
}
