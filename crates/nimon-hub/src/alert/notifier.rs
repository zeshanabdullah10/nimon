//! Alert notification sender

use actix::prelude::*;
use tracing::{error, info, warn};

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::Tls;
use lettre::transport::smtp::client::TlsParameters;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use actix::fut::wrap_future;
use nimon_core::alert::{Alert, ChannelType, NotificationChannel, Severity};

/// Alert notifier actor
pub struct AlertNotifier {
    channels: Vec<NotificationChannel>,
    http: reqwest::Client,
}

impl AlertNotifier {
    pub fn new(channels: Vec<NotificationChannel>) -> Self {
        Self {
            channels,
            http: reqwest::Client::new(),
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
            if !channel.enabled {
                continue;
            }

            match &channel.channel_type {
                ChannelType::Console => log_console(alert),
                ChannelType::Email {
                    smtp_server,
                    smtp_port,
                    username,
                    from_addr,
                    to_addrs,
                } => {
                    let skip_tls_verify = channel
                        .config
                        .get("smtp_skip_tls_verify")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let password = channel
                        .config
                        .get("smtp_password")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let username = username.clone().or_else(|| {
                        channel
                            .config
                            .get("smtp_username_env")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    });
                    if let Err(e) = self
                        .send_email(
                            smtp_server,
                            *smtp_port,
                            username.as_deref(),
                            password.as_deref(),
                            from_addr,
                            to_addrs,
                            skip_tls_verify,
                            alert,
                        )
                        .await
                    {
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
            "title": alert.title,
            "message": alert.message,
            "triggered_at": alert.triggered_at.to_rfc3339(),
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
        let color = match alert.severity {
            Severity::Info => "#36a64f",
            Severity::Warning => "warning",
            Severity::Critical => "danger",
        };

        let payload = serde_json::json!({
            "attachments": [{
                "color": color,
                "title": alert.title,
                "text": alert.message,
                "fields": [
                    {"title": "Device", "value": alert.device_id, "short": true},
                    {"title": "Severity", "value": alert.severity.to_string(), "short": true},
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
        let color = match alert.severity {
            Severity::Info => "008000",
            Severity::Warning => "ff8c00",
            Severity::Critical => "ff0000",
        };

        let payload = serde_json::json!({
            "@type": "MessageCard",
            "@context": "https://schema.org/extensions",
            "summary": alert.title,
            "themeColor": color,
            "sections": [{
                "activityTitle": alert.message,
                "facts": [
                    {"name": "Device", "value": alert.device_id},
                    {"name": "Severity", "value": alert.severity.to_string()},
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

    #[allow(clippy::too_many_arguments)]
    async fn send_email(
        &self,
        smtp_server: &str,
        smtp_port: u16,
        username: Option<&str>,
        password: Option<&str>,
        from_addr: &str,
        to_addrs: &[String],
        skip_tls_verify: bool,
        alert: &Alert,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let to_list = to_addrs
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let email = Message::builder()
            .from(from_addr.parse()?)
            .to(to_list.parse()?)
            .subject(format!("[{}] {}", alert.severity, alert.title))
            .header(ContentType::TEXT_PLAIN)
            .body(alert.message.clone())?;

        let mut builder =
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(smtp_server).port(smtp_port);

        // TLS when the port suggests it; verification can be skipped via config
        if smtp_port != 25 && smtp_port != 465 {
            let mut tls = TlsParameters::builder(smtp_server.to_string());
            if skip_tls_verify {
                tls = tls.dangerous_accept_invalid_certs(true);
            }
            builder = builder.tls(Tls::Required(tls.build()?));
        }

        if let (Some(user), Some(pass)) = (username, password) {
            builder = builder.credentials(Credentials::new(user.to_string(), pass.to_string()));
        }

        builder.build().send(email).await?;
        Ok(())
    }
}

fn log_console(alert: &Alert) {
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

/// Message to send a notification
#[derive(Message)]
#[rtype(result = "()")]
pub struct SendNotification {
    pub alert: Alert,
    /// Channel ids the rule requested; empty means all enabled channels
    pub channels: Vec<String>,
}

impl Handler<SendNotification> for AlertNotifier {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: SendNotification, _ctx: &mut Self::Context) -> Self::Result {
        let notifier = self.clone();
        let alert = msg.alert;
        let channels = msg.channels;

        Box::pin(wrap_future(async move {
            notifier.send_alert(&alert, &channels).await;
        }))
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
    use std::collections::HashMap;

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
        ]);
        assert_eq!(notifier.resolve_channels(&[]).len(), 1);
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
    fn test_email_channel_config_serialization() {
        let channel = NotificationChannel {
            id: "email".to_string(),
            name: "email".to_string(),
            channel_type: ChannelType::Email {
                smtp_server: "smtp.example.com".to_string(),
                smtp_port: 587,
                username: Some("user".to_string()),
                from_addr: "nimon@example.com".to_string(),
                to_addrs: vec!["ops@example.com".to_string()],
            },
            enabled: true,
            config: serde_json::json!({
                "smtp_password": "secret",
                "smtp_skip_tls_verify": false
            }),
        };
        let json = serde_json::to_string(&channel).unwrap();
        assert!(json.contains("\"smtp_port\":587"));
        let headers: Option<HashMap<String, String>> = None;
        assert!(headers.is_none());
    }
}
