import React from 'react'
import { AbsoluteFill } from 'remotion'
import { Intro } from './scenes/Intro'
import { C, FONT } from './theme'

/** README thumbnail: the title scene (render frame 110) with a play button. */
export const Poster: React.FC = () => (
  <AbsoluteFill>
    <Intro />
    <AbsoluteFill style={{ alignItems: 'center', justifyContent: 'flex-end', paddingBottom: 90 }}>
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 22,
          padding: '18px 34px 18px 22px',
          borderRadius: 999,
          background: 'rgba(16,17,18,0.85)',
          border: `2px solid ${C.ok}`,
          boxShadow: `0 0 50px ${C.ok}55`,
        }}
      >
        <div
          style={{
            width: 64,
            height: 64,
            borderRadius: 32,
            background: C.ok,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
          }}
        >
          <svg width={30} height={30} viewBox="0 0 24 24">
            <path d="M8 5v14l11-7z" fill="#07080a" />
          </svg>
        </div>
        <span style={{ fontFamily: FONT, fontSize: 36, fontWeight: 700, color: C.t1 }}>Watch the 86-second tour</span>
      </div>
    </AbsoluteFill>
  </AbsoluteFill>
)
