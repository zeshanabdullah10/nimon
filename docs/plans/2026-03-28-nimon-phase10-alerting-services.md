# NIMon Phase 10: Alerting, Service Management, Documentation

**Goal:** Complete email alerting, add systemd and Windows service support, overhaul the README.

---

## Task 1: Complete Email Alerting

### Problem
`ChannelType::Email` exists in `nimon-core/src/alert/mod.rs` but the sender is TODO in `nimon-hub/src/alert/notifier.rs`.

### Implementation

**Add `lettre` dependency to `crates/nimon-hub/Cargo.toml`:**
```toml
lettre = "0.11"
```

**Implement `send_email` in `crates/nimon-hub/src/alert/notifier.rs`:**
```rust
async fn send_email(&self, smtp_server: &str, from: &str, to_addrs: &[String], alert: &Alert) -> Result<(), Box<dyn std::error::Error>> {
    let relay = TokioSmtpRelay::new(smtp_server);
    let email = Email::build()
        .from(from)
        .to(to_addrs.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .subject(format!("[{:?}] {}", alert.severity, alert.title))
        .text(alert.message.clone())
        .build();

    // Use Tokio runtime-compatible email sending
    // ... implementation
}
```

**Update `send_alert` match arm for `ChannelType::Email`:**
```rust
ChannelType::Email { smtp_server, from_addr, to_addrs } => {
    if let Err(e) = self.send_email(smtp_server, from_addr, to_addrs, alert).await {
        error!("Email notification failed for {}: {}", channel.id, e);
    }
}
```

**Config format in `config/hub.yaml`:**
```yaml
notification_channels:
  - channel_type: "email"
    name: "ops-team"
    webhook_url: "smtp.example.com:587"  # smtp_server
    from_addr: "nimon@example.com"
    to_addrs: ["ops@example.com"]
    enabled: true
```

---

## Task 2: Systemd Service

### Files
- `systemd/nimon-hub.service`

### Content
```ini
[Unit]
Description=NIMon Hub Server - NI Hardware Monitoring Platform
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=nimon
Group=nimon
WorkingDirectory=/opt/nimon
ExecStart=/opt/nimon/nimon-hub /opt/nimon/config/hub.yaml
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

### Installation
```bash
sudo cp systemd/nimon-hub.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable nimon-hub
sudo systemctl start nimon-hub
```

---

## Task 3: Windows Service

### Implementation
Use `windows` crate for raw Windows Service API. Add to `nimon-hub`:

**New file: `crates/nimon-hub/src/service.rs`**

```rust
use windows::Win32::Foundation::*;
use windows::Win32::System::Services::*;

pub struct WindowsService {
    status_handle: SERVICE_STATUS_HANDLE,
}

impl WindowsService {
    pub fn run(service_name: &str) -> Result<(), Box<dyn std::error::Error>> {
        // Register service control handler
        // Start service as Single-Threaded-Apartment
        // Call into async run_server() on a tokio runtime
        // Report status to SCM via SetServiceStatus()
    }

    pub fn install(service_name: &str, display_name: &str, exe_path: &str) -> Result<(), Box<dyn std::error::Error>> {
        // CreateService() via OpenSCManager()
    }

    pub fn uninstall(service_name: &str) -> Result<(), Box<dyn std::error::Error>> {
        // DeleteService() via OpenSCManager()
    }
}
```

**Add CLI flags to `nimon-hub`:**
```bash
nimon-hub install     # install as Windows service
nimon-hub uninstall   # remove Windows service
nimon-hub start       # start the service
nimon-hub stop        # stop the service
```

---

## Task 4: README Overhaul

Replace existing README with complete documentation:

### Sections
1. **Overview** — What NIMon is
2. **Architecture** — ASCII diagram (already present)
3. **Quick Start** — 3 commands to running
4. **Build** — `cargo build --workspace --release`
5. **Binaries** — List all 4 binaries
6. **Configuration** — Full `hub.yaml` reference
7. **Notification Channels** — email, Slack, Teams, webhook, console
8. **REST API** — All endpoints with descriptions
9. **CLI Tool** — All commands with examples
10. **Service Installation**
    - Linux (systemd)
    - Windows (sc.exe or nimon-hub install)
11. **Edge Simulator** — How to use for testing
12. **Development** — Testing, formatting, clippy
