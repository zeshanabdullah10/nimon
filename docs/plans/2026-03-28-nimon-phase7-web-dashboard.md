# NIMon Phase 7: Web Dashboard

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a real-time web dashboard for NIMon's predictive maintenance system. Display edge nodes, devices, alerts, and predictions in a cohesive industrial control room aesthetic.

**Tech Stack:** Vanilla HTML/CSS/JS (no WASM complexity), Axum (existing hub backend serves static files)

---

## Design Direction: "Industrial Control Room"

**Purpose**: Monitoring dashboard for industrial NI hardware (PXI, DAQ, etc.) — operators need at-a-glance status, drill-down capability, and historical context.

**Aesthetic**: Dark, precise, data-dense. Like a Bloomberg terminal for hardware. Evokes the feeling of a live industrial control room — everything is intentional, nothing is decorative without purpose.

**Color Palette**:
- Background: `#0a0e14` (deep slate)
- Surface: `#141a22` (card backgrounds)
- Border: `#1e2530` (subtle dividers)
- Text Primary: `#e6edf3` (high contrast)
- Text Secondary: `#7d8590` (muted labels)
- Accent Cyan: `#00d4ff` (healthy/online/active)
- Warning Amber: `#ffb347` (warning state)
- Critical Red: `#ff4757` (critical alerts)
- Success Green: `#00ff88` (resolved/healthy)

**Typography**:
- Display/Headers: `Space Grotesk` (geometric, technical feel)
- Data/Metrics: `JetBrains Mono` (precise, readable for numbers)
- Body: `IBM Plex Sans` (clean, industrial)

**Key Differentiator**: Live data pulses. Active connections breathe with subtle cyan glows. Critical alerts pulse in red. The dashboard feels alive — monitoring real hardware in real-time.

---

## Task 1: Create Static Directory and Basic HTML

**Files:**
- Create: `crates/nimon-hub/static/index.html`
- Create: `crates/nimon-hub/static/style.css`
- Create: `crates/nimon-hub/static/app.js`

**Step 1: Create index.html**

```html
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>NIMon Dashboard</title>
    <link rel="preconnect" href="https://fonts.googleapis.com">
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
    <link href="https://fonts.googleapis.com/css2?family=IBM+Plex+Sans:wght@400;500;600&family=JetBrains+Mono:wght@400;500&family=Space+Grotesk:wght@500;700&display=swap" rel="stylesheet">
    <link rel="stylesheet" href="/style.css">
</head>
<body>
    <div class="dashboard">
        <!-- Header -->
        <header class="header">
            <div class="header-left">
                <div class="logo">
                    <span class="logo-mark">◈</span>
                    <span class="logo-text">NIMon</span>
                </div>
                <span class="version-badge">v0.1.0</span>
            </div>
            <div class="header-right">
                <div class="connection-status" id="connectionStatus">
                    <span class="status-dot"></span>
                    <span class="status-text">Connecting...</span>
                </div>
            </div>
        </header>

        <!-- Stats Row -->
        <section class="stats-row" id="statsRow">
            <div class="stat-card">
                <div class="stat-label">Edges Connected</div>
                <div class="stat-value" id="edgesCount">—</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Active Alerts</div>
                <div class="stat-value" id="alertsCount">—</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">System Status</div>
                <div class="stat-value status-healthy" id="systemStatus">—</div>
            </div>
        </section>

        <!-- Main Grid -->
        <main class="main-grid">
            <!-- Edges Panel -->
            <section class="panel edges-panel">
                <div class="panel-header">
                    <h2>Edge Nodes</h2>
                </div>
                <div class="panel-content" id="edgesList">
                    <div class="empty-state">No edges connected</div>
                </div>
            </section>

            <!-- Alerts Panel -->
            <section class="panel alerts-panel">
                <div class="panel-header">
                    <h2>Active Alerts</h2>
                </div>
                <div class="panel-content" id="alertsList">
                    <div class="empty-state">No active alerts</div>
                </div>
            </section>
        </main>

        <!-- Footer -->
        <footer class="footer">
            <span>Last updated: <span id="lastUpdated">—</span></span>
        </footer>
    </div>

    <script src="/app.js"></script>
</body>
</html>
```

---

## Task 2: CSS with Industrial Design System

**Files:**
- Modify: `crates/nimon-hub/static/style.css`

**Step 1: Complete CSS**

```css
:root {
    --bg-deep: #0a0e14;
    --bg-surface: #141a22;
    --bg-elevated: #1a222c;
    --border: #1e2530;
    --border-bright: #2d3848;
    --text-primary: #e6edf3;
    --text-secondary: #7d8590;
    --text-muted: #484f58;
    --accent-cyan: #00d4ff;
    --accent-cyan-dim: rgba(0, 212, 255, 0.15);
    --accent-amber: #ffb347;
    --accent-amber-dim: rgba(255, 179, 71, 0.15);
    --accent-red: #ff4757;
    --accent-red-dim: rgba(255, 71, 87, 0.15);
    --accent-green: #00ff88;
    --accent-green-dim: rgba(0, 255, 136, 0.15);
    --font-display: 'Space Grotesk', system-ui, sans-serif;
    --font-mono: 'JetBrains Mono', 'Fira Code', monospace;
    --font-body: 'IBM Plex Sans', system-ui, sans-serif;
    --radius-sm: 4px;
    --radius-md: 8px;
    --radius-lg: 12px;
}

*, *::before, *::after { margin: 0; padding: 0; box-sizing: border-box; }

body {
    font-family: var(--font-body);
    background: var(--bg-deep);
    color: var(--text-primary);
    min-height: 100vh;
    line-height: 1.6;
}

.dashboard {
    display: flex;
    flex-direction: column;
    min-height: 100vh;
}

/* Header */
.header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 16px 24px;
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border);
}

.header-left { display: flex; align-items: center; gap: 16px; }

.logo { display: flex; align-items: center; gap: 8px; }
.logo-mark {
    font-size: 24px;
    color: var(--accent-cyan);
    animation: pulse-glow 2s ease-in-out infinite;
}
.logo-text {
    font-family: var(--font-display);
    font-size: 20px;
    font-weight: 700;
    letter-spacing: -0.5px;
}

.version-badge {
    font-family: var(--font-mono);
    font-size: 11px;
    padding: 2px 8px;
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
}

.connection-status { display: flex; align-items: center; gap: 8px; }
.status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--text-muted);
    transition: background 0.3s;
}
.status-dot.connected { background: var(--accent-green); box-shadow: 0 0 8px var(--accent-green); }
.status-dot.disconnected { background: var(--accent-red); }
.status-text { font-size: 13px; color: var(--text-secondary); }

/* Stats Row */
.stats-row {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 16px;
    padding: 24px;
}

.stat-card {
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: 20px 24px;
}

.stat-label {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-secondary);
    margin-bottom: 8px;
}

.stat-value {
    font-family: var(--font-mono);
    font-size: 32px;
    font-weight: 500;
    color: var(--text-primary);
}

.stat-value.status-healthy { color: var(--accent-green); }
.stat-value.status-warning { color: var(--accent-amber); }
.stat-value.status-critical { color: var(--accent-red); }

/* Main Grid */
.main-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px;
    padding: 0 24px 24px;
    flex: 1;
}

.panel {
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    display: flex;
    flex-direction: column;
}

.panel-header {
    padding: 16px 20px;
    border-bottom: 1px solid var(--border);
}
.panel-header h2 {
    font-family: var(--font-display);
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
}

.panel-content { flex: 1; padding: 12px; overflow-y: auto; }

/* Edge Item */
.edge-item {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px;
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    margin-bottom: 8px;
    transition: border-color 0.2s;
}
.edge-item:hover { border-color: var(--border-bright); }
.edge-item:last-child { margin-bottom: 0; }

.edge-status {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--accent-cyan);
    box-shadow: 0 0 8px var(--accent-cyan);
    animation: pulse-glow 2s ease-in-out infinite;
}

.edge-info { flex: 1; }
.edge-name {
    font-weight: 500;
    font-size: 14px;
    margin-bottom: 2px;
}
.edge-id {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-secondary);
}
.edge-devices {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-muted);
}

/* Alert Item */
.alert-item {
    padding: 12px;
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    margin-bottom: 8px;
    border-left: 3px solid var(--accent-red);
}
.alert-item:last-child { margin-bottom: 0; }
.alert-item.critical { border-left-color: var(--accent-red); background: var(--accent-red-dim); }
.alert-item.warning { border-left-color: var(--accent-amber); background: var(--accent-amber-dim); }

.alert-header { display: flex; justify-content: space-between; margin-bottom: 4px; }
.alert-title { font-weight: 500; font-size: 13px; }
.alert-severity {
    font-family: var(--font-mono);
    font-size: 10px;
    padding: 2px 6px;
    border-radius: var(--radius-sm);
    text-transform: uppercase;
}
.alert-severity.critical { background: var(--accent-red); color: #fff; }
.alert-severity.warning { background: var(--accent-amber); color: #000; }
.alert-message { font-size: 12px; color: var(--text-secondary); }
.alert-time { font-family: var(--font-mono); font-size: 10px; color: var(--text-muted); margin-top: 4px; }

/* Empty State */
.empty-state {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 120px;
    color: var(--text-muted);
    font-size: 13px;
}

/* Footer */
.footer {
    padding: 12px 24px;
    border-top: 1px solid var(--border);
    font-size: 11px;
    color: var(--text-muted);
    font-family: var(--font-mono);
}

/* Animations */
@keyframes pulse-glow {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.5; }
}

/* Responsive */
@media (max-width: 768px) {
    .stats-row { grid-template-columns: 1fr; }
    .main-grid { grid-template-columns: 1fr; }
}
```

---

## Task 3: JavaScript API Client and Dashboard Logic

**Files:**
- Create: `crates/nimon-hub/static/app.js`

**Step 1: Complete JavaScript**

```javascript
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
        document.getElementById('edgesCount').textContent = data.connected_edges;
        document.getElementById('alertsCount').textContent = this.alerts.length;

        const status = data.status === 'healthy'
            ? '<span class="status-healthy">HEALTHY</span>'
            : '<span class="status-warning">DEGRADED</span>';
        document.getElementById('systemStatus').innerHTML = status;

        document.getElementById('lastUpdated').textContent = new Date().toLocaleTimeString();
    }

    renderEdges() {
        const container = document.getElementById('edgesList');
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
        const dot = statusEl.querySelector('.status-dot');
        const text = statusEl.querySelector('.status-text');
        if (connected) {
            dot.className = 'status-dot connected';
            text.textContent = 'Connected';
        } else {
            dot.className = 'status-dot disconnected';
            text.textContent = 'Disconnected';
        }
    }

    escapeHtml(str) {
        if (!str) return '';
        const div = document.createElement('div');
        div.textContent = str;
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
```

---

## Task 4: Serve Static Files from Hub

**Files:**
- Modify: `crates/nimon-hub/src/server/mod.rs`
- Modify: `crates/nimon-hub/Cargo.toml`

**Step 1: Add tower-http fs dependency**

Update `crates/nimon-hub/Cargo.toml`:
```toml
tower-http = { version = "0.5", features = ["fs"] }
```

**Step 2: Add static file serving to router**

In `crates/nimon-hub/src/server/mod.rs`, add after the existing routes:
```rust
use tower_http::fs::ServeDir;

// In the Router builder, after .with_state(state):
.nest_service("/", ServeDir::new("static"))
```

Note: You may need to adjust the static path to point to the crate's static directory.

---

## Task 5: Build and Test

**Step 1: Build the hub**
```bash
cargo build -p nimon-hub --release
```

**Step 2: Start hub and verify**
```bash
cargo run -p nimon-hub --release -- config/test-hub.yaml
# Open http://localhost:9090
```

Expected: Dashboard loads, shows edge connections, updates every 5 seconds.

**Step 3: Run tests**
```bash
cargo test --workspace
```

---

## Commit

```bash
git add crates/nimon-hub/static/
git add crates/nimon-hub/src/
git commit -m "feat(web): add NIMon dashboard with industrial control room aesthetic"
```
