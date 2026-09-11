use anyhow::Result;
use serde::Deserialize;

pub struct Client {
    base_url: String,
    client: reqwest::Client,
}

impl Client {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn health(&self) -> Result<HealthResponse> {
        Ok(self
            .client
            .get(format!("{}/health", self.base_url))
            .send()
            .await?
            .json()
            .await?)
    }

    pub async fn edges(&self) -> Result<EdgesResponse> {
        Ok(self
            .client
            .get(format!("{}/api/v1/edges", self.base_url))
            .send()
            .await?
            .json()
            .await?)
    }

    pub async fn edge(&self, edge_id: &str) -> Result<EdgeDetailResponse> {
        Ok(self
            .client
            .get(format!("{}/api/v1/edges/{}", self.base_url, edge_id))
            .send()
            .await?
            .json()
            .await?)
    }

    pub async fn alerts(&self) -> Result<AlertsResponse> {
        Ok(self
            .client
            .get(format!("{}/api/v1/alerts", self.base_url))
            .send()
            .await?
            .json()
            .await?)
    }

    pub async fn ack_alert(&self, alert_id: &str) -> Result<()> {
        let res = self
            .client
            .post(format!(
                "{}/api/v1/alerts/{}/acknowledge",
                self.base_url, alert_id
            ))
            .send()
            .await?;
        if !res.status().is_success() {
            anyhow::bail!("acknowledge failed: HTTP {}", res.status());
        }
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub connected_edges: usize,
}

#[derive(Deserialize)]
pub struct EdgesResponse {
    pub edges: Vec<EdgeInfo>,
    pub total: usize,
}

// Response DTOs mirror the hub API payload exactly; some fields exist for
// deserialization completeness even where the CLI does not print them.
#[allow(dead_code)]
#[derive(Deserialize)]
pub struct EdgeInfo {
    pub edge_id: String,
    pub name: String,
    pub connected_at: Option<String>,
    pub last_seen: Option<String>,
    pub device_count: Option<usize>,
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct EdgeDetailResponse {
    pub edge_id: String,
    pub name: String,
    pub hostname: Option<String>,
    pub ip_address: Option<String>,
    pub connected_at: Option<String>,
    pub last_seen: Option<String>,
    pub device_count: Option<usize>,
    pub status: String,
}

#[allow(dead_code)]
#[derive(Deserialize)]
pub struct AlertsResponse {
    pub alerts: Vec<AlertInfo>,
    pub total: usize,
}

#[allow(dead_code)]
#[derive(Deserialize)]
pub struct AlertInfo {
    pub id: String,
    pub device_id: Option<String>,
    pub edge_id: Option<String>,
    pub severity: String,
    pub title: String,
    pub message: String,
    pub status: String,
    pub triggered_at: String,
}
