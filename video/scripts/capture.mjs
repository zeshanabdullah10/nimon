// Capture real dashboard screenshots for the video from a running hub.
// Usage: HUB=http://127.0.0.1:9096 DEVICE="sim-edge-01:PXI1Slot2" node scripts/capture.mjs
import puppeteer from 'puppeteer-core'
import { mkdirSync } from 'node:fs'

const HUB = process.env.HUB ?? 'http://127.0.0.1:9090'
const DEVICE = process.env.DEVICE ?? ''
const EDGE =
  process.env.EDGE_PATH ?? 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe'
const OUT = new URL('../public/shots/', import.meta.url)
mkdirSync(OUT, { recursive: true })

const shots = [
  ['overview', '#/overview'],
  ['modules', '#/devices'],
  ['alerts', '#/alerts'],
  ['history', '#/alerts/history'],
  ['edges', '#/edges'],
  ['predictions', '#/predictions'],
  ['settings', '#/settings'],
  ['device', `#/overview?device=${encodeURIComponent(DEVICE)}`],
]

const browser = await puppeteer.launch({
  executablePath: EDGE,
  headless: true,
  args: ['--hide-scrollbars', '--force-device-scale-factor=1.5'],
})
const page = await browser.newPage()
await page.setViewport({ width: 1600, height: 1000, deviceScaleFactor: 1.5 })
await page.emulateMediaFeatures([{ name: 'prefers-color-scheme', value: 'dark' }])

for (const [name, hash] of shots) {
  if (name === 'device' && !DEVICE) continue
  await page.goto(`${HUB}/${hash}`, { waitUntil: 'networkidle0' })
  // Let the polling data layer fill every card and the chart draw.
  await new Promise((r) => setTimeout(r, 3500))
  const file = new URL(`${name}.png`, OUT)
  await page.screenshot({ path: file, type: 'png' })
  console.log('captured', name)
}

await browser.close()
