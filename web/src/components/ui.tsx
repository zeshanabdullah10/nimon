import { memo, useEffect, useId, useRef, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { createStore, useStore } from '@/lib/store'
import {
  CircleCheck, CircleHelp, Clock, FlaskConical, OctagonAlert, PowerOff, TriangleAlert, Unlink, X,
} from 'lucide-react'
import { cn } from '@/lib/utils'
import { SEV_LABEL, type Sev } from '@/lib/severity'
import { ago, clockTime } from '@/lib/format'
import { useNow } from '@/hooks/nimon'

/* ═══ severity: icon + colour + text, never colour alone ══════════════ */

export const SEV_TEXT: Record<Sev, string> = {
  critical: 'text-crit', warning: 'text-warn', nolink: 'text-t2', offline: 'text-t2', unknown: 'text-t3', ok: 'text-ok',
}
export const SEV_BG: Record<Sev, string> = {
  critical: 'bg-crit', warning: 'bg-warn', nolink: 'bg-t3', offline: 'bg-t3', unknown: 'bg-t3', ok: 'bg-ok',
}
const SEV_ICON = {
  critical: OctagonAlert, warning: TriangleAlert, nolink: Unlink, offline: PowerOff, unknown: CircleHelp, ok: CircleCheck,
} as const

export function SevIcon({ sev, stale, className }: { sev: Sev; stale?: boolean; className?: string }) {
  const Icon = stale ? Clock : SEV_ICON[sev]
  return <Icon aria-hidden className={cn('h-4 w-4 shrink-0', stale ? 'text-t3' : SEV_TEXT[sev], className)} strokeWidth={2.2} />
}

export function SevBadge({ sev, stale, label, className }: { sev: Sev; stale?: boolean; label?: string; className?: string }) {
  return (
    <span className={cn(
      'inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] font-semibold uppercase tracking-wide',
      stale ? 'border-fg/15 text-t2' : sevBadgeCls(sev), className)}>
      <SevIcon sev={sev} stale={stale} className="h-3 w-3" />
      {label ?? (stale ? 'Stale' : SEV_LABEL[sev])}
    </span>
  )
}

export function sevBadgeCls(sev: Sev): string {
  switch (sev) {
    case 'critical': return 'border-crit/40 bg-crit/10 text-crit'
    case 'warning': return 'border-warn/40 bg-warn/10 text-warn'
    case 'ok': return 'border-ok/35 bg-ok/10 text-ok'
    default: return 'border-fg/15 bg-fg/5 text-t2'
  }
}

export function SimBadge({ className }: { className?: string }) {
  return (
    <span className={cn('inline-flex items-center gap-1 rounded border border-info/40 bg-info/10 px-1.5 py-px text-[10px] font-bold uppercase tracking-wider text-info', className)}
      title="Simulated device — not real hardware">
      <FlaskConical aria-hidden className="h-3 w-3" /> Sim
    </span>
  )
}

/* ═══ time: each instance subscribes to the 1 s ticker by itself ═══════ */

export const Ago = memo(function Ago({ ms, prefix = '' }: { ms: number | null; prefix?: string }) {
  const now = useNow()
  return (
    <time dateTime={ms ? new Date(ms).toISOString() : undefined} title={ms ? new Date(ms).toLocaleString() : undefined}>
      {prefix}{ago(ms, now)}
    </time>
  )
})

/** "Updated 3s ago" / "Stale — last good 12:03:22" for a resource */
export function Updated({ at, stale, error, className }: { at: number | null; stale?: boolean; error?: string | null; className?: string }) {
  return (
    <span className={cn('inline-flex items-center gap-1 text-xs', stale || error ? 'text-warn' : 'text-t3', className)}>
      {stale || error ? <Clock aria-hidden className="h-3 w-3" /> : null}
      <span>
        {at === null
          ? (error ? 'unavailable' : 'loading…')
          : stale
            ? <>stale · last good {clockTime(at)} (<Ago ms={at} />)</>
            : <>updated <Ago ms={at} /></>}
      </span>
    </span>
  )
}

export function Card({ title, actions, children, className, labelId }: {
  title?: ReactNode; actions?: ReactNode; children: ReactNode; className?: string; labelId?: string
}) {
  const auto = useId()
  const id = labelId ?? auto
  return (
    <section className={cn('card flex min-w-0 flex-col p-4', className)} aria-labelledby={title ? id : undefined}>
      {(title || actions) && (
        <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
          {title && <h2 id={id} className="card-title">{title}</h2>}
          {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
        </div>
      )}
      {children}
    </section>
  )
}

export function Empty({ children }: { children: ReactNode }) {
  return <div className="py-6 text-center text-[13px] text-t2">{children}</div>
}

/** Horizontal meter (percent used), label outside for screen readers */
export function Meter({ value, label, sev }: { value: number | null; label: string; sev: Sev }) {
  const v = value === null ? 0 : Math.max(0, Math.min(100, value))
  return (
    <div role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={value === null ? undefined : Math.round(v)}
      aria-valuetext={value === null ? 'unknown' : `${Math.round(v)}%`}
      className="h-2 w-full overflow-hidden rounded-full bg-fg/10">
      <div className={cn('h-full rounded-full', SEV_BG[sev])} style={{ width: `${v}%` }} />
    </div>
  )
}

/* ═══ modal layers ═════════════════════════════════════════════════════
 * Open modal <dialog>s live in the top layer and make the page inert, so
 * toasts / live regions are portalled into the topmost open dialog. */

const layerStore = createStore<HTMLElement[]>([])
export function pushLayer(el: HTMLElement) { layerStore.set((l) => [...l.filter((x) => x !== el), el]) }
export function popLayer(el: HTMLElement) { layerStore.set((l) => l.filter((x) => x !== el)) }

export function TopLayer({ children }: { children: ReactNode }) {
  const layers = useStore(layerStore)
  const top = layers[layers.length - 1]
  return top ? createPortal(children, top) : <>{children}</>
}

/** showModal()/close() a native dialog in step with `open`, restoring focus. */
export function useModalDialog(open: boolean) {
  const ref = useRef<HTMLDialogElement>(null)
  useEffect(() => {
    const d = ref.current
    if (!open || !d) return
    const prev = document.activeElement as HTMLElement | null
    if (!d.open) d.showModal()
    pushLayer(d)
    // runs when `open` turns false and on unmount (ref.current is null by then)
    return () => {
      popLayer(d)
      if (d.open) d.close()
      prev?.focus?.()
    }
  }, [open])
  return ref
}

/* ═══ modal dialog (native <dialog>: focus containment, Esc, top layer) ═ */

export function Modal({ open, onClose, title, children, footer, wide }: {
  open: boolean; onClose: () => void; title: ReactNode; children: ReactNode; footer?: ReactNode; wide?: boolean
}) {
  const ref = useModalDialog(open)
  const titleId = useId()
  return (
    <dialog ref={ref} aria-labelledby={titleId}
      onCancel={(e) => { e.preventDefault(); onClose() }}
      onClick={(e) => { if (e.target === ref.current) onClose() }}
      className={cn('m-auto w-[calc(100vw-24px)] rounded-xl border border-fg/15 bg-card p-0 text-t1 shadow-2xl', wide ? 'max-w-[560px]' : 'max-w-[440px]')}>
      {open && (
        <div className="flex max-h-[85vh] flex-col">
          <div className="flex items-start justify-between gap-3 border-b border-fg/10 px-4 py-3">
            <h2 id={titleId} className="text-[15px] font-semibold">{title}</h2>
            <button className="btn btn-ghost btn-sm -mr-1 px-1.5" onClick={onClose} aria-label="Close dialog"><X className="h-4 w-4" aria-hidden /></button>
          </div>
          <div className="overflow-y-auto px-4 py-3 text-[13px] leading-relaxed">{children}</div>
          {footer && <div className="flex flex-wrap justify-end gap-2 border-t border-fg/10 px-4 py-3">{footer}</div>}
        </div>
      )}
    </dialog>
  )
}
