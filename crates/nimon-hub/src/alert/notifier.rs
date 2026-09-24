//! Alert notification sender

use actix::prelude::*;
use tracing::{error, info, warn};

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::Tls;
use lettre::transport::smtp::client::TlsParameters;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use actix::fut::wrap_future;
use nimon_core::alert::{Alert, AlertStatus, ChannelType, NotificationChannel, Severity};

/// SMTP transport security.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTlsMode {
    /// TLS from the first byte (SMTPS, port 465)
    Implicit,
    /// STARTTLS, required (port 587)
    StartTls,
    /// STARTTLS when offered, plaintext otherwise (port 25)
    Opportunistic,
    /// No TLS
    None,
}

/// TLS mode from the configured value, or by port when unset
/// (465 implicit, 25 opportunistic, anything else STARTTLS).
pub fn smtp_tls_mode(port: u16, configured: Option<&str>) -> Result<SmtpTlsMode, String> {
    match configured.map(|s| s.trim().to_ascii_lowercase()) {
        None => Ok(match port {
            465 => SmtpTlsMode::Implicit,
            25 => SmtpTlsMode::Opportunistic,
            _ => SmtpTlsMode::StartTls,
        }),
        Some(s) => match s.as_str() {
            "" => smtp_tls_mode(port, None),
            "implicit" | "wrapper" | "smtps" | "tls" => Ok(SmtpTlsMode::Implicit),
            "starttls" | "required" => Ok(SmtpTlsMode::StartTls),
            "opportunistic" => Ok(SmtpTlsMode::Opportunistic),
            "none" | "plain" | "plaintext" => Ok(SmtpTlsMode::None),
            other => Err(format!("unknown smtp_tls mode '{}'", other)),
        },
    }
}

/// Mode actually used: with credentials and without
/// `allow_plaintext_auth`, opportunistic TLS is upgraded to required
/// STARTTLS and `None` is refused, so credentials never travel in clear.
pub fn effective_tls_mode(
    mode: SmtpTlsMode,
    has_credentials: bool,
    allow_plaintext_auth: bool,
) -> Result<SmtpTlsMode, String> {
    if !has_credentials || allow_plaintext_auth {
        return Ok(mode);
    }
    match mode {
        SmtpTlsMode::Opportunistic => Ok(SmtpTlsMode::StartTls),
        SmtpTlsMode::None => Err(
            "refusing to send SMTP credentials without TLS (set smtp_allow_plaintext_auth: true to override)"
                .to_string(),
        ),
        other => Ok(other),
    }
}

/// Alert notifier actor
pub struct AlertNotifier {
    channels: Vec<NotificationChannel>,
    http: reqwest::Client,
}

impl AlertNotifier {
    pub fn new(channels: Vec<NotificationChannel>) -> Self {
        Self {
            channels,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Resolve the channels a notification should go to: the rule's
    /// requested channels when set, otherwise every enabled channel.
    fn resolve_channels(&self, requested: &[String]) -> Vec<&NotificationChannel> {
        if requested.is_empty() {
            return self.channels.iter().filter(|c| c.enabled).collect();
        }
        self.channels
            .iter()
            .filter(|c| c.enabled && requested.iter().any(|r| r == &c.id || r == &c.name))
            .collect()
    }

    async fn send_alert(&self, alert: &Alert, requested: &[String]) {
        let channels = self.resolve_channels(requested);
        if channels.is_empty() {
            // Never lose the alert entirely: log to console.
            warn!(
                "No matching notification channels for {} (requested {:?}); logging to console only",
                alert.id, requested
            );
            log_console(alert);
            return;
        }

        for channel in channels {
            match &channel.channel_type {
                ChannelType::Console => log_console(alert),
                ChannelType::Email {
                    smtp_server,
                    smtp_port,
                    username,
                    from_addr,
                    to_addrs,
                } => {
                    let cfg = &channel.config;
                    let skip_tls_verify = cfg
                        .get("smtp_skip_tls_verify")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let allow_plaintext = cfg
                        .get("smtp_allow_plaintext_auth")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let tls = cfg.get("smtp_tls").and_then(|v| v.as_str());
                    let password = cfg
                        .get("smtp_password")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let email = EmailTarget {
                        server: smtp_server,
                        port: *smtp_port,
                        username: username.as_deref(),
                        password: password.as_deref(),
                        from_addr,
                        to_addrs,
                        tls,
                        skip_tls_verify,
                        allow_plaintext,
                    };
                    if let Err(e) = self.send_email(&email, alert).await {
                        error!("Email notification failed for {}: {}", channel.id, e);
                    }
                }
                ChannelType::Webhook { url, headers } => {
                    if let Err(e) = self.send_webhook(url, headers.as_ref(), alert).await {
                        error!("Webhook failed for {}: {}", channel.id, e);
                    }
                }
                ChannelType::Slack { webhook_url } => {
                    if let Err(e) = self.send_slack(webhook_url, alert).await {
                        error!("Slack notification failed for {}: {}", channel.id, e);
                    }
                }
                ChannelType::Teams { webhook_url } => {
                    if let Err(e) = self.send_teams(webhook_url, alert).await {
                        error!("Teams notification failed for {}: {}", channel.id, e);
                    }
                }
            }
        }
    }

    async fn send_webhook(
        &self,
        url: &str,
        headers: Option<&std::collections::HashMap<String, String>>,
        alert: &Alert,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let payload = serde_json::json!({
            "alert_id": alert.id,
            "rule_id": alert.rule_id,
            "edge_id": alert.edge_id,
            "device_id": alert.device_id,
            "severity": alert.severity.to_string(),
            "status": alert.status.to_string(),
            "title": display_title(alert),
            "message": alert.message,
            "triggered_at": alert.triggered_at.to_rfc3339(),
            "resolved_at": alert.resolved_at.map(|t| t.to_rfc3339()),
        });

        let mut request = self.http.post(url).json(&payload);
        if let Some(headers) = headers {
            for (name, value) in headers {
                request = request.header(name, value);
            }
        }
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("webhook returned {}", status).into());
        }
        Ok(())
    }

    async fn send_slack(
        &self,
        webhook_url: &str,
        alert: &Alert,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let color = if alert.status == AlertStatus::Resolved {
            "good"
        } else {
            match alert.severity {
                Severity::Info => "#36a64f",
                Severity::Warning => "warning",
                Severity::Critical => "danger",
            }
        };

        let payload = serde_json::json!({
            "attachments": [{
                "color": color,
                "title": display_title(alert),
                "text": alert.message,
                "fields": [
                    {"title": "Device", "value": alert.device_id, "short": true},
                    {"title": "Severity", "value": alert.severity.to_string(), "short": true},
                    {"title": "Status", "value": alert.status.to_string(), "short": true},
                    {"title": "Time", "value": alert.triggered_at.to_rfc3339(), "short": true},
                ]
            }]
        });

        let response = self.http.post(webhook_url).json(&payload).send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("slack webhook returned {}", status).into());
        }
        Ok(())
    }

    async fn send_teams(
        &self,
        webhook_url: &str,
        alert: &Alert,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let color = if alert.status == AlertStatus::Resolved {
            "2eb886"
        } else {
            match alert.severity {
                Severity::Info => "008000",
                Severity::Warning => "ff8c00",
                Severity::Critical => "ff0000",
            }
        };

        let payload = serde_json::json!({
            "@type": "MessageCard",
            "@context": "https://schema.org/extensions",
            "summary": display_title(alert),
            "themeColor": color,
            "sections": [{
                "activityTitle": display_title(alert),
                "text": alert.message,
                "facts": [
                    {"name": "Device", "value": alert.device_id},
                    {"name": "Severity", "value": alert.severity.to_string()},
                    {"name": "Status", "value": alert.status.to_string()},
                    {"name": "Time", "value": alert.triggered_at.to_rfc3339()},
                ]
            }]
        });

        let response = self.http.post(webhook_url).json(&payload).send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("teams webhook returned {}", status).into());
        }
        Ok(())
    }

    async fn send_email(
        &self,
        target: &EmailTarget<'_>,
        alert: &Alert,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let to_list = target
            .to_addrs
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let subject = if alert.status == AlertStatus::Resolved {
            format!("[RESOLVED][{}] {}", alert.severity, alert.title)
        } else {
            format!("[{}] {}", alert.severity, alert.title)
        };
        let email = Message::builder()
            .from(target.from_addr.parse()?)
            .to(to_list.parse()?)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(alert.message.clone())?;

        let credentials = match (target.username, target.password) {
            (Some(user), Some(pass)) => Some(Credentials::new(user.to_string(), pass.to_string())),
            _ => None,
        };
        let mode = effective_tls_mode(
            smtp_tls_mode(target.port, target.tls)?,
            credentials.is_some(),
            target.allow_plaintext,
        )?;

        let tls_params = || {
            let mut tls = TlsParameters::builder(target.server.to_string());
            if target.skip_tls_verify {
                tls = tls.dangerous_accept_invalid_certs(true);
            }
            tls.build()
        };
        let tls = match mode {
            SmtpTlsMode::Implicit => Tls::Wrapper(tls_params()?),
            SmtpTlsMode::StartTls => Tls::Required(tls_params()?),
            SmtpTlsMode::Opportunistic => Tls::Opportunistic(tls_params()?),
            SmtpTlsMode::None => Tls::None,
        };

        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(target.server)
            .port(target.port)
            .tls(tls)
            .timeout(Some(std::time::Duration::from_secs(30)));
        if let Some(credentials) = credentials {
            builder = builder.credentials(credentials);
        }

        builder.build().send(email).await?;
        Ok(())
    }
}

/// Everything needed to deliver one email.
struct EmailTarget<'a> {
    server: &'a str,
    port: u16,
    username: Option<&'a str>,
    password: Option<&'a str>,
    from_addr: &'a str,
    to_addrs: &'a [String],
    tls: Option<&'a str>,
    skip_tls_verify: bool,
    allow_plaintext: bool,
}

/// Title with a `[RESOLVED]` prefix for resolution notifications.
fn display_title(alert: &Alert) -> String {
    if alert.status == AlertStatus::Resolved {
        format!("[RESOLVED] {}", alert.title)
    } else {
        alert.title.clone()
    }
}

fn log_console(alert: &Alert) {
    if alert.status == AlertStatus::Resolved {
        info!(
            "[ALERT resolved] {} - {}: {}",
            alert.device_id, alert.title, alert.message
        );
        return;
    }
    match alert.severity {
        Severity::Critical => error!(
            "[ALERT critical] {} - {}: {}",
            alert.device_id, alert.title, alert.message
        ),
        Severity::Warning => warn!(
            "[ALERT warning] {} - {}: {}",
            alert.device_id, alert.title, alert.message
        ),
        Severity::Info => info!(
            "[ALERT info] {} - {}: {}",
            alert.device_id, alert.title, alert.message
        ),
    }
}

impl Actor for AlertNotifier {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        info!(
            "AlertNotifier actor started with {} channels",
            self.channels.len()
        );
    }
}

/// Message to send a notification. An alert with status `Resolved` is
/// sent as a resolution notice.
#[derive(Message)]
#[rtype(result = "()")]
pub struct SendNotification {
    pub alert: Alert,
    /// Channel ids the rule requested; empty means all enabled channels
    pub channels: Vec<String>,
}

impl Handler<SendNotification> for AlertNotifier {
    type Result = ();

    fn handle(&mut self, msg: SendNotification, ctx: &mut Self::Context) -> Self::Result {
        // Deliveries run concurrently so one slow channel does not delay
        // later notifications.
        let notifier = self.clone();
        ctx.spawn(wrap_future(async move {
            notifier.send_alert(&msg.alert, &msg.channels).await;
        }));
    }
}

impl Clone for AlertNotifier {
    fn clone(&self) -> Self {
        Self {
            channels: self.channels.clone(),
            http: self.http.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console_channel(id: &str, enabled: bool) -> NotificationChannel {
        NotificationChannel {
            id: id.to_string(),
            name: id.to_string(),
            channel_type: ChannelType::Console,
            enabled,
            config: serde_json::json!({}),
        }
    }

    #[test]
    fn test_resolve_channels_empty_requests_all_enabled() {
        let notifier = AlertNotifier::new(vec![
            console_channel("a", true),
            console_channel("b", false),
            console_channel("c", true),
        ]);
        assert_eq!(notifier.resolve_channels(&[]).len(), 2);
    }

    #[test]
    fn test_resolve_channels_per_rule_routing() {
        let notifier = AlertNotifier::new(vec![
            console_channel("console", true),
            console_channel("slack-ops", true),
        ]);
        let resolved = notifier.resolve_channels(&["slack-ops".to_string()]);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].id, "slack-ops");
    }

    #[test]
    fn test_resolve_channels_no_match() {
        let notifier = AlertNotifier::new(vec![console_channel("console", true)]);
        assert!(notifier
            .resolve_channels(&["nonexistent".to_string()])
            .is_empty());
    }

    #[test]
    fn test_smtp_tls_mode_by_port() {
        assert_eq!(smtp_tls_mode(465, None).unwrap(), SmtpTlsMode::Implicit);
        assert_eq!(smtp_tls_mode(587, None).unwrap(), SmtpTlsMode::StartTls);
        assert_eq!(smtp_tls_mode(25, None).unwrap(), SmtpTlsMode::Opportunistic);
        assert_eq!(smtp_tls_mode(2525, None).unwrap(), SmtpTlsMode::StartTls);
        assert_eq!(
            smtp_tls_mode(587, Some("wrapper")).unwrap(),
            SmtpTlsMode::Implicit
        );
        assert_eq!(smtp_tls_mode(25, Some("none")).unwrap(), SmtpTlsMode::None);
        assert!(smtp_tls_mode(25, Some("bogus")).is_err());
    }

    #[test]
    fn test_credentials_never_plaintext_by_default() {
        // No credentials: mode unchanged
        assert_eq!(
            effective_tls_mode(SmtpTlsMode::Opportunistic, false, false).unwrap(),
            SmtpTlsMode::Opportunistic
        );
        // Credentials: opportunistic upgraded, none refused
        assert_eq!(
            effective_tls_mode(SmtpTlsMode::Opportunistic, true, false).unwrap(),
            SmtpTlsMode::StartTls
        );
        assert!(effective_tls_mode(SmtpTlsMode::None, true, false).is_err());
        // Explicitly allowed
        assert_eq!(
            effective_tls_mode(SmtpTlsMode::None, true, true).unwrap(),
            SmtpTlsMode::None
        );
        assert_eq!(
            effective_tls_mode(SmtpTlsMode::Implicit, true, false).unwrap(),
            SmtpTlsMode::Implicit
        );
    }

    #[test]
    fn test_resolved_title_prefix() {
        let mut alert = Alert {
            id: "a".into(),
            rule_id: "r".into(),
            edge_id: "e".into(),
            device_id: "d".into(),
            severity: Severity::Warning,
            status: AlertStatus::Firing,
            title: "Hot".into(),
            message: "m".into(),
            metric_name: None,
            metric_value: None,
            threshold: None,
            triggered_at: chrono::Utc::now(),
            resolved_at: None,
            fired_count: 1,
            notification_sent: false,
        };
        assert_eq!(display_title(&alert), "Hot");
        alert.status = AlertStatus::Resolved;
        assert_eq!(display_title(&alert), "[RESOLVED] Hot");
    }
}
