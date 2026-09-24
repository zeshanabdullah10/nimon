import React from 'react'
import { Composition } from 'remotion'
import { Poster } from './Poster'
import { Promo, SCENES, TOTAL_FRAMES } from './Promo'
import { FPS, H, W } from './theme'

export const RemotionRoot: React.FC = () => (
  <>
    <Composition id="NimonPromo" component={Promo} durationInFrames={TOTAL_FRAMES} fps={FPS} width={W} height={H} />
    <Composition id="Poster" component={Poster} durationInFrames={150} fps={FPS} width={W} height={H} />
    {SCENES.map((s) => (
      <Composition key={s.id} id={`scene-${s.id}`} component={s.C} durationInFrames={s.frames} fps={FPS} width={W} height={H} />
    ))}
  </>
)
