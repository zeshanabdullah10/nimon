import React from 'react'
import {
  AbsoluteFill,
  Easing,
  Img,
  interpolate,
  spring,
  staticFile,
  useCurrentFrame,
  useVideoConfig,
} from 'remotion'
import { C, FONT, MONO } from './theme'

export const easeOut = Easing.bezier(0.16, 1, 0.3, 1)
export const easeInOut = Easing.bezier(0.65, 0, 0.35, 1)

/** 0→1 over `dur` frames starting at `delay`, eased. */
export function useProgress(delay: number, dur = 24, easing = easeOut): number {
  const frame = useCurrentFrame()
  return interpolate(frame, [delay, delay + dur], [0, 1], {
    extrapolateLeft: 'clamp',
    extrapolateRight: 'clamp',
    easing,
  })
}

/** Soft spring that settles without overshoot (0→1). */
export function useSpring(delay: number, damping = 18): number {
  const frame = useCurrentFrame()
  const { fps } = useVideoConfig()
  return spring({ frame: frame - delay, fps, config: { damping, mass: 0.8 } })
}

export function lerp(p: number, a: number, b: number): number {
  return a + (b - a) * p
}

export const FadeUp: React.FC<{
  delay?: number
  dur?: number
  y?: number
  style?: React.CSSProperties
  children: React.ReactNode
}> = ({ delay = 0, dur = 22, y = 28, style, children }) => {
  const p = useProgress(delay, dur)
  return (
    <div style={{ opacity: p, transform: `translateY(${(1 - p) * y}px)`, ...style }}>
      {children}
    </div>
  )
}

/** Dark stage with a faint grid, drifting glows and a vignette. */
export const Background: React.FC<{ glow?: string; glow2?: string }> = ({
  glow = C.info,
  glow2 = C.violet,
}) => {
  const frame = useCurrentFrame()
  const dx = Math.sin(frame / 90) * 60
  const dy = Math.cos(frame / 110) * 40
  return (
    <AbsoluteFill style={{ background: C.bg, overflow: 'hidden' }}>
      <AbsoluteFill
        style={{
          backgroundImage: `linear-gradient(${C.line} 1px, transparent 1px), linear-gradient(90deg, ${C.line} 1px, transparent 1px)`,
          backgroundSize: '64px 64px',
          opacity: 0.35,
          maskImage: 'radial-gradient(ellipse at center, black 30%, transparent 75%)',
          WebkitMaskImage: 'radial-gradient(ellipse at center, black 30%, transparent 75%)',
        }}
      />
      <div
        style={{
          position: 'absolute',
          width: 1100,
          height: 1100,
          left: -250 + dx,
          top: -450 + dy,
          borderRadius: '50%',
          background: `radial-gradient(circle, ${glow}33 0%, transparent 60%)`,
          filter: 'blur(40px)',
        }}
      />
      <div
        style={{
          position: 'absolute',
          width: 1000,
          height: 1000,
          right: -300 - dx,
          bottom: -500 - dy,
          borderRadius: '50%',
          background: `radial-gradient(circle, ${glow2}2a 0%, transparent 60%)`,
          filter: 'blur(40px)',
        }}
      />
      <AbsoluteFill
        style={{ background: 'radial-gradient(ellipse at center, transparent 55%, rgba(0,0,0,0.65) 100%)' }}
      />
    </AbsoluteFill>
  )
}

export const Kicker: React.FC<{ color?: string; children: React.ReactNode; delay?: number }> = ({
  color = C.info,
  children,
  delay = 0,
}) => (
  <FadeUp delay={delay} y={14}>
    <div
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 12,
        fontFamily: MONO,
        fontSize: 22,
        letterSpacing: 4,
        color,
        textTransform: 'uppercase',
      }}
    >
      <span
        style={{ width: 10, height: 10, borderRadius: 5, background: color, boxShadow: `0 0 16px ${color}` }}
      />
      {children}
    </div>
  </FadeUp>
)

export const Headline: React.FC<{
  children: React.ReactNode
  size?: number
  delay?: number
  style?: React.CSSProperties
}> = ({ children, size = 72, delay = 6, style }) => (
  <FadeUp delay={delay} y={34}>
    <div
      style={{
        fontFamily: FONT,
        fontWeight: 700,
        fontSize: size,
        lineHeight: 1.08,
        letterSpacing: -1.5,
        color: C.t1,
        ...style,
      }}
    >
      {children}
    </div>
  </FadeUp>
)

export const Sub: React.FC<{ children: React.ReactNode; delay?: number; style?: React.CSSProperties }> = ({
  children,
  delay = 14,
  style,
}) => (
  <FadeUp delay={delay} y={20}>
    <div style={{ fontFamily: FONT, fontSize: 30, lineHeight: 1.4, color: C.t2, ...style }}>{children}</div>
  </FadeUp>
)

export const Pill: React.FC<{ color: string; children: React.ReactNode; style?: React.CSSProperties }> = ({
  color,
  children,
  style,
}) => (
  <span
    style={{
      display: 'inline-flex',
      alignItems: 'center',
      gap: 8,
      padding: '6px 16px',
      borderRadius: 999,
      border: `1.5px solid ${color}88`,
      background: `${color}1f`,
      color,
      fontFamily: MONO,
      fontSize: 20,
      fontWeight: 600,
      letterSpacing: 1,
      whiteSpace: 'nowrap',
      ...style,
    }}
  >
    {children}
  </span>
)

export const Card: React.FC<{ style?: React.CSSProperties; glow?: string; children: React.ReactNode }> = ({
  style,
  glow,
  children,
}) => (
  <div
    style={{
      background: `linear-gradient(180deg, ${C.cardHi}, ${C.card})`,
      border: `1.5px solid ${glow ? `${glow}66` : C.line}`,
      borderRadius: 22,
      boxShadow: glow
        ? `0 0 0 1px ${glow}22, 0 20px 60px rgba(0,0,0,0.5), 0 0 60px ${glow}22`
        : '0 20px 60px rgba(0,0,0,0.45)',
      ...style,
    }}
  >
    {children}
  </div>
)

/** Browser window chrome around a dashboard screenshot. */
export const BrowserFrame: React.FC<{
  width: number
  height: number
  url?: string
  style?: React.CSSProperties
  children: React.ReactNode
}> = ({ width, height, url = '127.0.0.1:9090', style, children }) => (
  <div
    style={{
      width,
      height,
      borderRadius: 18,
      overflow: 'hidden',
      background: C.window,
      border: `1.5px solid ${C.lineHi}`,
      boxShadow: '0 40px 120px rgba(0,0,0,0.65), 0 0 0 1px rgba(255,255,255,0.03)',
      display: 'flex',
      flexDirection: 'column',
      ...style,
    }}
  >
    <div
      style={{
        height: 46,
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        gap: 10,
        padding: '0 18px',
        background: '#0c0d0e',
        borderBottom: `1px solid ${C.line}`,
      }}
    >
      {['#ff5f57', '#febc2e', '#28c840'].map((c) => (
        <span key={c} style={{ width: 13, height: 13, borderRadius: 7, background: c, opacity: 0.85 }} />
      ))}
      <div
        style={{
          marginLeft: 18,
          flex: 1,
          maxWidth: 520,
          height: 28,
          borderRadius: 8,
          background: 'rgba(255,255,255,0.06)',
          display: 'flex',
          alignItems: 'center',
          padding: '0 14px',
          fontFamily: MONO,
          fontSize: 15,
          color: C.t2,
        }}
      >
        {url}
      </div>
    </div>
    <div style={{ position: 'relative', flex: 1, overflow: 'hidden' }}>{children}</div>
  </div>
)

export const Shot: React.FC<{ name: string; style?: React.CSSProperties }> = ({ name, style }) => (
  <Img
    src={staticFile(`shots/${name}.png`)}
    style={{ width: '100%', display: 'block', ...style }}
  />
)

/** Lucide-style stroke icons, drawn inline so nothing loads at render time. */
const ICONS: Record<string, React.ReactNode> = {
  activity: <path d="M22 12h-4l-3 9L9 3l-3 9H2" />,
  trend: (
    <>
      <polyline points="22 7 13.5 15.5 8.5 10.5 2 17" />
      <polyline points="16 7 22 7 22 13" />
    </>
  ),
  wrench: (
    <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />
  ),
  bell: (
    <>
      <path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" />
      <path d="M10.3 21a1.94 1.94 0 0 0 3.4 0" />
    </>
  ),
  terminal: (
    <>
      <polyline points="4 17 10 11 4 5" />
      <line x1="12" x2="20" y1="19" y2="19" />
    </>
  ),
  monitor: (
    <>
      <rect width="20" height="14" x="2" y="3" rx="2" />
      <line x1="8" x2="16" y1="21" y2="21" />
      <line x1="12" x2="12" y1="17" y2="21" />
    </>
  ),
  shield: (
    <path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" />
  ),
  cpu: (
    <>
      <rect width="16" height="16" x="4" y="4" rx="2" />
      <rect width="6" height="6" x="9" y="9" rx="1" />
      <path d="M15 2v2M15 20v2M2 15h2M2 9h2M20 15h2M20 9h2M9 2v2M9 20v2" />
    </>
  ),
  server: (
    <>
      <rect width="20" height="8" x="2" y="2" rx="2" />
      <rect width="20" height="8" x="2" y="14" rx="2" />
      <line x1="6" x2="6.01" y1="6" y2="6" />
      <line x1="6" x2="6.01" y1="18" y2="18" />
    </>
  ),
  database: (
    <>
      <ellipse cx="12" cy="5" rx="9" ry="3" />
      <path d="M3 5v14a9 3 0 0 0 18 0V5" />
      <path d="M3 12a9 3 0 0 0 18 0" />
    </>
  ),
  mail: (
    <>
      <rect width="20" height="16" x="2" y="4" rx="2" />
      <path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7" />
    </>
  ),
  globe: (
    <>
      <circle cx="12" cy="12" r="10" />
      <path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20" />
      <path d="M2 12h20" />
    </>
  ),
  check: <path d="M20 6 9 17l-5-5" />,
  zap: (
    <path d="M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z" />
  ),
  lock: (
    <>
      <rect width="18" height="11" x="3" y="11" rx="2" ry="2" />
      <path d="M7 11V7a5 5 0 0 1 10 0v4" />
    </>
  ),
  power: (
    <>
      <path d="M12 2v10" />
      <path d="M18.4 6.6a9 9 0 1 1-12.77.04" />
    </>
  ),
  gauge: (
    <>
      <path d="m12 14 4-4" />
      <path d="M3.34 19a10 10 0 1 1 17.32 0" />
    </>
  ),
  flask: (
    <>
      <path d="M10 2v7.527a2 2 0 0 1-.211.896L4.72 20.55a1 1 0 0 0 .9 1.45h12.76a1 1 0 0 0 .9-1.45l-5.069-10.127A2 2 0 0 1 14 9.527V2" />
      <path d="M8.5 2h7" />
      <path d="M7 16h10" />
    </>
  ),
  clock: (
    <>
      <circle cx="12" cy="12" r="10" />
      <polyline points="12 6 12 12 16 14" />
    </>
  ),
  alert: (
    <>
      <circle cx="12" cy="12" r="10" />
      <line x1="12" x2="12" y1="8" y2="12" />
      <line x1="12" x2="12.01" y1="16" y2="16" />
    </>
  ),
  eye: (
    <>
      <path d="M2.06 12.35a1 1 0 0 1 0-.7 10.75 10.75 0 0 1 19.88 0 1 1 0 0 1 0 .7 10.75 10.75 0 0 1-19.88 0" />
      <circle cx="12" cy="12" r="3" />
    </>
  ),
  github: (
    <path d="M15 22v-4a4.8 4.8 0 0 0-1-3.5c3 0 6-2 6-5.5.08-1.25-.27-2.48-1-3.5.28-1.15.28-2.35 0-3.5 0 0-1 0-3 1.5-2.64-.5-5.36-.5-8 0C6 2 5 2 5 2c-.3 1.15-.3 2.35 0 3.5A5.403 5.403 0 0 0 4 9c0 3.5 3 5.5 6 5.5-.39.49-.68 1.05-.85 1.65-.17.6-.22 1.23-.15 1.85v4" />
  ),
}

export const Icon: React.FC<{ name: keyof typeof ICONS | string; size?: number; color?: string; stroke?: number }> = ({
  name,
  size = 32,
  color = C.t1,
  stroke = 2,
}) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    fill="none"
    stroke={color}
    strokeWidth={stroke}
    strokeLinecap="round"
    strokeLinejoin="round"
    style={{ flexShrink: 0 }}
  >
    {ICONS[name]}
  </svg>
)

/** The NIMon mark: a rounded tile with a heartbeat trace. */
export const LogoMark: React.FC<{ size?: number; draw?: number }> = ({ size = 140, draw = 1 }) => {
  const len = 60
  return (
    <svg width={size} height={size} viewBox="0 0 48 48">
      <defs>
        <linearGradient id="lm" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor={C.info} />
          <stop offset="1" stopColor={C.ok} />
        </linearGradient>
      </defs>
      <rect x="3" y="3" width="42" height="42" rx="11" fill="#0d0f11" stroke="url(#lm)" strokeWidth="2.4" />
      <path
        d="M9 25h7l3-8 5 15 4-11 2 4h9"
        fill="none"
        stroke="url(#lm)"
        strokeWidth="3"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeDasharray={len}
        strokeDashoffset={len * (1 - draw)}
      />
    </svg>
  )
}
