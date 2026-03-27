//! Alert notification sender

use actix::prelude::*;
use tracing::{error, info};

use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use lettre::transport::smtp::client::Tls;
use lettre::transport::smtp::client::TlsParameters;

use nimon_core::alert::{Alert, AlertSeverity, ChannelType, NotificationChannel};
use actix::fut::wrap_future;

/// Alert notifier actor
pub struct AlertNotifier {
    channels: Vec<NotificationChannel>,
}

impl AlertNotifier {
    pub fn new(channels: Vec<NotificationChannel>) -> Self {
        Self { channels }
    }

    async fn send_alert(&self, alert: &Alert) {
        for channel in &self.channels {
            if !channel.enabled {
                continue;
            }

            match &channel.channel_type {
                ChannelType::Console => {
                    info!(
                        "[ALERT {:?}] {} - {}: {}",
                        alert.severity, alert.device_id, alert.title, alert.message
                    );
                }
                ChannelType::Email { smtp_server, from_addr, to_addrs } => {
                    let skip_tls_verify = channel.config
                        .get("smtp_skip_tls_verify")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if let Err(e) = self.send_email(smtp_server, from_addr, to_addrs, skip_tls_verify, alert).await {
                        error!("Email notification failed for {}: {}", channel.id, e);
                    }
                }
                ChannelType::Webhook { url, .. } => {
                    if let Err(e) = self.send_webhook(url, alert).await {
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

    async fn send_webhook(&self, url: &str, alert: &Alert) -> Result<(), Box<dyn std::error::Error>> {
        let client = reqwest::Client::new();
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

        client.post(url)
            .json(&payload)
            .send()
            .await?;

        Ok(())
    }

    async fn send_slack(&self, webhook_url: &str, alert: &Alert) -> Result<(), Box<dyn std::error::Error>> {
        let client = reqwest::Client::new();
        let color = match alert.severity {
            AlertSeverity::Info => "#36a64f",
            AlertSeverity::Warning => "warning",
            AlertSeverity::Critical => "danger",
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

        client.post(webhook_url)
            .json(&payload)
            .send()
            .await?;

        Ok(())
    }

    async fn send_teams(&self, webhook_url: &str, alert: &Alert) -> Result<(), Box<dyn std::error::Error>> {
        let client = reqwest::Client::new();
        let color = match alert.severity {
            AlertSeverity::Info => "008000",
            AlertSeverity::Warning => "ff8c00",
            AlertSeverity::Critical => "ff0000",
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

        client.post(webhook_url)
            .json(&payload)
            .send()
            .await?;

        Ok(())
    }

    async fn send_email(&self, smtp_server: &str, from_addr: &str, to_addrs: &[String], skip_tls_verify: bool, alert: &Alert) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let email = Message::builder()
            .from(from_addr.parse()?)
            .to(to_addrs.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ").parse()?)
            .subject(format!("[{:?}] {}", alert.severity, alert.title))
            .body(alert.message.clone())?;

        let mailer: AsyncSmtpTransport<Tokio1Executor> = if skip_tls_verify {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(smtp_server).build()
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(smtp_server)
                .port(587)
                .tls(Tls::Required(TlsParameters::new(smtp_server.into())?))
                .build()
        };
        mailer.send(email).await?;

        Ok(())
    }
}

impl Actor for AlertNotifier {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        info!("AlertNotifier actor started");
    }
}

/// Message to send a notification
#[derive(Message)]
#[rtype(result = "()")]
pub struct SendNotification {
    pub alert: Alert,
}

impl Handler<SendNotification> for AlertNotifier {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: SendNotification, _ctx: &mut Self::Context) -> Self::Result {
        let notifier = self.clone();
        let alert = msg.alert;

        Box::pin(wrap_future(async move {
            notifier.send_alert(&alert).await;
        }))
    }
}

impl Clone for AlertNotifier {
    fn clone(&self) -> Self {
        Self {
            channels: self.channels.clone(),
        }
    }
}
