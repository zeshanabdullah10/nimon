/* NIMon widget UI.
 *
 * All data comes from the Rust side (`poll` command); the webview itself
 * has no network access (strict CSP). Rendering is keyed and patch-based:
 * nodes are created once and only changed fields are written, so an
 * unchanged poll touches (almost) nothing.
 */
'use strict';
(() => {
const T = window.__TAURI__;
const invoke = T.core.invoke;
const listen = T.event.listen;

/* poll cadence (ms): fast while the panel is open, coarse when collapsed */
const POLL_OPEN = 5000;
const POLL_IDLE = 30000;
const POLL_DOWN_IDLE = 10000;
const POLL_STARTING = 1000;
const MAX_CARDS = 6;
const MAX_ALERTS = 3;

/* ── diagnostics → Rust stderr (JS errors, CSP violations) ── */
function report(level, message) {
    try { invoke('ui_log', { level, message: String(message) }).catch(() => {}); } catch { /* ignore */ }
}
window.addEventListener('error', (e) => report('error', `${e.message} @${e.filename}:${e.lineno}`));
window.addEventListener('unhandledrejection', (e) =>
    report('error', 'unhandled rejection: ' + (e.reason?.message || e.reason)));
document.addEventListener('securitypolicyviolation', (e) =>
    report('csp', `${e.violatedDirective} blocked ${e.blockedURI || '(inline)'}`));

/* ── state ── */
const S = {
    up: false, starting: false, hubError: null, edgeState: '', hub: '',
    hasToken: false, health: null,
    edges: [], devices: [], alerts: [], predictions: [], settings: null,
    res: {},                 // per-resource {ok, error, age_ms}
    updatedAt: null,         // ms epoch of the last good device fetch
    everData: false,
    locked: false, dragging: false,
    selected: null,          // edge id or null (= all)
    showAll: false,
    expanded: false,
    openedBy: null,          // 'hover' | 'keyboard'
    opener: null,            // element that opened the panel (focus return)
    winH: 336,
    tileY: 60,
    alertOps: {},            // id -> {busy, error}
};

/* ── small helpers ── */
const $ = (id) => document.getElementById(id);
const num = (v) => (typeof v === 'number' && Number.isFinite(v)) ? v : null;
const plural = (n, w) => `${n} ${w}${n === 1 ? '' : 's'}`;

function parseTs(ts) {
    if (!ts) return null;
    let s = String(ts).trim().replace(' ', 'T');
    if (!/[zZ]$/.test(s) && !/[+-]\d{2}:?\d{2}$/.test(s)) s += 'Z';
    const t = Date.parse(s);
    return Number.isNaN(t) ? null : t;
}
function ago(ms) {
    const s = Math.max(0, Math.round(ms / 1000));
    if (s < 60) return s + 's ago';
    const m = Math.floor(s / 60); if (m < 60) return m + 'm ago';
    const h = Math.floor(m / 60); if (h < 24) return h + 'h ago';
    return Math.floor(h / 24) + 'd ago';
}
function relTime(ts) {
    const t = parseTs(ts);
    return t === null ? '' : ago(Date.now() - t);
}
/* "edge:PXIe-6368#2" -> {name: "PXIe-6368", tag: "2"}; tolerant of junk */
function deviceTitle(id) {
    const s = typeof id === 'string' ? id : '';
    const i = s.indexOf(':');
    const short = i === -1 ? s : s.slice(i + 1);
    const h = short.lastIndexOf('#');
    return {
        name: (h === -1 ? short : short.slice(0, h)).trim() || s || 'device',
        tag: h === -1 ? '' : short.slice(h + 1),
    };
}
const initials = (name) =>
    (String(name || '').split(/\s+/).filter(Boolean).map((w) => w[0]).join('') || '?')
        .slice(0, 2).toUpperCase();

/* DOM patch helpers: write only when the value actually changes */
function setText(el, t) { t = String(t ?? ''); if (el.textContent !== t) el.textContent = t; }
function setAttr(el, k, v) { v = String(v); if (el.getAttribute(k) !== v) el.setAttribute(k, v); }
function setClass(el, c) { if (el.getAttribute('class') !== c) el.setAttribute('class', c); }
function setHidden(el, h) { h = !!h; if (el.hidden !== h) el.hidden = h; }
function setStyle(el, k, v) { if (el.style[k] !== v) el.style[k] = v; }
function fromTemplate(html) {
    const t = document.createElement('template');
    t.innerHTML = html.trim();   /* static markup only — never data */
    return t.content.firstElementChild;
}
/* keyed list reconcile: reuse nodes by key, keep order, drop the rest */
function reconcile(container, items, keyOf, create, update) {
    const existing = new Map();
    for (const child of container.children) existing.set(child.dataset.key, child);
    let prev = null;
    for (const item of items) {
        const key = String(keyOf(item));
        let node = existing.get(key);
        if (node) existing.delete(key);
        else { node = create(item); node.dataset.key = key; }
        update(node, item);
        const want = prev ? prev.nextElementSibling : container.firstElementChild;
        if (node !== want) container.insertBefore(node, want);
        prev = node;
    }
    existing.forEach((n) => n.remove());
}

/* ═══ model (pure; shared rules with the web dashboard) ═══════════════ */

function thresholdsFor(settings, edgeId) {
    if (!settings) return null;
    const e = settings.edge_thresholds?.[edgeId] || null;
    const g = settings.thresholds || null;
    const warning = num(e?.temperature_warning) ?? num(g?.temperature_warning);
    const critical = num(e?.temperature_critical) ?? num(g?.temperature_critical);
    return warning === null && critical === null ? null : { warning, critical };
}
function tempSeverity(t, thr) {
    if (t === null || !thr) return null;
    if (thr.critical !== null && t >= thr.critical) return 'critical';
    if (thr.warning !== null && t >= thr.warning) return 'warning';
    return 'ok';
}
const STATUS_SEV = { error: 'critical', critical: 'critical', warning: 'warning', offline: 'offline', healthy: 'ok' };
const SEV_RANK = { ok: 0, warning: 1, critical: 2 };
function worse(a, b) {
    if (a == null) return b ?? null;
    if (b == null) return a;
    return (SEV_RANK[b] ?? 0) > (SEV_RANK[a] ?? 0) ? b : a;
}
const SEV_LABEL = {
    ok: 'OK', warning: 'WARN', critical: 'CRIT', offline: 'OFFLINE',
    nolink: 'NO LINK', unknown: 'NO DATA',
};
/* severity -> css suffix */
const SEV_CLS = { ok: 'ok', warning: 'warn', critical: 'crit', offline: 'idle', nolink: 'idle', unknown: 'idle' };

function deviceEdge(d) {
    if (d.edge_id) return String(d.edge_id);
    const id = String(d.device_id ?? '');
    const i = id.indexOf(':');
    return i === -1 ? '' : id.slice(0, i);
}

function deviceView(d, settings, now) {
    const edgeId = deviceEdge(d);
    const thr = thresholdsFor(settings, edgeId);
    const temp = num(d.metrics?.temperature);
    const status = String(d.status || '').toLowerCase();
    const statusSev = STATUS_SEV[status] ?? null;
    const tSev = tempSeverity(temp, thr);
    let sev;
    if (d.is_reachable === false) sev = 'nolink';
    else if (statusSev === 'offline') sev = 'offline';
    else sev = worse(statusSev, tSev) ?? 'unknown';
    const offAfter = num(settings?.edge_offline_after_secs);
    const seen = parseTs(d.last_seen);
    const stale = d.live === false
        || (offAfter !== null && offAfter > 0 && (seen === null || now - seen > offAfter * 1000));
    const title = deviceTitle(d.device_id);
    const slot = num(d.slot) ?? num(d.metrics?.slot);
    const model = typeof d.model === 'string' ? d.model
        : typeof d.metrics?.product === 'string' ? d.metrics.product : '';
    return {
        id: String(d.device_id ?? ''),
        edgeId,
        name: (typeof d.name === 'string' && d.name) || title.name,
        tag: title.tag,
        slot, model, temp, thr, sev, tSev, statusSev, status, stale,
        sim: !!d.is_simulated,
        live: d.live !== false,
        reachable: d.is_reachable !== false,
        lastSeen: d.last_seen,
        raw: d,
    };
}

/* worst live-device severity -> headline-level ('ok' | 'warning' | 'critical') */
function levelOf(v) {
    if (v.sev === 'critical') return 'critical';
    if (v.sev === 'warning' || v.sev === 'offline' || v.sev === 'nolink') return 'warning';
    return 'ok';
}
const alertActive = (a) => ['firing', 'pending'].includes(String(a.status || '').toLowerCase());

function headline(ctx, views, alerts) {
    if (!ctx.up) return ctx.starting ? { code: 'STARTING', cls: 'idle' } : { code: 'HUB DOWN', cls: 'crit' };
    if (ctx.devicesStale) return { code: 'STALE', cls: 'stale' };
    const live = views.filter((v) => !v.stale);
    if (views.length && !live.length) return { code: 'STALE', cls: 'stale' };
    let level = 'ok';
    for (const v of live) level = worse(level, levelOf(v));
    for (const a of alerts) {
        const status = String(a.status || '').toLowerCase();
        const s = String(a.severity || '').toLowerCase();
        if (alertActive(a)) {
            if (s === 'critical' || s === 'warning') level = worse(level, s);
        } else if (status === 'acknowledged' && (s === 'critical' || s === 'warning')) {
            // Acknowledged: still unresolved, but capped at WARN (matches the web dashboard)
            level = worse(level, 'warning');
        }
    }
    return level === 'critical' ? { code: 'CRIT', cls: 'crit' }
        : level === 'warning' ? { code: 'WARN', cls: 'warn' }
        : { code: 'OK', cls: 'ok' };
}

function sortViews(views) {
    const rank = (v) => v.stale ? -1 : ({ critical: 5, warning: 4, nolink: 3, offline: 3, ok: 1 }[v.sev] ?? 0);
    return [...views].sort((a, b) =>
        (rank(b) - rank(a)) || ((b.temp ?? -1e9) - (a.temp ?? -1e9)) || a.id.localeCompare(b.id));
}

function edgeList(S) {
    const seen = new Set();
    const out = [];
    for (const e of S.edges) {
        if (!e || !e.edge_id || seen.has(e.edge_id)) continue;
        seen.add(e.edge_id);
        out.push({ id: String(e.edge_id), name: e.name || e.edge_id, live: e.live !== false && e.status !== 'offline' });
    }
    /* devices of edges missing from the (possibly failed) edge list */
    for (const d of S.devices) {
        const id = deviceEdge(d);
        if (id && !seen.has(id)) { seen.add(id); out.push({ id, name: id, live: true }); }
    }
    return out;
}

function ctxOf(S) {
    return { up: S.up, starting: S.starting, devicesStale: S.up && S.everData && S.res.devices?.ok === false };
}

/* strip view model (tiles + hub tile) */
function modelStrip(S, now) {
    const views = S.devices.map((d) => deviceView(d, S.settings, now));
    const head = headline(ctxOf(S), views, S.alerts);
    const tiles = edgeList(S).map((e) => {
        const vs = views.filter((v) => v.edgeId === e.id);
        const live = vs.filter((v) => !v.stale);
        const pool = live.length ? live : vs;
        const temps = pool.map((v) => v.temp).filter((t) => t !== null);
        const hot = temps.length ? Math.max(...temps) : null;
        const stale = !e.live || (vs.length > 0 && !live.length) || !S.up;
        let level = 'ok';
        for (const v of live) level = worse(level, levelOf(v));
        const cls = stale ? 'stale' : !vs.length ? 'idle' : SEV_CLS[level];
        const thr = thresholdsFor(S.settings, e.id);
        const pct = hot !== null && thr?.critical ? Math.max(0, Math.min(100, (hot / thr.critical) * 100)) : null;
        const word = stale ? 'STALE' : !vs.length ? 'NO MODULES' : SEV_LABEL[level];
        const flag = stale ? '?' : level === 'critical' ? '!!' : level === 'warning' ? '!' : '';
        const sim = vs.some((v) => v.sim);
        const tempTxt = hot === null ? '—' : Math.round(hot) + '°C';
        return {
            id: e.id, initials: initials(e.name), cls, flag, sim, tempTxt,
            offset: pct === null ? 100 : Math.round(100 - pct),
            label: `${e.name}: ${word}${hot === null ? '' : `, hottest ${hot.toFixed(1)} °C${stale ? ' (last known)' : ''}`}, `
                + `${plural(vs.length, 'module')}${sim ? ', simulated' : ''}. Enter opens details.`,
        };
    });
    const hubFlag = { 'HUB DOWN': '×', STARTING: '…', CRIT: '!!', WARN: '!', STALE: '?' }[head.code] || '';
    return {
        head, tiles,
        hub: {
            cls: head.cls, flag: hubFlag,
            label: `NIMon hub: ${head.code}. All edges, ${plural(S.devices.length, 'module')}. Enter opens details.`,
        },
    };
}

function fmtC(t) { return t === null ? '—' : t.toFixed(1); }
function gb(mb) { const v = num(mb); return v === null ? '—' : (v / 1024).toFixed(1) + ' GB'; }

function cardView(v) {
    const crit = v.thr?.critical ?? null;
    let cls = v.stale ? 'stale' : SEV_CLS[v.sev] || 'idle';
    const noTemp = v.temp === null || (!v.stale && !v.reachable);
    const parts = [];
    if (v.slot !== null) parts.push('Slot ' + v.slot);
    if (v.model && v.model !== v.name) parts.push(v.model);
    if (v.stale) parts.push(`last known · ${relTime(v.lastSeen) || 'never'}`);
    else if (v.sev === 'nolink') parts.push('no link');
    else if (v.sev === 'offline') parts.push('offline');
    else parts.push(`link ok · ${relTime(v.lastSeen)}`);
    let sub;
    if (v.stale) sub = 'STALE';
    else if (v.sev === 'nolink' || v.sev === 'offline') sub = SEV_LABEL[v.sev];
    else if (noTemp) sub = v.statusSev && v.statusSev !== 'ok' ? `${SEV_LABEL[v.sev]} · ${v.status}` : 'no data';
    else if (v.statusSev && SEV_RANK[v.statusSev] > (SEV_RANK[v.tSev] ?? -1)) sub = `${SEV_LABEL[v.sev]} · ${v.status}`;
    else if (crit === null) sub = v.thr ? SEV_LABEL[v.sev] : 'no thresholds';
    else if (v.sev === 'critical') sub = `CRIT · ≥ ${crit}°`;
    else if (v.sev === 'warning') sub = `WARN · ${Math.max(0, crit - v.temp).toFixed(0)}° to crit`;
    else sub = `OK · ${Math.max(0, crit - v.temp).toFixed(0)}° to crit`;
    if (noTemp && !v.stale) cls = 'idle';
    const fill = !v.stale && !noTemp && crit ? Math.max(0, Math.min(100, (v.temp / crit) * 100)) : 0;
    return {
        key: v.id,
        l1: v.name + (v.tag ? ' · #' + v.tag : ''),
        l2: parts.join(' · '),
        metric: noTemp && !v.stale ? '—' : fmtC(v.temp),
        unit: (noTemp && !v.stale) || v.temp === null ? '' : '°C',
        cls, sub, fill, fillCls: SEV_CLS[v.sev] === 'idle' ? 'ok' : SEV_CLS[v.sev],
        stale: v.stale, sim: v.sim,
    };
}

function alertView(a, views, S) {
    const v = views.find((x) => x.id === a.device_id);
    const sev = String(a.severity || 'info').toLowerCase();
    const status = String(a.status || '').toLowerCase();
    const isTemp = /temp/i.test(a.metric_name || '');
    const val = num(a.metric_value), thr = num(a.threshold);
    const unit = isTemp ? '°C' : '';
    let value = '';
    if (val !== null) value = `${val.toFixed(1)}${unit}` + (thr !== null ? ` vs ${thr.toFixed(1)}${unit}` : '');
    const where = v ? v.name + (v.slot !== null ? ` · Slot ${v.slot}` : '')
        : a.device_id ? deviceTitle(a.device_id).name : (a.edge_id || '');
    const op = S.alertOps[a.id] || {};
    return {
        key: a.id,
        sev, sevLabel: sev === 'critical' ? 'CRIT' : sev === 'warning' ? 'WARN' : 'INFO',
        title: a.title || a.rule_id || 'alert',
        time: relTime(a.triggered_at),
        info: [where, value, status === 'acknowledged' ? 'acknowledged' : ''].filter(Boolean).join(' · '),
        acked: status === 'acknowledged',
        busy: !!op.busy, error: op.error || '',
    };
}

function modelPanel(S, now) {
    const allViews = S.devices.map((d) => deviceView(d, S.settings, now));
    const id = S.selected;
    const edge = id ? edgeList(S).find((e) => e.id === id) : null;
    const views = id ? allViews.filter((v) => v.edgeId === id) : allViews;
    const alerts = id ? S.alerts.filter((a) => a.edge_id === id) : S.alerts;
    const cut = num(S.settings?.prediction_alert_threshold);
    const preds = cut === null ? null
        : (id ? S.predictions.filter((p) => p.edge_id === id) : S.predictions)
            .filter((p) => (num(p.probability) ?? 0) >= cut);
    const head = headline(ctxOf(S), views, alerts);
    const sorted = sortViews(views);
    const shown = S.showAll ? sorted : sorted.slice(0, MAX_CARDS);
    const live = views.filter((v) => !v.stale);
    const temps = live.map((v) => v.temp).filter((t) => t !== null);
    const online = views.filter((v) => v.live && v.reachable && v.sev !== 'offline').length;
    const crit = alerts.filter((a) => String(a.severity).toLowerCase() === 'critical').length;
    const sys = views.find((v) => num(v.raw.metrics?.mem_total_mb) !== null)?.raw.metrics;
    const pct = (n, d) => d ? Math.round((n / d) * 100) : null;
    const meterCls = (n, d) => !d ? 'idle' : n === d ? 'ok' : n === 0 ? 'crit' : 'warn';

    return {
        head,
        scope: edge ? edge.name : 'All edges',
        modA: `${edge ? edge.name : 'All edges'} · ${plural(views.length, 'device')}`,
        modB: `${online} active`,
        cards: shown.map(cardView),
        overflow: sorted.length - MAX_CARDS,
        alerts: alerts.slice(0, MAX_ALERTS).map((a) => alertView(a, allViews, S)),
        alertsMore: Math.max(0, alerts.length - MAX_ALERTS),
        alertHead: S.res.alerts?.ok === false && S.everData ? 'last known · refresh failed' : `${alerts.length} active`,
        peak: temps.length ? Math.max(...temps).toFixed(1) + '°C' : '—',
        stAlerts: plural(alerts.length, 'alert'),
        stPreds: preds === null ? 'predictions —' : `${plural(preds.length, 'prediction')} ≥ ${Math.round(cut * 100)}%`,
        stMods: plural(views.length, 'module'),
        station: sys ? {
            memFree: gb(sys.mem_free_mb),
            mem: `${gb(sys.mem_free_mb)} / ${gb(sys.mem_total_mb)} RAM free`,
            disk: `${gb(sys.disk_free_mb)} / ${gb(sys.disk_total_mb)} disk free`,
            name: typeof sys.product === 'string' ? sys.product : '',
        } : null,
        online: { n: pct(online, views.length), s: `${online} of ${views.length}`, cls: meterCls(online, views.length) },
        fresh: { n: pct(live.length, views.length), s: `${live.length} reporting`, cls: meterCls(live.length, views.length) },
        alertMeter: { n: alerts.length, s: `${crit} critical`, cls: alerts.length === 0 ? 'ok' : crit ? 'crit' : 'warn' },
    };
}

function noticeOf(S) {
    if (S.hubError) return { cls: 'n-crit', text: `Hub failed to start: ${S.hubError}` };
    if (S.starting) return { cls: 'n-info', text: `Starting hub… (edge: ${S.edgeState || 'n/a'})` };
    if (!S.up) {
        return {
            cls: 'n-crit',
            text: `Hub unreachable at ${S.hub}. Retrying every ${Math.round(nextDelay() / 1000)} s…`
                + (S.everData ? ' Showing last known data.' : ''),
        };
    }
    const failed = ['devices', 'edges', 'alerts', 'predictions', 'settings']
        .filter((k) => S.res[k] && S.res[k].ok === false);
    if (failed.length) {
        return {
            cls: 'n-warn',
            text: `Could not refresh ${failed.join(', ')} (${S.res[failed[0]].error || 'error'}). Showing last known data.`,
        };
    }
    if (S.health && S.health.status === 'degraded') {
        const bad = [S.health.db_ok === false ? 'database' : '', S.health.alert_manager_ok === false ? 'alert manager' : '']
            .filter(Boolean).join(', ');
        return { cls: 'n-warn', text: `Hub degraded${bad ? ` (${bad})` : ''}.` };
    }
    return null;
}

/* ═══ data ═══════════════════════════════════════════════════════════ */

function applyPoll(data) {
    const now = Date.now();
    if (!data || typeof data !== 'object') data = { up: false, res: {} };
    S.up = !!data.up;
    S.starting = !!data.starting && !S.up;
    S.hubError = data.hub_error || null;
    S.edgeState = data.edge_state || '';
    S.hub = data.hub || S.hub;
    S.hasToken = !!data.has_token;
    const res = data.res || {};
    const take = (k) => {
        const r = res[k];
        S.res[k] = r ? { ok: !!r.ok, error: r.error || null, age: r.age_ms } : { ok: false, error: 'no data' };
        return r && r.age_ms !== null && r.age_ms !== undefined ? r.data : undefined;
    };
    const arr = (v, fallback) => Array.isArray(v) ? v : fallback;
    S.edges = arr(take('edges'), S.edges);
    const devs = take('devices');
    if (Array.isArray(devs)) {
        S.devices = devs;
        S.everData = true;
        S.updatedAt = now - (num(res.devices.age_ms) ?? 0);
    }
    S.alerts = arr(take('alerts'), S.alerts);
    S.predictions = arr(take('predictions'), S.predictions);
    const settings = take('settings');
    if (settings && typeof settings === 'object') S.settings = settings;
    const health = take('health');
    S.health = S.up && health && typeof health === 'object' ? health : null;
    /* forget finished alert operations for alerts that are gone */
    const ids = new Set(S.alerts.map((a) => a.id));
    for (const k of Object.keys(S.alertOps)) if (!ids.has(k) && !S.alertOps[k].busy) delete S.alertOps[k];
}

/* ═══ rendering: strip ═══════════════════════════════════════════════ */

const TILE_HTML = `<button type="button" class="tile">
    <span class="box">
        <svg class="ring" viewBox="0 0 40 40" aria-hidden="true">
            <rect class="ring-bg" x="3" y="3" width="34" height="34" rx="11"/>
            <rect class="ring-prog" x="3" y="3" width="34" height="34" rx="11" pathLength="100" stroke-dasharray="100" stroke-dashoffset="100" transform="rotate(-90 20 20)"/>
        </svg>
        <span class="glyph txt" aria-hidden="true"></span>
        <span class="flag" aria-hidden="true"></span>
    </span>
    <span class="pct" aria-hidden="true"></span>
    <span class="simtag" aria-hidden="true" hidden>SIM</span>
</button>`;

function patchTile(node, t, isHub) {
    setClass(node, `tile${isHub ? ' hub' : ''} s-${t.cls}`);
    setAttr(node, 'aria-label', t.label);
    setAttr(node, 'title', t.label.replace(/ Enter opens details\.$/, ''));
    setText(node.querySelector('.flag'), t.flag);
    if (!isHub) {
        node.dataset.edge = t.id;
        setText(node.querySelector('.glyph'), t.initials);
        setText(node.querySelector('.pct'), t.tempTxt);
        setAttr(node.querySelector('.ring-prog'), 'stroke-dashoffset', t.offset);
        setHidden(node.querySelector('.simtag'), !t.sim);
    }
    setAttr(node, 'aria-expanded', String(S.expanded && S.selected === (isHub ? null : t.id)));
}

let lastStripKey = '';
function renderStrip(force) {
    const m = modelStrip(S, Date.now());
    const key = JSON.stringify([m, S.locked, S.expanded, S.selected]);
    if (!force && key === lastStripKey) return;
    lastStripKey = key;

    const bar = $('bar');
    bar.classList.toggle('locked', S.locked);
    const lock = $('lockBtn');
    setAttr(lock, 'aria-pressed', String(S.locked));
    setAttr(lock, 'aria-label', S.locked ? 'Unlock position' : 'Lock position');
    setAttr(lock, 'title', S.locked ? 'Unlock position' : 'Lock position');

    patchTile($('hubTile'), m.hub, true);
    reconcile($('tiles'), m.tiles, (t) => t.id, () => fromTemplate(TILE_HTML), (n, t) => patchTile(n, t, false));
}

/* ═══ rendering: panel ═══════════════════════════════════════════════ */

const CARD_HTML = `<div class="card" role="listitem">
    <div class="fill"></div>
    <div class="content">
        <div class="left">
            <div class="l1"><span class="nm"></span><span class="tag" hidden>SIM</span></div>
            <div class="l2"></div>
        </div>
        <div class="right">
            <span class="metric"><span class="v"></span><span class="unit"></span></span>
            <span class="sub"></span>
        </div>
    </div>
</div>`;

function patchCard(n, c) {
    setClass(n, 'card' + (c.stale ? ' stale' : ''));
    setText(n.querySelector('.nm'), c.l1);
    setHidden(n.querySelector('.tag'), !c.sim);
    setAttr(n.querySelector('.tag'), 'title', 'Simulated device (not real hardware)');
    setText(n.querySelector('.l2'), c.l2);
    setClass(n.querySelector('.metric'), `metric c-${c.cls}`);
    setText(n.querySelector('.v'), c.metric);
    setText(n.querySelector('.unit'), c.unit);
    setText(n.querySelector('.sub'), c.sub);
    const fill = n.querySelector('.fill');
    setClass(fill, `fill f-${c.fillCls}`);
    setStyle(fill, 'width', c.fill.toFixed(1) + '%');
}

const ALERT_HTML = `<div class="alert" role="listitem">
    <div class="r1"><span class="sev"></span><span class="ttl"></span><span class="tm"></span></div>
    <div class="r2">
        <span class="info"></span>
        <button type="button" class="act" data-op="acknowledge">Ack</button>
        <button type="button" class="act" data-op="resolve">Resolve</button>
    </div>
    <span class="err" role="alert"></span>
</div>`;

function patchAlert(n, a) {
    const sev = n.querySelector('.sev');
    setClass(sev, `sev v-${a.sev}`);
    setText(sev, a.sevLabel);
    setText(n.querySelector('.ttl'), a.title);
    setAttr(n.querySelector('.ttl'), 'title', a.title);
    setText(n.querySelector('.tm'), a.time);
    setText(n.querySelector('.info'), a.info);
    setAttr(n.querySelector('.info'), 'title', a.info);
    const ack = n.querySelector('[data-op="acknowledge"]');
    const res = n.querySelector('[data-op="resolve"]');
    setHidden(ack, a.acked);
    setAttr(ack, 'aria-label', `Acknowledge alert: ${a.title}`);
    setAttr(res, 'aria-label', `Resolve alert: ${a.title}`);
    if (ack.disabled !== a.busy) ack.disabled = a.busy;
    if (res.disabled !== a.busy) res.disabled = a.busy;
    setText(n.querySelector('.err'), a.error);
}

function renderPanel() {
    if (!S.expanded) return;   /* collapsed: panel is invisible, skip all work */
    const m = modelPanel(S, Date.now());
    document.body.classList.toggle('stale-data', !S.up || m.head.code === 'STALE');

    const hl = $('headline');
    setClass(hl, `badge b-${m.head.cls}`);
    setText(hl, m.head.code);
    setAttr(hl, 'aria-label', `Status ${m.head.code}`);
    setText($('scopeLine'), m.scope);
    renderUpdated();

    const note = noticeOf(S);
    const ne = $('notice');
    setHidden(ne, !note);
    if (note) { setClass(ne, `notice ${note.cls}`); setText(ne, note.text); }

    setText($('modA'), m.modA);
    setText($('modB'), m.modB);
    reconcile($('cardList'), m.cards, (c) => c.key, () => fromTemplate(CARD_HTML), patchCard);
    setHidden($('cardsEmpty'), m.cards.length > 0);
    const more = $('moreBtn');
    setHidden(more, m.overflow <= 0);
    setText(more, S.showAll ? 'Show fewer' : `+${m.overflow} more`);
    setAttr(more, 'aria-expanded', String(S.showAll));

    setText($('alertHead'), m.alertHead);
    reconcile($('alertList'), m.alerts, (a) => a.key, () => fromTemplate(ALERT_HTML), patchAlert);
    setHidden($('alertsEmpty'), m.alerts.length > 0);
    const am = $('alertsMore');
    setHidden(am, m.alertsMore <= 0);
    setText(am, `${m.alertsMore} more — open dashboard`);

    setText($('peakNum'), m.peak);
    setText($('stAlerts'), m.stAlerts);
    setText($('stPreds'), m.stPreds);
    setText($('stMods'), m.stMods);

    setHidden($('stationSec'), !m.station);
    if (m.station) {
        setText($('memFree'), m.station.memFree);
        setText($('memLine'), m.station.mem);
        setText($('diskLine'), m.station.disk);
        setText($('stationName'), m.station.name);
    }

    const meter = (id, v, suffix) => {
        const n = $(id + 'N');
        setClass(n, `n c-${v.cls}`);
        setText(n, v.n === null ? '—' : v.n + suffix);
        setText($(id + 'S'), v.s);
    };
    meter('mOnline', m.online, '%');
    meter('mFresh', m.fresh, '%');
    meter('mAlerts', m.alertMeter, '');
}

function renderUpdated() {
    const el = $('updated');
    setText(el, S.updatedAt === null ? 'no data yet' : 'updated ' + ago(Date.now() - S.updatedAt));
}

/* ═══ geometry (window size/position live in Rust) ═══════════════════ */

const geo = { want: null, applied: '', busy: false, failed: null };

function requestGeo(mode, h) {
    geo.want = { mode, h: Math.round(h) };
    pumpGeo();
}
async function pumpGeo() {
    if (geo.busy || !geo.want) return;
    const { mode, h } = geo.want;
    geo.want = null;
    const key = mode + ':' + h;
    if (key === geo.applied) { positionPanel(); return; }
    geo.busy = true;
    try {
        const finalH = await invoke('update_geometry', { mode, height: h });
        geo.applied = key;          /* only after success: failures retry */
        geo.failed = null;
        applyWinH(finalH);
    } catch (e) {
        geo.applied = '';
        geo.failed = { mode, h };
        report('warn', 'update_geometry failed: ' + e);
    } finally {
        geo.busy = false;
    }
    positionPanel();
    if (geo.want) pumpGeo();
}
function applyWinH(finalH) {
    if (typeof finalH !== 'number' || !Number.isFinite(finalH)) return;
    S.winH = finalH;
    setStyle($('panel'), 'maxHeight', (finalH - 16) + 'px');
}

/* natural (unclamped) panel height without toggling styles: the card
   list is measured inside its scroller */
function measureGeo() {
    const bar = $('bar');
    const barH = bar.offsetHeight + 16;
    if (!S.expanded) { requestGeo('strip', barH); return; }
    const panel = $('panel');
    const scroller = $('cardsScroll');
    const natural = panel.offsetHeight - scroller.clientHeight + $('cardList').offsetHeight;
    requestGeo('panel', Math.max(natural + 16, barH));
}
let geoFrame = 0;
function scheduleGeo() {
    if (geoFrame) return;
    geoFrame = requestAnimationFrame(() => { geoFrame = 0; measureGeo(); });
}

/* center the panel on the bar / hovered tile; clamp to the window */
function positionPanel() {
    if (!S.expanded) return;
    const panel = $('panel');
    const bar = $('bar');
    const ph = panel.offsetHeight;
    const barCenter = bar.offsetTop + bar.offsetHeight / 2;
    const top = Math.max(8, Math.min(S.winH - ph - 8, barCenter - ph / 2));
    setStyle(panel, 'top', top + 'px');
    panel.style.setProperty('--notch-y', Math.max(14, Math.min(ph - 14, S.tileY - top - 5)) + 'px');
}

/* ═══ expand / collapse ══════════════════════════════════════════════ */

let tickTimer = 0;
function setExpanded(on, how) {
    if (on && how) S.openedBy = how;
    if (S.expanded === on) return;
    S.expanded = on;
    document.body.classList.toggle('expanded', on);
    $('panel').inert = !on;
    clearInterval(tickTimer);
    if (on) {
        renderPanel();
        tickTimer = setInterval(renderUpdated, 1000);   /* text only, while open */
        /* catch up quickly when opening after a long idle interval */
        if (Date.now() - lastPollAt > POLL_OPEN) pollSoon(0);
        else pollSoon(POLL_OPEN - (Date.now() - lastPollAt));
    } else {
        S.openedBy = null;
        const f = document.activeElement;
        if (f && $('panel').contains(f) && S.opener) S.opener.focus({ preventScroll: true });
        document.body.classList.remove('stale-data');
        pollSoon(nextDelay());
    }
    renderStrip(true);
    scheduleGeo();
}

function select(id, how, node) {
    if (S.selected !== id) S.showAll = false;
    S.selected = id;
    S.opener = node || null;
    if (node) {
        const r = node.getBoundingClientRect();   /* user-triggered only */
        S.tileY = r.top + r.height / 2;
    }
    const wasOpen = S.expanded;
    setExpanded(true, how);
    if (wasOpen) { renderPanel(); renderStrip(true); positionPanel(); }
}

/* ═══ polling: next poll is scheduled after the previous one finished ═ */

let pollTimer = 0;
let polling = false;
let lastPollAt = 0;

function nextDelay() {
    if (S.starting) return POLL_STARTING;
    if (!S.up) return S.expanded ? POLL_OPEN : POLL_DOWN_IDLE;
    return S.expanded ? POLL_OPEN : POLL_IDLE;
}
function pollSoon(ms) {
    clearTimeout(pollTimer);
    pollTimer = setTimeout(pollNow, Math.max(0, ms));
}
async function pollNow() {
    clearTimeout(pollTimer);
    if (polling) return;           /* in flight: it reschedules itself */
    polling = true;
    let data = null;
    try { data = await invoke('poll'); } catch (e) { report('warn', 'poll failed: ' + e); }
    lastPollAt = Date.now();
    try {
        applyPoll(data);
        renderStrip(false);
        renderPanel();
        if (geo.failed && !geo.want) requestGeo(geo.failed.mode, geo.failed.h);
    } catch (e) {
        report('error', 'render failed: ' + (e && e.stack || e));
    } finally {
        polling = false;
        pollSoon(nextDelay());
    }
}

/* ═══ alert actions (done in Rust; token handled there) ══════════════ */

async function alertOp(id, op) {
    S.alertOps[id] = { busy: true, error: '' };
    renderPanel();
    try {
        await invoke('alert_action', { id, action: op });
        S.alertOps[id] = { busy: false, error: '' };
        pollSoon(0);
    } catch (e) {
        S.alertOps[id] = { busy: false, error: `${op === 'resolve' ? 'Resolve' : 'Acknowledge'} failed: ${e}` };
    }
    renderPanel();
}

/* ═══ wiring ═════════════════════════════════════════════════════════ */

function initUi() {
    const bar = $('bar');

    /* drag the bar (not from tiles/buttons) */
    bar.addEventListener('pointerdown', (e) => {
        if (S.locked || e.button !== 0) return;
        if (e.target.closest('button')) return;
        S.dragging = true;
        bar.classList.add('dragging');
        bar.setPointerCapture(e.pointerId);
    });
    bar.addEventListener('pointermove', (e) => {
        if (!S.dragging) return;
        invoke('move_by', { dx: e.movementX, dy: e.movementY }).catch(() => {});
    });
    const end = (e) => {
        if (!S.dragging) return;
        S.dragging = false;
        bar.classList.remove('dragging');
        try { bar.releasePointerCapture(e.pointerId); } catch { /* ignore */ }
        invoke('save_state').catch(() => {});
    };
    bar.addEventListener('pointerup', end);
    bar.addEventListener('pointercancel', end);

    /* tiles: hover opens (as before); Enter/Space/click open + keep open */
    const tileOf = (e) => e.target.closest('.tile');
    bar.addEventListener('pointerover', (e) => {
        const t = tileOf(e);
        if (!t || S.dragging || e.pointerType === 'touch') return;
        if (e.relatedTarget && t.contains(e.relatedTarget)) return;
        const id = t.dataset.edge === '__hub' ? null : t.dataset.edge;
        if (S.expanded && S.selected === id) return;
        select(id, S.openedBy === 'keyboard' ? 'keyboard' : 'hover', t);
    });
    bar.addEventListener('click', (e) => {
        const t = tileOf(e);
        if (!t) return;
        const id = t.dataset.edge === '__hub' ? null : t.dataset.edge;
        const keyboard = e.detail === 0;
        if (keyboard && S.expanded && S.selected === id && S.openedBy === 'keyboard') {
            setExpanded(false);
            return;
        }
        select(id, keyboard ? 'keyboard' : (S.openedBy || 'hover'), t);
    });

    $('lockBtn').addEventListener('click', (e) => {
        e.stopPropagation();
        invoke('set_locked', { locked: !S.locked }).catch((err) => report('warn', 'set_locked: ' + err));
    });

    $('moreBtn').addEventListener('click', () => { S.showAll = !S.showAll; renderPanel(); });
    $('alertsMore').addEventListener('click', () => invoke('open_dashboard').catch(() => {}));
    $('dashBtn').addEventListener('click', () => invoke('open_dashboard').catch(() => {}));
    $('closeBtn').addEventListener('click', () => setExpanded(false));
    $('alertList').addEventListener('click', (e) => {
        const b = e.target.closest('button[data-op]');
        const row = b && b.closest('.alert');
        if (row && !b.disabled) alertOp(row.dataset.key, b.dataset.op);
    });

    /* mouse leaves the window: collapse unless opened from the keyboard */
    const onLeave = () => {
        if (!S.dragging && S.openedBy !== 'keyboard') setExpanded(false);
    };
    document.addEventListener('mouseleave', onLeave);
    document.documentElement.addEventListener('mouseleave', onLeave);
    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && S.expanded) { e.preventDefault(); setExpanded(false); }
    });

    /* geometry follows content size changes (no per-tick layout reads) */
    const ro = new ResizeObserver(scheduleGeo);
    ro.observe(bar);
    ro.observe($('panel'));
    ro.observe($('cardList'));
    for (const child of $('panel').children) ro.observe(child);
}

async function init() {
    initUi();
    try {
        const state = await invoke('get_state');
        S.locked = !!state.locked;
    } catch { /* ignore */ }
    listen('lock-changed', (e) => { S.locked = !!e.payload; renderStrip(true); });
    listen('geometry-changed', (e) => { applyWinH(e.payload); positionPanel(); scheduleGeo(); });
    renderStrip(true);
    await pollNow();
}

/* test hook (harness only) */
if (window.__NIMON_TEST__) {
    window.__NIMON_TEST__.api = {
        S, deviceTitle, thresholdsFor, tempSeverity, deviceView, headline, modelStrip,
        modelPanel, cardView, noticeOf, applyPoll, renderStrip, renderPanel, setExpanded, pollNow,
    };
}

init().catch((e) => report('error', 'init failed: ' + (e && e.stack || e)));
})();
