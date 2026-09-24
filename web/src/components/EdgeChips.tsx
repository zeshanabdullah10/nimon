import { memo } from 'react'
import { setEdgeFilter, useEdgeFilter, useEdges } from '@/hooks/nimon'
import { cn } from '@/lib/utils'

/** Edge filter chips, shared by every view (selection persists across views). */
export const EdgeChips = memo(function EdgeChips() {
  const edges = useEdges().data ?? []
  const sel = useEdgeFilter()
  if (edges.length < 2 && !sel) return null
  const chip = (id: string | null, label: string, live?: boolean) => {
    const on = sel === id
    return (
      <button key={id ?? '__all'} type="button" aria-pressed={on} onClick={() => setEdgeFilter(id)}
        className={cn('inline-flex min-h-[32px] items-center gap-1.5 rounded-full border px-3 text-xs font-medium transition-colors',
          on ? 'border-info/60 bg-info/15 text-t1' : 'border-fg/15 text-t2 hover:bg-fg/5 hover:text-t1')}>
        {live !== undefined && (
          <span aria-hidden className={cn('h-2 w-2 rounded-full', live ? 'bg-ok' : 'border border-t3')} />
        )}
        {label}
        {live === false && <span className="text-t3">(offline)</span>}
      </button>
    )
  }
  return (
    <div role="group" aria-label="Filter by edge" className="flex flex-wrap gap-1.5">
      {chip(null, 'All edges')}
      {edges.map((e) => chip(e.edge_id, e.name || e.edge_id, e.live))}
      {sel && !edges.some((e) => e.edge_id === sel) && chip(sel, `${sel} (unknown)`)}
    </div>
  )
})
