import React from 'react'
import { AbsoluteFill, Audio, staticFile } from 'remotion'
import { Sfx } from './sfx'
import { linearTiming, TransitionSeries } from '@remotion/transitions'
import { fade } from '@remotion/transitions/fade'
import { slide } from '@remotion/transitions/slide'
import { C } from './theme'
import { Intro } from './scenes/Intro'
import { Problem } from './scenes/Problem'
import { Pillars } from './scenes/Pillars'
import { Widget } from './scenes/Widget'
import { Architecture } from './scenes/Architecture'
import { Dashboard } from './scenes/Dashboard'
import { Predict } from './scenes/Predict'
import { Alerts } from './scenes/Alerts'
import { Heal } from './scenes/Heal'
import { Everywhere } from './scenes/Everywhere'
import { BuiltFor } from './scenes/BuiltFor'
import { Outro } from './scenes/Outro'

export const SCENES: { id: string; frames: number; C: React.FC }[] = [
  { id: 'intro', frames: 150, C: Intro },
  { id: 'problem', frames: 240, C: Problem },
  { id: 'pillars', frames: 165, C: Pillars },
  { id: 'widget', frames: 420, C: Widget },
  { id: 'architecture', frames: 330, C: Architecture },
  { id: 'dashboard', frames: 330, C: Dashboard },
  { id: 'predict', frames: 270, C: Predict },
  { id: 'alerts', frames: 270, C: Alerts },
  { id: 'heal', frames: 300, C: Heal },
  { id: 'everywhere', frames: 300, C: Everywhere },
  { id: 'builtfor', frames: 210, C: BuiltFor },
  { id: 'outro', frames: 180, C: Outro },
]

export const TRANSITION = 16

export const TOTAL_FRAMES =
  SCENES.reduce((sum, s) => sum + s.frames, 0) - TRANSITION * (SCENES.length - 1)

/** Frame at which each scene (after the first) starts, i.e. its transition begins. */
const TRANSITION_STARTS = SCENES.slice(1).reduce<number[]>((acc, _, i) => {
  const prev = i === 0 ? 0 : acc[i - 1]
  acc.push(prev + SCENES[i].frames - TRANSITION)
  return acc
}, [])

export const Promo: React.FC = () => (
  <AbsoluteFill style={{ background: C.bg }}>
    <Audio src={staticFile('audio/music.wav')} volume={0.55} />
    {TRANSITION_STARTS.map((f) => (
      <Sfx key={f} at={Math.max(0, f - 6)} name="whoosh" volume={0.3} />
    ))}
    <TransitionSeries>
      {SCENES.flatMap((s, i) => {
        const seq = (
          <TransitionSeries.Sequence key={s.id} durationInFrames={s.frames}>
            <s.C />
          </TransitionSeries.Sequence>
        )
        if (i === 0) return [seq]
        const presentation = i % 3 === 0 ? slide({ direction: 'from-right' }) : fade()
        return [
          <TransitionSeries.Transition
            key={`t-${s.id}`}
            presentation={presentation}
            timing={linearTiming({ durationInFrames: TRANSITION })}
          />,
          seq,
        ]
      })}
    </TransitionSeries>
  </AbsoluteFill>
)
