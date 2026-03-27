const API_BASE = '';

class NIMonDashboard {
    constructor() {
        this.edges = [];
        this.alerts = [];
        this.pollInterval = 5000;
        this.init();
    }

    async init() {
        await this.fetchData();
        this.startPolling();
    }

    startPolling() {
        setInterval(() => this.fetchData(), this.pollInterval);
    }

    async fetchData() {
        try {
            await Promise.all([
                this.fetchHealth(),
                this.fetchEdges(),
                this.fetchAlerts()
            ]);
            this.updateConnectionStatus(true);
        } catch (err) {
            console.error('Failed to fetch data:', err);
            this.updateConnectionStatus(false);
        }
    }

    async fetchHealth() {
        const res = await fetch(`${API_BASE}/health`);
        const data = await res.json();
        this.updateStats(data);
    }

    async fetchEdges() {
        const res = await fetch(`${API_BASE}/api/v1/edges`);
        const data = await res.json();
        this.edges = data.edges || [];
        this.renderEdges();
    }

    async fetchAlerts() {
        const res = await fetch(`${API_BASE}/api/v1/alerts`);
        const data = await res.json();
        this.alerts = data.alerts || [];
        this.renderAlerts();
    }

    updateStats(data) {
        const edgesCountEl = document.getElementById('edgesCount');
        const alertsCountEl = document.getElementById('alertsCount');
        const systemStatusEl = document.getElementById('systemStatus');

        if (edgesCountEl) {
            edgesCountEl.textContent = data.connected_edges ?? '—';
        }

        if (alertsCountEl) {
            alertsCountEl.textContent = this.alerts.length;
        }

        if (systemStatusEl) {
            if (data.status === 'healthy') {
                systemStatusEl.innerHTML = '<span class="status-healthy">HEALTHY</span>';
            } else if (data.status === 'warning') {
                systemStatusEl.innerHTML = '<span class="status-warning">WARNING</span>';
            } else {
                systemStatusEl.innerHTML = '<span class="status-critical">CRITICAL</span>';
            }
        }

        const lastUpdatedEl = document.getElementById('lastUpdated');
        if (lastUpdatedEl) {
            lastUpdatedEl.textContent = new Date().toLocaleTimeString();
        }
    }

    renderEdges() {
        const container = document.getElementById('edgesList');
        if (!container) return;

        if (this.edges.length === 0) {
            container.innerHTML = '<div class="empty-state">No edges connected</div>';
            return;
        }

        container.innerHTML = this.edges.map(edge => `
            <div class="edge-item">
                <div class="edge-status"></div>
                <div class="edge-info">
                    <div class="edge-name">${this.escapeHtml(edge.name)}</div>
                    <div class="edge-id">${this.escapeHtml(edge.edge_id)}</div>
                </div>
                <div class="edge-devices">${edge.device_count || 0} devices</div>
            </div>
        `).join('');
    }

    renderAlerts() {
        const container = document.getElementById('alertsList');
        if (!container) return;

        if (this.alerts.length === 0) {
            container.innerHTML = '<div class="empty-state">No active alerts</div>';
            return;
        }

        container.innerHTML = this.alerts.map(alert => {
            const severity = alert.severity?.toLowerCase() || 'warning';
            return `
                <div class="alert-item ${severity}">
                    <div class="alert-header">
                        <span class="alert-title">${this.escapeHtml(alert.title || 'Alert')}</span>
                        <span class="alert-severity ${severity}">${this.escapeHtml(severity)}</span>
                    </div>
                    <div class="alert-message">${this.escapeHtml(alert.message || '')}</div>
                    <div class="alert-time">${this.formatTime(alert.triggered_at)}</div>
                </div>
            `;
        }).join('');
    }

    updateConnectionStatus(connected) {
        const statusEl = document.getElementById('connectionStatus');
        if (!statusEl) return;

        const dot = statusEl.querySelector('.status-dot');
        const text = statusEl.querySelector('.status-text');

        if (dot && text) {
            if (connected) {
                dot.className = 'status-dot connected';
                text.textContent = 'Connected';
            } else {
                dot.className = 'status-dot disconnected';
                text.textContent = 'Disconnected';
            }
        }
    }

    escapeHtml(str) {
        if (str === null || str === undefined) return '';
        const div = document.createElement('div');
        div.textContent = String(str);
        return div.innerHTML;
    }

    formatTime(timestamp) {
        if (!timestamp) return '';
        try {
            return new Date(timestamp).toLocaleString();
        } catch {
            return timestamp;
        }
    }
}

document.addEventListener('DOMContentLoaded', () => {
    window.dashboard = new NIMonDashboard();
});
