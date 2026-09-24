// Capture the REAL widget UI (crates/nimon-widget/ui) in headless Edge, with
// the Tauri bridge stubbed so `poll` / `alert_action` hit a live hub.
// Transparent PNGs, so the video can composite them over a desktop.
// Usage: HUB=http://127.0.0.1:9096 node scripts/capture-widget.mjs
import puppeteer from 'puppeteer-core'
import { mkdirSync } from 'node:fs'
import { fileURLToPath, pathToFileURL } from 'node:url'

const HUB = process.env.HUB ?? 'http://127.0.0.1:9090'
const EDGE = process.env.EDGE_PATH ?? 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe'
const UI = pathToFileURL(fileURLToPath(new URL('../../crates/nimon-widget/ui/index.html', import.meta.url)))
const OUT = new URL('../public/widget/', import.meta.url)
mkdirSync(OUT, { recursive: true })

// Presentation names for the simulator's edges (tile initials SA / SB)
const EDGE_NAMES = { 'sim-edge-01': 'Station A', 'sim-edge-02': 'Station B' }
const W = 450
const H = 860

async function get(path, key) {
  const r = await fetch(HUB + path)
  const body = await r.json()
  return key ? body[key] : body
}
const res = (data) => ({ data, ok: true, error: null, age_ms: 600 })

async function poll() {
  const [health, edges, devices, alerts, predictions, settings] = await Promise.all([
    get('/health'),
    get('/api/v1/edges', 'edges'),
    get('/api/v1/devices', 'devices'),
    get('/api/v1/alerts', 'alerts'),
    get('/api/v1/predictions', 'predictions'),
    get('/api/v1/settings'),
  ])
  for (const e of edges) e.name = EDGE_NAMES[e.edge_id] ?? e.name
  return {
    hub: HUB, up: true, starting: false, hub_error: null, edge_state: 'running', has_token: false,
    res: {
      health: res(health), edges: res(edges), devices: res(devices),
      alerts: res(alerts), predictions: res(predictions), settings: res(settings),
    },
  }
}

async function alertAction(id, action) {
  const r = await fetch(`${HUB}/api/v1/alerts/${encodeURIComponent(id)}/${action}`, { method: 'POST' })
  if (!r.ok) throw new Error(`HTTP ${r.status}`)
  return r.json()
}

const browser = await puppeteer.launch({ executablePath: EDGE, headless: true, args: ['--hide-scrollbars'] })
const page = await browser.newPage()
await page.setViewport({ width: W, height: H, deviceScaleFactor: 2 })
await page.exposeFunction('__hubPoll', poll)
await page.exposeFunction('__hubAlert', alertAction)
await page.evaluateOnNewDocument(() => {
  const handlers = {}
  window.__tauriHandlers = handlers
  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args = {}) => {
        switch (cmd) {
          case 'poll': return window.__hubPoll()
          case 'alert_action': return window.__hubAlert(args.id, args.action)
          case 'update_geometry': return Math.min(args.height, window.innerHeight)
          case 'get_state': return { locked: false }
          default: return null
        }
      },
    },
    event: {
      listen: async (name, cb) => {
        handlers[name] = cb
        return () => delete handlers[name]
      },
    },
  }
})
page.on('pageerror', (e) => console.error('page error:', e.message))

const wait = (ms) => new Promise((r) => setTimeout(r, ms))
const shot = async (name) => {
  await page.screenshot({ path: fileURLToPath(new URL(`${name}.png`, OUT)), omitBackground: true })
  console.log('captured', name)
}
const collapse = () => page.evaluate(() => document.dispatchEvent(new MouseEvent('mouseleave')))

await page.goto(UI.href, { waitUntil: 'load' })
await wait(2500)
await shot('strip')

await page.hover('#hubTile')
await wait(1200)
await shot('panel-all')

await page.hover('#tiles .tile:nth-child(1)')
await wait(1200)
await shot('panel-a')

await page.hover('#tiles .tile:nth-child(2)')
await wait(1200)
await shot('panel-b')

await page.hover('#tiles .tile:nth-child(1)')
await wait(900)
const ack = await page.$('#alertList .alert button[data-op="acknowledge"]:not([disabled])')
if (ack) {
  await ack.click()
  await wait(2500)
  await shot('panel-a-acked')
} else {
  console.warn('no acknowledgeable alert found')
}

await collapse()
await wait(900)
await page.evaluate(() => window.__tauriHandlers['lock-changed']?.({ payload: true }))
await wait(500)
await shot('strip-locked')

await browser.close()
