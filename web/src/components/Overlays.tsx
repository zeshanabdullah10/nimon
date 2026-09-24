import { useEffect, useRef, useState } from 'react'
import { CircleCheck, Info, KeyRound, OctagonAlert, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import { announceStore, dismissToast, toastStore, tokenPromptStore } from '@/hooks/nimon'
import { useStore } from '@/lib/store'
import { Modal } from '@/components/ui'

export function Toaster() {
  const toasts = useStore(toastStore)
  return (
    <div role="region" aria-label="Notifications" className="pointer-events-none fixed bottom-3 right-3 z-[60] flex w-[min(360px,calc(100vw-24px))] flex-col gap-2">
      <div aria-live="polite" className="flex flex-col gap-2">
        {toasts.map((t) => {
          const Icon = t.kind === 'ok' ? CircleCheck : t.kind === 'error' ? OctagonAlert : Info
          return (
            <div key={t.id} role={t.kind === 'error' ? 'alert' : 'status'}
              className={cn('pointer-events-auto flex items-start gap-2.5 rounded-lg border bg-card p-3 shadow-xl',
                t.kind === 'error' ? 'border-crit/50' : t.kind === 'ok' ? 'border-ok/40' : 'border-fg/15')}>
              <Icon aria-hidden className={cn('mt-0.5 h-4 w-4 shrink-0', t.kind === 'error' ? 'text-crit' : t.kind === 'ok' ? 'text-ok' : 'text-info')} />
              <div className="min-w-0 flex-1">
                <div className="text-[13px] font-semibold">{t.title}</div>
                {t.detail && <div className="mt-0.5 break-words text-xs text-t2">{t.detail}</div>}
              </div>
              <button className="btn btn-ghost btn-sm -m-1 px-1.5" onClick={() => dismissToast(t.id)} aria-label="Dismiss notification">
                <X aria-hidden className="h-3.5 w-3.5" />
              </button>
            </div>
          )
        })}
      </div>
    </div>
  )
}

/** Screen-reader announcements (status changes). */
export function LiveRegion() {
  const a = useStore(announceStore)
  return (
    <>
      <div className="sr-only" aria-live="polite" aria-atomic="true">{!a.urgent ? a.text : ''}</div>
      <div className="sr-only" aria-live="assertive" aria-atomic="true">{a.urgent ? a.text : ''}</div>
    </>
  )
}

export function TokenDialog() {
  const prompt = useStore(tokenPromptStore)
  const [value, setValue] = useState('')
  const input = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (!prompt) return
    setValue('')
    requestAnimationFrame(() => input.current?.focus())
  }, [prompt])
  const cancel = () => prompt?.resolve(null)
  return (
    <Modal open={!!prompt} onClose={cancel} title={<span className="inline-flex items-center gap-2"><KeyRound aria-hidden className="h-4 w-4" />API token required</span>}
      footer={<>
        <button type="button" className="btn" onClick={cancel}>Cancel</button>
        <button type="submit" form="token-form" className="btn btn-primary" disabled={!value.trim()}>Save &amp; retry</button>
      </>}>
      <form id="token-form" onSubmit={(e) => { e.preventDefault(); if (value.trim()) prompt?.resolve(value.trim()) }}>
        <p className="mb-3 text-t1">{prompt?.message}</p>
        <label className="label" htmlFor="token-input">Bearer token (hub <code className="font-mono">auth.api_token</code> / <code className="font-mono">NIMON_API_TOKEN</code>)</label>
        <input ref={input} id="token-input" className="input font-mono" type="password" autoComplete="off" autoFocus
          value={value} onChange={(e) => setValue(e.target.value)} />
        <p className="mt-2 text-xs text-t2">Stored in this browser's localStorage. Clear it any time in Settings.</p>
      </form>
    </Modal>
  )
}
