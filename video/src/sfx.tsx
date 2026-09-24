import React from 'react'
import { Audio, Sequence, staticFile } from 'remotion'

export type SfxName = 'whoosh' | 'pop' | 'tick' | 'beep' | 'alarm' | 'chime' | 'ping' | 'impact'

/** One sound effect starting at scene-local frame `at`. */
export const Sfx: React.FC<{ at: number; name: SfxName; volume?: number }> = ({ at, name, volume = 0.5 }) => (
  <Sequence from={at} durationInFrames={75} layout="none">
    <Audio src={staticFile(`audio/${name}.wav`)} volume={volume} />
  </Sequence>
)

/** Keyboard clicks while `chars` characters are typed at `speed` chars/frame. */
export const Typing: React.FC<{ start: number; chars: number; speed: number; volume?: number }> = ({
  start,
  chars,
  speed,
  volume = 0.28,
}) => {
  const frames = Math.ceil(chars / speed)
  const clicks: React.ReactNode[] = []
  for (let f = 0; f < frames; f += 2) {
    const variant = ((f * 7 + chars) % 3) + 1
    const v = volume * (0.75 + ((f * 13) % 5) / 16)
    clicks.push(
      <Sequence key={f} from={start + f} durationInFrames={6} layout="none">
        <Audio src={staticFile(`audio/key${variant}.wav`)} volume={v} />
      </Sequence>,
    )
  }
  return <>{clicks}</>
}
