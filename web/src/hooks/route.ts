/* Hash routing: #/<view>[/<sub>][?device=<id>] — links are shareable and
 * the browser back button closes the device drawer. */

import { useSyncExternalStore } from 'react'

export type ViewId = 'overview' | 'devices' | 'alerts' | 'edges' | 'predictions' | 'settings'
export const VIEWS: ViewId[] = ['overview', 'devices', 'alerts', 'edges', 'predictions', 'settings']

export interface Route { view: ViewId; sub: string | null; device: string | null }

function parse(hash: string): Route {
  const h = hash.replace(/^#\/?/, '')
  const [path, query = ''] = h.split('?')
  const [v, sub] = path.split('/')
  const view = (VIEWS as string[]).includes(v) ? (v as ViewId) : 'overview'
  const params = new URLSearchParams(query)
  return { view, sub: sub || null, device: params.get('device') }
}

let current = parse(location.hash)
let currentHash = location.hash
const listeners = new Set<() => void>()
window.addEventListener('hashchange', () => {
  currentHash = location.hash
  current = parse(currentHash)
  listeners.forEach((l) => l())
})

export function useRoute(): Route {
  return useSyncExternalStore(
    (l) => { listeners.add(l); return () => listeners.delete(l) },
    () => current,
  )
}

export function href(r: Partial<Route>): string {
  const view = r.view ?? current.view
  const sub = r.sub === undefined ? (r.view && r.view !== current.view ? null : current.sub) : r.sub
  const device = r.device === undefined ? current.device : r.device
  return `#/${view}${sub ? '/' + sub : ''}${device ? '?device=' + encodeURIComponent(device) : ''}`
}

export function navigate(r: Partial<Route>) {
  const h = href(r)
  if (h !== currentHash) location.hash = h
}

export const openDevice = (id: string) => navigate({ device: id })
export const closeDevice = () => navigate({ device: null })
