import React from 'react'
import { AbsoluteFill, interpolate, useCurrentFrame } from 'remotion'
import { C, FONT, MONO, W } from '../theme'
import { Background, FadeUp, Icon, LogoMark, useProgress, useSpring } from '../ui'
import { Sfx } from '../sfx'
import { ecgPath } from './Intro'

export const OUTRO_FRAMES = 180

export const Outro: React.FC = () => {
  const frame = useCurrentFrame()
  const durationInFrames = OUTRO_FRAMES
  const logo = useSpring(4)
  const draw = useProgress(0, 60)
  const total = 12000
  const fadeOut = interpolate(frame, [durationInFrames - 24, durationInFrames - 2], [1, 0], {
    extrapolateLeft: 'clamp',
    extrapolateRight: 'clamp',
  })
  const words = ['Monitor.', 'Predict.', 'Heal.']
  const colors = [C.info, C.warn, C.ok]

  return (
    <AbsoluteFill style={{ opacity: fadeOut }}>
      <Background glow={C.ok} glow2={C.info} />
      <Sfx at={4} name="impact" volume={0.65} /><Sfx at={22} name="chime" volume={0.4} />
      <svg width={W} height={1080} style={{ position: 'absolute', inset: 0 }}>
        <path
          d={ecgPath(W, 930, 380)}
          fill="none"
          stroke={C.ok}
          strokeOpacity={0.35}
          strokeWidth={3}
          strokeDasharray={total}
          strokeDashoffset={total * (1 - draw)}
          style={{ filter: `drop-shadow(0 0 10px ${C.ok})` }}
        />
      </svg>
      <AbsoluteFill style={{ alignItems: 'center', justifyContent: 'center', flexDirection: 'column', gap: 34 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 34, marginTop: -110, opacity: logo, transform: `scale(${0.85 + 0.15 * logo})` }}>
          <LogoMark size={150} />
          <span style={{ fontFamily: FONT, fontWeight: 800, fontSize: 168, letterSpacing: -5, color: C.t1 }}>NIMon</span>
        </div>
        <div style={{ display: 'flex', gap: 30 }}>
          {words.map((w, i) => (
            <FadeUp key={w} delay={20 + i * 10} y={24}>
              <span style={{ fontFamily: FONT, fontSize: 58, fontWeight: 700, color: colors[i] }}>{w}</span>
            </FadeUp>
          ))}
        </div>
        <FadeUp delay={56} y={16}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 18, marginTop: 22, padding: '14px 28px', borderRadius: 16, border: `1.5px solid ${C.lineHi}`, background: 'rgba(255,255,255,0.04)' }}>
            <Icon name="github" size={34} color={C.t1} />
            <span style={{ fontFamily: MONO, fontSize: 30, color: C.t1 }}>github.com/zeshanabdullah10/nimon</span>
            <span style={{ fontFamily: MONO, fontSize: 24, color: C.ok, marginLeft: 10 }}>v0.3.0</span>
          </div>
        </FadeUp>
        <FadeUp delay={70} y={12}>
          <div style={{ fontFamily: FONT, fontSize: 26, color: C.t3 }}>MIT licensed · built in Rust, Tauri &amp; React</div>
        </FadeUp>
      </AbsoluteFill>
    </AbsoluteFill>
  )
}
