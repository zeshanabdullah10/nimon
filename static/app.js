'use strict';

/* ═══════════════════════════════════════════════════════════════════════
   NIMON · Material 3 dashboard logic
   Same data layer as before; renders into M3 components. Ripples on all
   interactive hosts. Entrance animation only for newly-seen elements.
   ═══════════════════════════════════════════════════════════════════════ */

const POLL_MS = 5000;
const STALE_MS = 45000;

/* Material Symbols (subset loaded in index.html via icon_names) */
const ICONS = {
    check: '<span class="ms ms-18">check</span>',
    ackCheck: '<span class="ms ms-16">check</span>',
    alert: '<span class="ms ms-fill ms-22">warning</span>',
    alertCritical: '<span class="ms ms-fill ms-22">error</span>',
    bolt: '<span class="ms ms-fill ms-22">bolt</span>',
    statWarning: '<span class="ms ms-fill ms-22">warning</span>',
    statError: '<span class="ms ms-fill ms-22">error</span>',
    devicesEmpty: '<span class="ms ms-fill ms-26">developer_board</span>',
    alertsEmpty: '<span class="ms ms-fill ms-26">notifications</span>',
    predsEmpty: '<span class="ms ms-fill ms-26">bolt</span>',
    thermostat: '<span class="ms ms-14">device_thermostat</span>',
};

const STATE_LABEL = {
    healthy: 'Healthy',
    warning: 'Warning',
    error: 'Error',
    offline: 'Offline',
};

class Dashboard {
    constructor() {
        this.edges = [];
        this.devicesByEdge = {};
        this.alerts = [];
        this.predictions = [];
        this.selectedEdge = 'all';
        this.ackInFlight = new Set();
        this.connected = null;
        this.busy = false;

        /* entrance choreography bookkeeping */
        this.seenDevices = new Set();
        this.seenEntries = new Set();

        this.init();
    }

    async init() {
        document.getElementById('chipRow')
            .addEventListener('click', (e) => this.onChip(e));
        document.getElementById('alertsList')
            .addEventListener('click', (e) => this.onAck(e));
        this.attachRipple(document.body);

        await this.refresh();
        setInterval(() => this.refresh(), POLL_MS);
    }

    /* M3 ripple — delegated pointerdown */
    attachRipple(root) {
        root.addEventListener('pointerdown', (e) => {
            const host = e.target.closest('.ripple-host');
            if (!host) return;
            const rect = host.getBoundingClientRect();
            const size = Math.max(rect.width, rect.height) * 1.4;
            const r = document.createElement('span');
            r.className = 'ripple';
            r.style.width = r.style.height = `${size}px`;
            r.style.left = `${e.clientX - rect.left - size / 2}px`;
            r.style.top = `${e.clientY - rect.top - size / 2}px`;
            host.appendChild(r);
            r.addEventListener('animationend', () => r.remove());
        });
    }

    /* ── data layer ─────────────────────────────────────────────── */

    async refresh() {
        if (this.busy) return;
        this.busy = true;
        try {
            const [health] = await Promise.all([
                this.getJSON('/health'),
                this.loadEdges(),
                this.loadAlerts(),
                this.loadPredictions(),
            ]);
            await this.loadDevices();
            this.renderStats(health);
            this.renderChips();
            this.renderDevices();
            this.renderAlerts();
            this.renderPredictions();
            document.getElementById('syncText').textContent =
                `Updated ${new Date().toLocaleTimeString()}`;
            this.setConnected(true);
        } catch (err) {
            console.error('[nimon] refresh failed:', err);
            this.setConnected(false);
        } finally {
            this.busy = false;
        }
    }

    async getJSON(url, opts) {
        const res = await fetch(url, opts);
        if (!res.ok) throw new Error(`${url} → ${res.status}`);
        return res.json();
    }

    async loadEdges() {
        const data = await this.getJSON('/api/v1/edges');
        this.edges = data.edges || [];
    }

    async loadDevices() {
        const pairs = await Promise.all(this.edges.map(async (edge) => {
            try {
                const data = await this.getJSON(
                    `/api/v1/edges/${encodeURIComponent(edge.edge_id)}/devices`);
                return [edge.edge_id, data.devices || []];
            } catch {
                return [edge.edge_id, this.devicesByEdge[edge.edge_id] || []];
            }
        }));
        this.devicesByEdge = Object.fromEntries(pairs);
    }

    async loadAlerts() {
        const data = await this.getJSON('/api/v1/alerts');
        this.alerts = data.alerts || [];
    }

    async loadPredictions() {
        const data = await this.getJSON('/api/v1/predictions');
        this.predictions = (data.predictions || [])
            .filter((p) => (p.probability ?? 0) >= 0.8)
            .sort((a, b) => b.probability - a.probability);
    }

    /* ── summary cards + app bar ────────────────────────────────── */

    renderStats(health) {
        const devices = Object.values(this.devicesByEdge).flat();
        const online = devices.filter((d) =>
            ['healthy', 'warning'].includes((d.status || '').toLowerCase())).length;
        const critAlerts = this.alerts.filter((a) =>
            (a.severity || '').toLowerCase() === 'critical').length;

        this.setText('statEdges', health?.connected_edges ?? this.edges.length);
        this.setText('statEdgesSub', this.edges.length === 1
            ? this.edges[0].name
            : `${this.edges.length} nodes reporting`);

        const devEl = document.getElementById('statDevices');
        devEl.textContent = online;
        devEl.className = 'stat-value ' + (
            devices.length === 0 ? '' :
            online === devices.length ? 'v-ok' :
            online === 0 ? 'v-crit' : 'v-warn');
        this.setText('statDevicesSub', `of ${devices.length} monitored`);

        const alertEl = document.getElementById('statAlerts');
        alertEl.textContent = this.alerts.length;
        alertEl.className = 'stat-value ' + (
            critAlerts > 0 ? 'v-crit' : this.alerts.length > 0 ? 'v-warn' : 'v-ok');
        this.setText('statAlertsSub',
            critAlerts > 0 ? `${critAlerts} critical` : 'all clear');

        const alertIcon = document.getElementById('statAlertsIcon');
        alertIcon.className = 'stat-icon tonal-error ' +
            (critAlerts > 0 ? 'hot' : this.alerts.length > 0 ? 'warm' : '');
        alertIcon.innerHTML = critAlerts > 0 ? ICONS.statError : ICONS.statWarning;

        const predEl = document.getElementById('statPreds');
        predEl.textContent = this.predictions.length;
        predEl.className = 'stat-value ' +
            (this.predictions.length > 0 ? 'v-warn' : 'v-ok');

        /* system status pill */
        const anyErr = devices.some((d) => (d.status || '').toLowerCase() === 'error');
        const anyWarn = devices.some((d) => (d.status || '').toLowerCase() === 'warning');
        const pill = document.getElementById('sysStatus');
        const pillText = document.getElementById('sysStatusText');
        const level = anyErr || critAlerts > 0 ? 'crit'
            : anyWarn || this.alerts.length > 0 ? 'warn' : '';
        pill.className = `pill ${level}`;
        pillText.textContent =
            level === 'crit' ? 'Critical issue' :
            level === 'warn' ? 'Warnings active' : 'All systems operational';
    }

    setText(id, value) {
        const el = document.getElementById(id);
        if (el) el.textContent = value;
    }

    setConnected(up) {
        if (this.connected === up) return;
        this.connected = up;
        const pill = document.getElementById('syncPill');
        pill.classList.toggle('down', !up);
        document.getElementById('syncText').textContent =
            up ? `Updated ${new Date().toLocaleTimeString()}` : 'Connection lost';
    }

    /* ── filter chips ───────────────────────────────────────────── */

    renderChips() {
        const row = document.getElementById('chipRow');
        const total = Object.values(this.devicesByEdge).flat().length;
        const chip = (id, label, count) => `
            <button class="m-chip ripple-host ${this.selectedEdge === id ? 'selected' : ''}"
                    data-edge="${this.esc(id)}">
                <span class="chip-check">${ICONS.check}</span>
                <span>${this.esc(label)}</span>
                <span class="chip-count">${count}</span>
            </button>`;
        row.innerHTML =
            chip('all', 'All edges', total) +
            this.edges.map((e) =>
                chip(e.edge_id, e.name, (this.devicesByEdge[e.edge_id] || []).length)
            ).join('');
    }

    onChip(e) {
        const btn = e.target.closest('.m-chip');
        if (!btn) return;
        this.selectedEdge = btn.dataset.edge;
        this.renderChips();
        this.renderDevices();
    }

    /* ── device cards ───────────────────────────────────────────── */

    renderDevices() {
        const body = document.getElementById('devicesList');
        const selected = this.selectedEdge === 'all'
            ? this.edges
            : this.edges.filter((e) => e.edge_id === this.selectedEdge);

        let html = '';
        let total = 0;
        let idx = 0;

        for (const edge of selected) {
            const devices = this.devicesByEdge[edge.edge_id] || [];
            if (devices.length === 0) continue;
            total += devices.length;
            html += `
                <div class="group">
                    <div class="group-head">
                        <span class="group-name">${this.esc(edge.name)}</span>
                        <span class="group-id">${this.esc(edge.edge_id)}</span>
                        <span class="panel-badge">${devices.length}</span>
                    </div>
                    <div class="device-grid">`;
            for (const device of [...devices]
                     .sort((a, b) => a.device_id.localeCompare(b.device_id))) {
                html += this.deviceHTML(edge, device, idx++);
            }
            html += '</div></div>';
        }

        this.setText('devicesCount', total > 0 ? `${total} devices` : '');
        body.innerHTML = total === 0
            ? this.empty(ICONS.devicesEmpty, 'No devices reporting')
            : html;
    }

    deviceHTML(edge, device, i) {
        const status = (device.status || 'offline').toLowerCase();
        const stale = this.isStale(device.last_seen);
        const metrics = device.metrics || {};
        const temp = typeof metrics.temperature === 'number' ? metrics.temperature : null;
        const reachable = metrics.is_reachable !== false;
        const { name, tag } = this.deviceTitle(edge.edge_id, device.device_id);

        const fresh = !this.seenDevices.has(device.device_id);
        this.seenDevices.add(device.device_id);

        const color = temp === null ? 'var(--primary)' :
            // Severity bands mirror web/src/hooks/nimon.ts (crit > 80, warn >= 65)
            temp > 80 ? 'var(--error)' :
            temp >= 65 ? 'var(--warning)' : 'var(--primary)';
        const pct = temp === null ? 0 : Math.max(0, Math.min(100, temp));

        return `
            <div class="device s-${status} ${stale ? 'stale' : ''} ${fresh ? 'enter' : ''}"
                 style="--i:${i}" title="${this.esc(device.device_id)}">
                <div class="device-head">
                    <span class="device-dot"></span>
                    <span class="device-name">${this.esc(name)}</span>
                    ${tag ? `<span class="device-tag">#${this.esc(tag)}</span>` : ''}
                    <span class="device-chip">${STATE_LABEL[status] || status}</span>
                </div>
                <div class="device-metric">
                    <span class="metric-name">${ICONS.thermostat}Temp</span>
                    <div class="m-progress"><i style="width:${pct}%;--pc:${color}"></i></div>
                    <span class="metric-value">${temp !== null ? temp.toFixed(1) + '°' : '—'}</span>
                </div>
                <div class="device-foot">
                    <span class="device-link ${reachable ? '' : 'bad'}">${reachable ? 'Connected' : 'Unreachable'}</span>
                    <span class="device-seen ${stale ? 'late' : ''}">${this.relTime(device.last_seen)}</span>
                </div>
            </div>`;
    }

    /* ── alerts ─────────────────────────────────────────────────── */

    renderAlerts() {
        const body = document.getElementById('alertsList');
        this.setText('alertsCount',
            this.alerts.length > 0 ? `${this.alerts.length} active` : '');

        if (this.alerts.length === 0) {
            body.innerHTML = this.empty(ICONS.alertsEmpty, 'No active alerts');
            return;
        }

        body.innerHTML = [...this.alerts]
            .sort((a, b) =>
                (a.severity === 'critical' ? -1 : 1) -
                (b.severity === 'critical' ? -1 : 1))
            .map((alert, i) => {
                const id = String(alert.id);
                const severity = (alert.severity || 'warning').toLowerCase();
                const acking = this.ackInFlight.has(id);
                const key = `a:${id}`;
                const fresh = !this.seenEntries.has(key);
                this.seenEntries.add(key);
                return `
                    <div class="list-item sev-${severity} ${fresh ? 'enter' : ''}" style="--i:${i}">
                        <div class="li-icon">${severity === 'critical' ? ICONS.alertCritical : ICONS.alert}</div>
                        <div class="li-body">
                            <div class="li-title-row">
                                <span class="li-title">${this.esc(alert.rule_name || 'Alert')}</span>
                            </div>
                            <div class="li-msg">${this.esc(alert.message || '')}</div>
                            <div class="li-meta">
                                ${alert.device_id ? `<span class="mono">${this.esc(this.shortId(alert.device_id))}</span>` : ''}
                                <span>${this.relTime(alert.created_at)}</span>
                                <button class="li-action ripple-host" data-ack="${id}" ${acking ? 'disabled' : ''}>
                                    ${acking ? '···' : ICONS.ackCheck + 'Acknowledge'}
                                </button>
                            </div>
                        </div>
                    </div>`;
            }).join('');
    }

    async onAck(e) {
        const btn = e.target.closest('[data-ack]');
        if (!btn) return;
        const id = btn.dataset.ack;
        this.ackInFlight.add(id);
        this.renderAlerts();
        try {
            await this.getJSON(`/api/v1/alerts/${encodeURIComponent(id)}/acknowledge`,
                { method: 'POST' });
            this.alerts = this.alerts.filter((a) => String(a.id) !== id);
            this.seenEntries.delete(`a:${id}`);
        } catch (err) {
            console.error('[nimon] ack failed:', err);
        } finally {
            this.ackInFlight.delete(id);
            this.renderAlerts();
            this.renderStats({ connected_edges: this.edges.length });
        }
    }

    /* ── predictions ────────────────────────────────────────────── */

    renderPredictions() {
        const body = document.getElementById('predsList');
        this.setText('predsCount',
            this.predictions.length > 0 ? `${this.predictions.length} active` : '');

        if (this.predictions.length === 0) {
            body.innerHTML = this.empty(ICONS.predsEmpty, 'No active predictions');
            return;
        }

        body.innerHTML = this.predictions.slice(0, 10).map((pred, i) => {
            const pct = Math.round((pred.probability ?? 0) * 100);
            const type = (pred.prediction_type || 'Unknown')
                .replace(/([a-z])([A-Z])/g, '$1 $2');
            const key = `p:${pred.id}`;
            const fresh = !this.seenEntries.has(key);
            this.seenEntries.add(key);
            return `
                <div class="list-item ${fresh ? 'enter' : ''}" style="--i:${i}">
                    <div class="li-icon">${ICONS.bolt}</div>
                    <div class="li-body">
                        <div class="li-title-row">
                            <span class="li-title">${this.esc(type)}</span>
                            <span class="li-prob">${pct}%</span>
                        </div>
                        <div class="li-bar"><i class="${pct >= 90 ? 'hot' : ''}" style="width:${pct}%"></i></div>
                        <div class="li-meta">
                            ${pred.device_id ? `<span class="mono">${this.esc(this.shortId(pred.device_id))}</span>` : ''}
                            ${pred.eta_minutes ? `<span>ETA ~${pred.eta_minutes} min</span>` : ''}
                            <span>${this.relTime(pred.created_at)}</span>
                        </div>
                    </div>
                </div>`;
        }).join('');
    }

    /* ── helpers ────────────────────────────────────────────────── */

    empty(icon, text) {
        return `
            <div class="empty">
                <div class="empty-icon">${icon}</div>
                <span>${this.esc(text)}</span>
            </div>`;
    }

    /* SQLite: "YYYY-MM-DD HH:MM:SS" (UTC); hub: RFC3339 */
    parseTs(ts) {
        if (!ts) return null;
        let s = String(ts).trim().replace(' ', 'T');
        if (!/[zZ]$/.test(s) && !/[+-]\d{2}:?\d{2}$/.test(s)) s += 'Z';
        const t = Date.parse(s);
        return Number.isNaN(t) ? null : t;
    }

    isStale(ts) {
        const t = this.parseTs(ts);
        return t === null ? true : Date.now() - t > STALE_MS;
    }

    relTime(ts) {
        const t = this.parseTs(ts);
        if (t === null) return '';
        const s = Math.max(0, Math.round((Date.now() - t) / 1000));
        if (s < 60) return `${s}s ago`;
        const m = Math.floor(s / 60);
        if (m < 60) return `${m} min ago`;
        const h = Math.floor(m / 60);
        if (h < 24) return `${h} hr ago`;
        return `${Math.floor(h / 24)} d ago`;
    }

    /* "edge-01:NI 9205#5" → { name: "NI 9205", tag: "5" } */
    deviceTitle(edgeId, deviceId) {
        let short = deviceId;
        const prefix = `${edgeId}:`;
        if (short.startsWith(prefix)) short = short.slice(prefix.length);
        const hash = short.lastIndexOf('#');
        if (hash !== -1) {
            return { name: short.slice(0, hash).trim(), tag: short.slice(hash + 1) };
        }
        return { name: short, tag: '' };
    }

    shortId(deviceId) {
        const idx = deviceId.indexOf(':');
        return idx === -1 ? deviceId : deviceId.slice(idx + 1);
    }

    esc(str) {
        if (str === null || str === undefined) return '';
        const div = document.createElement('div');
        div.textContent = String(str);
        return div.innerHTML;
    }
}

document.addEventListener('DOMContentLoaded', () => {
    window.dashboard = new Dashboard();
});
