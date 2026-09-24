// Synthesizes the promo soundtrack (music bed + sound effects) as WAV files.
// Everything is generated from code: no samples, no licensing questions.
// Usage: node scripts/synth-audio.mjs   → public/audio/*.wav
import { mkdirSync, writeFileSync } from 'node:fs'

const SR = 48000
const OUT = new URL('../public/audio/', import.meta.url)
mkdirSync(OUT, { recursive: true })

// ---------------------------------------------------------------- helpers
const TAU = Math.PI * 2
const midi = (n) => 440 * Math.pow(2, (n - 69) / 12)
let seed = 1234567
const rnd = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 4294967296) * 2 - 1

function writeWav(name, L, R = L) {
  const n = L.length
  const buf = Buffer.alloc(44 + n * 4)
  buf.write('RIFF', 0)
  buf.writeUInt32LE(36 + n * 4, 4)
  buf.write('WAVEfmt ', 8)
  buf.writeUInt32LE(16, 16)
  buf.writeUInt16LE(1, 20)
  buf.writeUInt16LE(2, 22)
  buf.writeUInt32LE(SR, 24)
  buf.writeUInt32LE(SR * 4, 28)
  buf.writeUInt16LE(4, 32)
  buf.writeUInt16LE(16, 34)
  buf.write('data', 36)
  buf.writeUInt32LE(n * 4, 40)
  for (let i = 0; i < n; i++) {
    buf.writeInt16LE(Math.round(Math.max(-1, Math.min(1, L[i])) * 32767), 44 + i * 4)
    buf.writeInt16LE(Math.round(Math.max(-1, Math.min(1, R[i])) * 32767), 46 + i * 4)
  }
  writeFileSync(new URL(name, OUT), buf)
  console.log('wrote', name, (n / SR).toFixed(2) + 's')
}

function normalize(chs, peak = 0.89) {
  let m = 0
  for (const c of chs) for (const v of c) m = Math.max(m, Math.abs(v))
  const g = m > 0 ? peak / m : 1
  for (const c of chs) for (let i = 0; i < c.length; i++) c[i] *= g
}

/** One-pole low-pass, in place. cutoff may be a function of sample index. */
function lowpass(x, cutoff) {
  let y = 0
  for (let i = 0; i < x.length; i++) {
    const fc = typeof cutoff === 'function' ? cutoff(i) : cutoff
    const a = 1 - Math.exp((-TAU * fc) / SR)
    y += a * (x[i] - y)
    x[i] = y
  }
  return x
}

/** Simple state-variable band-pass (returns new array). */
function bandpass(x, fcFn, q = 1.2) {
  const out = new Float32Array(x.length)
  let low = 0
  let band = 0
  for (let i = 0; i < x.length; i++) {
    const fc = typeof fcFn === 'function' ? fcFn(i) : fcFn
    const f = 2 * Math.sin((Math.PI * Math.min(fc, SR / 6)) / SR)
    low += f * band
    const high = x[i] - low - band / q
    band += f * high
    out[i] = band
  }
  return out
}

/** Small Schroeder reverb (mono in → stereo out, wet only). */
function reverb(x, { decay = 0.84, mix = 1 } = {}) {
  const combsL = [1557, 1617, 1491, 1422].map((d) => Math.round((d * SR) / 44100))
  const combsR = combsL.map((d) => d + 23)
  const aps = [225, 556].map((d) => Math.round((d * SR) / 44100))
  const run = (delays) => {
    const out = new Float32Array(x.length)
    for (const d of delays) {
      const buf = new Float32Array(d)
      let idx = 0
      let lp = 0
      for (let i = 0; i < x.length; i++) {
        const y = buf[idx]
        lp = y * 0.7 + lp * 0.3
        buf[idx] = x[i] + lp * decay
        out[i] += y / delays.length
        idx = (idx + 1) % d
      }
    }
    for (const d of aps) {
      const buf = new Float32Array(d)
      let idx = 0
      for (let i = 0; i < out.length; i++) {
        const b = buf[idx]
        const y = -out[i] + b
        buf[idx] = out[i] + b * 0.5
        out[i] = y
        idx = (idx + 1) % d
      }
    }
    for (let i = 0; i < out.length; i++) out[i] *= mix
    return out
  }
  return [run(combsL), run(combsR)]
}

const env = (t, a, d) => (t < 0 ? 0 : t < a ? t / a : Math.exp(-(t - a) / d))

// ------------------------------------------------------------------ music
// Timeline mirrors src/Promo.tsx: scene frames with 16-frame transitions.
const FPS = 30
const TRANSITION = 16
// intro, problem, pillars, widget, architecture, dashboard, predict, alerts, heal, everywhere, builtfor, outro
const SCENE_FRAMES = [150, 240, 165, 420, 330, 330, 270, 270, 300, 300, 210, 180]
const TOTAL = SCENE_FRAMES.reduce((a, b) => a + b, 0) - TRANSITION * (SCENE_FRAMES.length - 1)
const DUR = TOTAL / FPS
const N = Math.ceil(DUR * SR)
const START = SCENE_FRAMES.map((_, i) =>
  SCENE_FRAMES.slice(0, i).reduce((a, f) => a + f - TRANSITION, 0) / FPS,
)

const BPM = 100
const BEAT = 60 / BPM // 0.6 s
const BAR = BEAT * 4 // 2.4 s

// Sections (seconds), snapped to bars counted from the lift
const T_PROBLEM = 4.8
const T_LIFT = 12.0 // "Meet NIMon": drums enter
const snap = (t) => T_LIFT + Math.round((t - T_LIFT) / BAR) * BAR
const T_GROOVE = snap(START[3]) // widget scene onwards: full kit
const T_ARP = snap(START[4]) // architecture onwards: arpeggio
const T_DRUMS_END = snap(START[11]) // outro
const T_ARP_END = T_DRUMS_END - BAR
const T_END = DUR
console.log('sections', { T_GROOVE, T_ARP, T_ARP_END, T_DRUMS_END, DUR: DUR.toFixed(2) })

// Chords: [pad notes], bass root (MIDI)
const CH = {
  Am: { pad: [57, 60, 64, 67, 71], bass: 45 },
  F: { pad: [53, 57, 60, 64, 67], bass: 41 },
  C: { pad: [48, 55, 60, 64, 67], bass: 48 },
  G: { pad: [55, 59, 62, 66, 69], bass: 43 },
  Dim: { pad: [57, 58, 64, 70], bass: 45 }, // tension: A + Bb cluster
}
const PROG = ['Am', 'F', 'C', 'G']

function chordAt(t) {
  if (t < T_PROBLEM) return 'Am'
  if (t < T_LIFT) return t < 9.6 ? 'Am' : 'Dim'
  if (t >= T_DRUMS_END) return 'Am'
  const bar = Math.floor((t - T_LIFT) / BAR)
  return PROG[Math.floor(bar / 2) % 4]
}

const L = new Float32Array(N)
const R = new Float32Array(N)
const padBus = new Float32Array(N)
const arpBus = new Float32Array(N)

// Kick times (for sidechain) — four on the floor from T_LIFT (half-time until T_GROOVE)
const kicks = []
for (let t = T_LIFT; t < T_DRUMS_END - 0.01; t += BEAT) {
  if (t < T_GROOVE && Math.round((t - T_LIFT) / BEAT) % 2 === 1) continue
  kicks.push(t)
}
const duck = new Float32Array(N).fill(1)
for (const k of kicks) {
  const s = Math.floor(k * SR)
  for (let i = 0; i < SR * 0.4 && s + i < N; i++) {
    duck[s + i] = Math.min(duck[s + i], 1 - 0.55 * Math.exp(-i / SR / 0.12))
  }
}

// Master level envelope (fade in/out)
const master = (t) => Math.min(1, t / 1.5) * Math.min(1, (T_END - t) / 2.5)

// --- pad: additive, chorused, crossfaded chord changes
{
  const XF = 0.5 // crossfade seconds
  // Per-chord oscillator banks so shared notes never double-advance.
  const names = Object.keys(CH)
  const banks = Object.fromEntries(
    names.map((name) => [
      name,
      CH[name].pad.flatMap((n) => [-0.08, 0.08].map((det) => ({ inc: (TAU * midi(n + det)) / SR, ph: rnd() * TAU }))),
    ]),
  )
  let cur = chordAt(0)
  let prev = cur
  let changedAt = -XF
  for (let i = 0; i < N; i++) {
    const t = i / SR
    const c = chordAt(t)
    if (c !== cur) {
      prev = cur
      cur = c
      changedAt = t
    }
    const wCur = Math.min(1, (t - changedAt) / XF)
    let v = 0
    for (const [name, w] of wCur >= 1 ? [[cur, 1]] : [[cur, wCur], [prev, 1 - wCur]]) {
      for (const osc of banks[name]) {
        osc.ph += osc.inc
        v += w * (Math.sin(osc.ph) + 0.35 * Math.sin(2 * osc.ph) + 0.12 * Math.sin(3 * osc.ph))
      }
    }
    // brighter from the lift; darker in the problem section
    const bright = t < T_PROBLEM ? 0.6 : t < T_LIFT ? 0.35 : t < T_DRUMS_END ? 1 : 0.7
    padBus[i] = v * 0.05 * bright * master(t)
  }
  lowpass(padBus, (i) => 900 + 700 * Math.sin((i / SR) * 0.35))
}

// --- problem-section drone + clock tick
{
  for (let i = Math.floor(T_PROBLEM * SR); i < Math.floor(T_LIFT * SR); i++) {
    const t = i / SR
    const a = Math.min(1, (t - T_PROBLEM) / 2) * Math.min(1, (T_LIFT - t) / 0.8)
    const v = Math.sin(TAU * midi(33) * t) * 0.5 + Math.sin(TAU * midi(33.1) * t) * 0.5
    const s = v * 0.22 * a
    L[i] += s
    R[i] += s
  }
  for (let t = T_PROBLEM + BEAT; t < T_LIFT - 0.3; t += BEAT) {
    const s = Math.floor(t * SR)
    for (let i = 0; i < SR * 0.03; i++) {
      const e = Math.exp(-i / SR / 0.006)
      const v = Math.sin((TAU * 3200 * i) / SR) * e * 0.07
      L[s + i] += v * 0.8
      R[s + i] += v
    }
  }
}

// --- bass: 8ths with pluck envelope, ducked
{
  for (let t = T_LIFT; t < T_DRUMS_END + BAR; t += BEAT / 2) {
    const beatIdx = Math.round((t - T_LIFT) / (BEAT / 2))
    if (t < T_GROOVE && beatIdx % 2 === 1) continue
    const root = CH[chordAt(t + 0.001)].bass
    const f = midi(root + (beatIdx % 8 === 7 ? 12 : 0))
    const s = Math.floor(t * SR)
    const len = Math.floor(SR * 0.28)
    for (let i = 0; i < len && s + i < N; i++) {
      const tt = i / SR
      const e = env(tt, 0.004, 0.16)
      const ph = TAU * f * tt
      const tri = (2 / Math.PI) * Math.asin(Math.sin(ph))
      const v = (tri * 0.8 + Math.sin(ph) * 0.6) * e * 0.22 * master(t + tt)
      L[s + i] += v
      R[s + i] += v
    }
  }
}

// --- drums
function addKick(t, gain = 1) {
  const s = Math.floor(t * SR)
  let ph = 0
  for (let i = 0; i < SR * 0.45 && s + i < N; i++) {
    const tt = i / SR
    const f = 45 + 95 * Math.exp(-tt / 0.03)
    ph += (TAU * f) / SR
    const v = (Math.sin(ph) * Math.exp(-tt / 0.22) + rnd() * Math.exp(-tt / 0.002) * 0.3) * 0.62 * gain
    L[s + i] += v
    R[s + i] += v
  }
}
function addHat(t, gain = 1, open = false) {
  const s = Math.floor(t * SR)
  let prev = 0
  const d = open ? 0.09 : 0.03
  for (let i = 0; i < SR * (open ? 0.25 : 0.08) && s + i < N; i++) {
    const n = rnd()
    const hp = n - prev
    prev = n
    const v = hp * Math.exp(-i / SR / d) * 0.075 * gain
    L[s + i] += v * 0.7
    R[s + i] += v
  }
}
function addClap(t, gain = 1) {
  const s = Math.floor(t * SR)
  const len = Math.floor(SR * 0.25)
  const noise = new Float32Array(len)
  for (let i = 0; i < len; i++) {
    const tt = i / SR
    const bursts = [0, 0.008, 0.016].reduce((acc, o) => acc + (tt >= o ? Math.exp(-(tt - o) / 0.004) : 0), 0)
    noise[i] = rnd() * (bursts * 0.5 + Math.exp(-tt / 0.07))
  }
  const bp = bandpass(noise, 1400, 1.4)
  for (let i = 0; i < len && s + i < N; i++) {
    const v = bp[i] * 0.2 * gain
    L[s + i] += v
    R[s + i] += v * 0.9
  }
}
for (const k of kicks) addKick(k, k < T_GROOVE ? 0.8 : 1)
for (let t = T_GROOVE; t < T_DRUMS_END - 0.01; t += BEAT) {
  addHat(t + BEAT / 2, 1, Math.round((t - T_GROOVE) / BEAT) % 4 === 3)
  const b = Math.round((t - T_GROOVE) / BEAT) % 4
  if (b === 1 || b === 3) addClap(t)
}
for (let t = T_GROOVE; t < T_DRUMS_END - 0.01; t += BEAT / 4) addHat(t, 0.35)
// crash-ish swell into the outro
addHat(T_DRUMS_END, 2.2, true)

// --- arp: 16th plucks over chord tones
{
  const pattern = [0, 2, 1, 3, 2, 4, 3, 1]
  let step = 0
  for (let t = T_ARP; t < T_ARP_END; t += BEAT / 4, step++) {
    const notes = CH[chordAt(t + 0.001)].pad
    const n = notes[pattern[step % pattern.length] % notes.length] + 12
    const f = midi(n)
    const s = Math.floor(t * SR)
    const fadeIn = Math.min(1, (t - T_ARP) / 4)
    const fadeOut = Math.min(1, (T_ARP_END - t) / 3)
    for (let i = 0; i < SR * 0.35 && s + i < N; i++) {
      const tt = i / SR
      const e = env(tt, 0.002, 0.11)
      const v = (Math.sin(TAU * f * tt) + 0.3 * Math.sin(TAU * 2 * f * tt)) * e * 0.05 * fadeIn * fadeOut
      arpBus[s + i] += v
    }
  }
}

// --- outro swell: sustained Am(add9) chord
{
  const notes = [45, 57, 64, 67, 71, 76]
  const s0 = Math.floor(T_DRUMS_END * SR)
  for (let i = s0; i < N; i++) {
    const t = i / SR
    const a = Math.min(1, (t - T_DRUMS_END) / 1.2) * master(t)
    let v = 0
    for (const n of notes) v += Math.sin(TAU * midi(n) * t) + 0.2 * Math.sin(TAU * midi(n) * 2 * t)
    padBus[i] += v * 0.028 * a
  }
}

// --- mix: sidechain pad/arp, add reverb sends
{
  for (let i = 0; i < N; i++) padBus[i] *= duck[i]
  const [pl, pr] = reverb(padBus, { decay: 0.86, mix: 0.9 })
  const [al, ar] = reverb(arpBus, { decay: 0.8, mix: 0.7 })
  for (let i = 0; i < N; i++) {
    const pan = Math.sin((i / SR) * 1.3) * 0.3
    L[i] += padBus[i] * 0.8 + pl[i] + arpBus[i] * (0.8 + pan) + al[i]
    R[i] += padBus[i] * 0.8 + pr[i] + arpBus[i] * (0.8 - pan) + ar[i]
    // gentle glue: soft clip
    L[i] = Math.tanh(L[i] * 1.2) / 1.2
    R[i] = Math.tanh(R[i] * 1.2) / 1.2
  }
  normalize([L, R], 0.89)
  writeWav('music.wav', L, R)
}

// ------------------------------------------------------------------- sfx
function sfx(name, dur, fn, { reverbMix = 0, peak = 0.9 } = {}) {
  const n = Math.floor(dur * SR)
  const l = new Float32Array(n)
  const r = new Float32Array(n)
  for (let i = 0; i < n; i++) {
    const [a, b] = fn(i / SR, i)
    l[i] = a
    r[i] = b ?? a
  }
  if (reverbMix > 0) {
    const mono = new Float32Array(n)
    for (let i = 0; i < n; i++) mono[i] = (l[i] + r[i]) / 2
    const [wl, wr] = reverb(mono, { decay: 0.8, mix: reverbMix })
    for (let i = 0; i < n; i++) {
      l[i] += wl[i]
      r[i] += wr[i]
    }
  }
  normalize([l, r], peak)
  writeWav(name, l, r)
}

// whoosh: band-passed noise sweep with a pan sweep
{
  const dur = 0.7
  const n = Math.floor(dur * SR)
  const noise = new Float32Array(n)
  for (let i = 0; i < n; i++) noise[i] = rnd()
  const bp = bandpass(noise, (i) => 300 + 3200 * Math.pow(i / n, 1.4), 0.9)
  sfx('whoosh.wav', dur, (t, i) => {
    const e = Math.sin(Math.PI * Math.min(1, t / dur)) ** 2
    const p = t / dur
    return [bp[i] * e * (1 - p * 0.6), bp[i] * e * (0.4 + p * 0.6)]
  }, { peak: 0.7 })
}

// pop: soft UI pop
sfx('pop.wav', 0.25, (t) => {
  const f = 520 + 380 * Math.exp(-t / 0.02)
  const v = Math.sin(TAU * f * t) * env(t, 0.002, 0.05) + Math.sin(TAU * 2 * f * t) * env(t, 0.001, 0.02) * 0.3
  return [v]
}, { reverbMix: 0.25, peak: 0.6 })

// tick: tiny UI tick
sfx('tick.wav', 0.08, (t) => [Math.sin(TAU * 2400 * t) * env(t, 0.0008, 0.012)], { peak: 0.45 })

// beep: patient-monitor heartbeat beep
sfx('beep.wav', 0.3, (t) => {
  const g = t < 0.11 ? Math.min(1, t / 0.004) * Math.min(1, (0.11 - t) / 0.01) : 0
  return [(Math.sin(TAU * 988 * t) + 0.15 * Math.sin(TAU * 1976 * t)) * g]
}, { reverbMix: 0.35, peak: 0.55 })

// alarm: two-tone failure alarm
sfx('alarm.wav', 1.1, (t) => {
  const seg = Math.floor(t / 0.22)
  const f = seg % 2 === 0 ? 880 : 659
  const local = t - seg * 0.22
  const g = seg < 4 ? Math.min(1, local / 0.01) * Math.min(1, (0.2 - local) / 0.02) : 0
  const sq = Math.tanh(Math.sin(TAU * f * t) * 3)
  return [sq * Math.max(0, g) * 0.8]
}, { reverbMix: 0.3, peak: 0.6 })

// chime: success (rising bell pair)
function bell(t, f, start) {
  const tt = t - start
  if (tt < 0) return 0
  return (Math.sin(TAU * f * tt) + 0.4 * Math.sin(TAU * f * 2.76 * tt) * Math.exp(-tt / 0.08)) * env(tt, 0.002, 0.35)
}
sfx('chime.wav', 1.4, (t) => [bell(t, 1046.5, 0) * 0.7 + bell(t, 1568, 0.09)], { reverbMix: 0.45, peak: 0.6 })

// ping: softer warning ping (descending)
sfx('ping.wav', 1.2, (t) => [bell(t, 1174.7, 0) + bell(t, 880, 0.1) * 0.8], { reverbMix: 0.4, peak: 0.5 })

// impact: logo hit — sub drop + noise
sfx('impact.wav', 2.2, (t) => {
  const f = 40 + 70 * Math.exp(-t / 0.08)
  const sub = Math.sin(TAU * f * t) * env(t, 0.003, 0.6)
  const hit = rnd() * env(t, 0.001, 0.05) * 0.5
  return [sub + hit]
}, { reverbMix: 0.6, peak: 0.85 })

// key clicks (three variants)
for (const [k, f0] of [[1, 3800], [2, 4400], [3, 3300]]) {
  const n = Math.floor(0.05 * SR)
  const noise = new Float32Array(n)
  for (let i = 0; i < n; i++) noise[i] = rnd()
  const bp = bandpass(noise, f0, 2)
  sfx(`key${k}.wav`, 0.05, (t, i) => [bp[i] * env(t, 0.0005, 0.006) + Math.sin(TAU * 180 * t) * env(t, 0.001, 0.008) * 0.4], { peak: 0.5 })
}
