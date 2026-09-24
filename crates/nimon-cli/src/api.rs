//! Thin HTTP client for the hub REST API.
//!
//! Every call returns the raw JSON body (so `--json` can print it
//! verbatim); callers deserialize into the DTOs below for table output.
//! Non-2xx responses become an [`ApiError`] carrying the hub's
//! `{"error": ...}` message.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;

/// Default hub base URL (overridden by `--hub` / `NIMON_HUB`).
pub const DEFAULT_HUB_URL: &str = "http://127.0.0.1:9090";

/// A non-success HTTP response from the hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    /// The hub's `error` message, or the raw body / reason phrase
    pub message: String,
}

impl ApiError {
    /// Build from a status code and response body (JSON `{"error": ...}`
    /// preferred, raw text otherwise).
    pub fn from_body(status: u16, body: &str) -> Self {
        let message = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| {
                let trimmed = body.trim();
                if trimmed.is_empty() {
                    StatusCode::from_u16(status)
                        .ok()
                        .and_then(|s| s.canonical_reason())
                        .unwrap_or("request failed")
                        .to_string()
                } else {
                    trimmed.chars().take(300).collect()
                }
            });
        Self { status, message }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            404 => write!(f, "not found: {}", self.message),
            401 => write!(
                f,
                "unauthorized: {} (pass --token or set NIMON_API_TOKEN)",
                self.message
            ),
            status => {
                let reason = StatusCode::from_u16(status)
                    .ok()
                    .and_then(|s| s.canonical_reason())
                    .unwrap_or("");
                write!(
                    f,
                    "hub returned HTTP {} {}: {}",
                    status, reason, self.message
                )
            }
        }
    }
}

impl std::error::Error for ApiError {}

/// Percent-encode one URL path segment: everything except RFC 3986
/// unreserved characters is escaped (device ids contain `:` and `#`).
pub fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{:02X}", other)),
        }
    }
    out
}

/// Build `/a/b/c` from raw segments, encoding each.
pub fn api_path(segments: &[&str]) -> String {
    segments
        .iter()
        .map(|s| format!("/{}", encode_segment(s)))
        .collect()
}

/// Normalize a hub base URL: add `http://` when no scheme is given and
/// strip trailing slashes.
pub fn normalize_base_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{}", trimmed)
    }
}

pub struct Client {
    base_url: String,
    token: Option<String>,
    timeout: Duration,
    client: reqwest::Client,
}

impl Client {
    pub fn new(base_url: &str, token: Option<String>, timeout: Duration) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("nimon-cli/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self {
            base_url: normalize_base_url(base_url),
            token: token.filter(|t| !t.trim().is_empty()),
            timeout,
            client,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn transport_error(&self, url: &str, e: reqwest::Error) -> anyhow::Error {
        if e.is_timeout() {
            anyhow::anyhow!(
                "request to {} timed out after {}s",
                url,
                self.timeout.as_secs()
            )
        } else if e.is_connect() {
            anyhow::anyhow!(
                "cannot reach hub at {} (is it running? set --hub or NIMON_HUB): {}",
                self.base_url,
                e
            )
        } else {
            anyhow::anyhow!("request to {} failed: {}", url, e)
        }
    }

    /// Send a request and return (status, parsed JSON body).
    async fn send(&self, request: reqwest::RequestBuilder, url: &str) -> Result<(u16, Value)> {
        let response = request
            .send()
            .await
            .map_err(|e| self.transport_error(url, e))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| self.transport_error(url, e))?;
        if !(200..300).contains(&status) {
            return Err(ApiError::from_body(status, &body).into());
        }
        if body.trim().is_empty() {
            return Ok((status, Value::Null));
        }
        let json = serde_json::from_str(&body)
            .with_context(|| format!("hub returned invalid JSON from {}", url))?;
        Ok((status, json))
    }

    /// `GET path?query` -> JSON (non-2xx is an error).
    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let request = self.client.get(&url).query(query);
        Ok(self.send(request, &url).await?.1)
    }

    /// `POST path` with a JSON body (bearer token attached when set).
    pub async fn post(&self, path: &str, body: &Value) -> Result<(u16, Value)> {
        let url = format!("{}{}", self.base_url, path);
        let mut request = self.client.post(&url).json(body);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        self.send(request, &url).await
    }

    /// `GET /health`: a degraded hub answers 503 with the same body, which
    /// is still a valid health report.
    pub async fn health(&self) -> Result<Value> {
        let url = format!("{}/health", self.base_url);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| self.transport_error(&url, e))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| self.transport_error(&url, e))?;
        match serde_json::from_str::<Value>(&body) {
            Ok(v) if v.get("status").is_some() => Ok(v),
            _ => Err(ApiError::from_body(status, &body).into()),
        }
    }
}

// ----------------------------------------------------------------------
// Response DTOs (lenient: unknown fields ignored, most fields optional)
// ----------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct Health {
    pub status: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub db_ok: Option<bool>,
    #[serde(default)]
    pub alert_manager_ok: Option<bool>,
    #[serde(default)]
    pub edges_connected: Option<u64>,
    #[serde(default)]
    pub uptime_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct EdgeList {
    pub edges: Vec<Edge>,
}

#[derive(Debug, Deserialize)]
pub struct Edge {
    pub edge_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub ip_address: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub live: bool,
    #[serde(default)]
    pub connected_at: Option<String>,
    #[serde(default)]
    pub last_seen: Option<String>,
    #[serde(default)]
    pub device_count: Option<u64>,
    #[serde(default)]
    pub protocol_version: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub uptime_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct DeviceList {
    pub devices: Vec<Device>,
    #[serde(default)]
    pub live: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct Device {
    pub device_id: String,
    #[serde(default)]
    pub edge_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub slot: Option<i64>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub metrics: HashMap<String, Value>,
    #[serde(default)]
    pub last_seen: Option<String>,
    #[serde(default)]
    pub is_simulated: bool,
    #[serde(default)]
    pub is_reachable: bool,
    #[serde(default)]
    pub live: bool,
}

#[derive(Debug, Deserialize)]
pub struct MetricSeries {
    pub device_id: String,
    pub metric: String,
    pub points: Vec<MetricPoint>,
}

#[derive(Debug, Deserialize)]
pub struct MetricPoint {
    pub timestamp: String,
    pub value: f64,
}

#[derive(Debug, Deserialize)]
pub struct AlertList {
    pub alerts: Vec<Alert>,
}

#[derive(Debug, Deserialize)]
pub struct Alert {
    pub id: String,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub edge_id: Option<String>,
    pub severity: String,
    pub status: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub triggered_at: Option<String>,
    #[serde(default)]
    pub resolved_at: Option<String>,
    #[serde(default)]
    pub fired_count: Option<i64>,
}

impl Alert {
    /// Device id, else edge id (edge-level alerts carry an empty device)
    pub fn target(&self) -> &str {
        match self.device_id.as_deref() {
            Some(d) if !d.is_empty() => d,
            _ => self.edge_id.as_deref().unwrap_or("-"),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct PredictionList {
    pub predictions: Vec<Prediction>,
}

#[derive(Debug, Deserialize)]
pub struct Prediction {
    pub device_id: String,
    #[serde(default)]
    pub edge_id: Option<String>,
    #[serde(default)]
    pub prediction_type: Value,
    #[serde(default)]
    pub probability: f64,
    #[serde(default)]
    pub eta_minutes: Option<i64>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ActionList {
    pub actions: Vec<ActionRecord>,
}

#[derive(Debug, Deserialize)]
pub struct ActionRecord {
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub action_id: String,
    #[serde(default)]
    pub action_type: String,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub success: Option<bool>,
    #[serde(default)]
    pub executed_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub version: Option<String>,
    pub thresholds: Thresholds,
    #[serde(default)]
    pub edge_thresholds: std::collections::BTreeMap<String, Thresholds>,
    #[serde(default)]
    pub prediction_alert_threshold: Option<f64>,
    #[serde(default)]
    pub edge_offline_after_secs: Option<u64>,
    #[serde(default)]
    pub auth: Option<SettingsAuth>,
}

#[derive(Debug, Deserialize)]
pub struct Thresholds {
    #[serde(default)]
    pub temperature_warning: Option<f64>,
    #[serde(default)]
    pub temperature_critical: Option<f64>,
    #[serde(default)]
    pub poll_interval_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct SettingsAuth {
    #[serde(default)]
    pub writes_require_token: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_device_ids() {
        assert_eq!(
            encode_segment("edge-1:PXIe-6368#2"),
            "edge-1%3APXIe-6368%232"
        );
        assert_eq!(encode_segment("a b/c?d"), "a%20b%2Fc%3Fd");
        assert_eq!(encode_segment("plain_id.1~"), "plain_id.1~");
        // Multi-byte UTF-8 is encoded byte by byte
        assert_eq!(encode_segment("é"), "%C3%A9");
        assert_eq!(
            api_path(&["api", "v1", "devices", "e:dev#1", "metrics"]),
            "/api/v1/devices/e%3Adev%231/metrics"
        );
    }

    #[test]
    fn normalizes_base_url() {
        assert_eq!(normalize_base_url("http://h:9090/"), "http://h:9090");
        assert_eq!(
            normalize_base_url("hub.local:9090"),
            "http://hub.local:9090"
        );
        assert_eq!(normalize_base_url(" https://hub "), "https://hub");
    }

    #[test]
    fn api_error_uses_hub_message() {
        let e = ApiError::from_body(404, r#"{"error":"alert 'x' not found"}"#);
        assert_eq!(e.to_string(), "not found: alert 'x' not found");

        let e = ApiError::from_body(409, r#"{"error":"alert already resolved"}"#);
        assert_eq!(
            e.to_string(),
            "hub returned HTTP 409 Conflict: alert already resolved"
        );

        let e = ApiError::from_body(401, r#"{"error":"missing bearer token"}"#);
        assert!(e
            .to_string()
            .starts_with("unauthorized: missing bearer token"));
        assert!(e.to_string().contains("NIMON_API_TOKEN"));

        // Non-JSON / empty bodies fall back to text / reason phrase
        assert_eq!(
            ApiError::from_body(502, "bad gateway").message,
            "bad gateway"
        );
        assert_eq!(ApiError::from_body(404, "").message, "Not Found");
    }

    #[test]
    fn deserializes_hub_shapes() {
        let alerts: AlertList = serde_json::from_value(serde_json::json!({
            "alerts": [{
                "id": "01J", "rule_id": "builtin-edge-offline", "edge_id": "e1",
                "device_id": "", "severity": "warning", "status": "firing",
                "title": "Edge offline", "message": "m",
                "triggered_at": "2026-09-24T10:00:00Z", "resolved_at": null,
                "fired_count": 1, "notification_sent": false
            }],
            "total": 1
        }))
        .unwrap();
        assert_eq!(alerts.alerts[0].target(), "e1");

        let settings: Settings = serde_json::from_value(serde_json::json!({
            "version": "0.2.0",
            "thresholds": {"temperature_warning": 65.0, "temperature_critical": 75.0},
            "edge_thresholds": {"e1": {"temperature_warning": 65.0,
                "temperature_critical": 75.0, "poll_interval_secs": null}},
            "prediction_alert_threshold": 0.8,
            "edge_offline_after_secs": 90,
            "auth": {"writes_require_token": false}
        }))
        .unwrap();
        assert_eq!(settings.edge_thresholds["e1"].poll_interval_secs, None);
    }
}
