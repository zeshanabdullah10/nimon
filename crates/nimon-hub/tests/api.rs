//! HTTP route tests: the router is driven with `tower::ServiceExt::oneshot`
//! against an in-memory database and live actors.

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tower::ServiceExt;

use nimon_core::actor::messages::{ConfigUpdate, DeviceStatusUpdate, ThresholdConfig};
use nimon_core::alert::{Alert, AlertStatus, Severity};
use nimon_core::db::device_repo::DeviceRepository;
use nimon_core::db::AlertRepository;
use nimon_core::protocol::{WsMessage, WsMessageType};
use nimon_core::{EdgeNode, EdgeStatus, HealthStatus, MetricValue};
use nimon_hub::config::{AuthConfig, HubConfig};
use nimon_hub::session::EdgeSession;
use nimon_hub::HubState;

struct TestHub {
    state: HubState,
    app: Router,
    pool: sqlx::SqlitePool,
}

async fn seed_edge(pool: &sqlx::SqlitePool, id: &str) {
    nimon_core::db::edge_repo::EdgeRepository::new(pool)
        .upsert(&EdgeNode {
            id: id.to_string(),
            name: format!("Edge {}", id),
            hostname: None,
            ip_address: None,
            last_seen: Some(chrono::Utc::now()),
            status: EdgeStatus::Offline,
        })
        .await
        .unwrap();
}

async fn hub_with(config: HubConfig, auth: AuthConfig) -> TestHub {
    let pool = nimon_core::db::connect(":memory:").await.unwrap();
    seed_edge(&pool, "edge-1").await;
    let state = nimon_hub::start_hub_services(config, pool.clone()).with_auth(auth);
    let app = nimon_hub::build_router(state.clone());
    TestHub { state, app, pool }
}

async fn hub() -> TestHub {
    hub_with(HubConfig::default(), AuthConfig::default()).await
}

impl TestHub {
    async fn request(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (status, _, body) = self.raw(method, uri, body, &[]).await;
        let json = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        (status, json)
    }

    async fn raw(
        &self,
        method: Method,
        uri: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut builder = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        let request = match body {
            Some(b) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, headers, bytes.to_vec())
    }

    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.request(Method::GET, uri, None).await
    }

    async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.request(Method::POST, uri, Some(body)).await
    }

    async fn flush(&self) {
        if let Some(am) = self.state.alert_manager() {
            am.send(nimon_hub::alert::manager::FlushMetrics)
                .await
                .unwrap();
        }
        self.state.db_writer().unwrap().flush().await;
    }
}

fn hot_status(device: &str, temp: f64) -> DeviceStatusUpdate {
    DeviceStatusUpdate {
        device_id: device.to_string(),
        edge_id: "edge-1".to_string(),
        status: HealthStatus::Healthy,
        metrics: [("temperature".to_string(), MetricValue::Float(temp))].into(),
        timestamp: chrono::Utc::now(),
        is_simulated: false,
    }
}

fn connect_session(state: &HubState, edge_id: &str, version: &str) -> mpsc::Receiver<WsMessage> {
    let (tx, rx) = mpsc::channel(16);
    state.sessions().add(EdgeSession::for_connection(
        edge_id.to_string(),
        format!("Edge {}", edge_id),
        None,
        None,
        version.to_string(),
        tx,
        Default::default(),
    ));
    rx
}

#[actix::test]
async fn health_reports_components() {
    let hub = hub().await;
    let (status, body) = hub.get("/health").await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    assert_eq!(body["status"], "healthy");
    assert_eq!(body["db_ok"], true);
    assert_eq!(body["db_writer_ok"], true);
    assert_eq!(body["db_writes_shed"], 0);
    assert_eq!(body["alert_manager_ok"], true);
    assert_eq!(body["edges_connected"], 0);
    assert!(body["uptime_secs"].is_u64());
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));

    // No DB / no alert manager: degraded, 503
    let bare = nimon_hub::build_router(HubState::new());
    let response = bare
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[actix::test]
async fn health_is_degraded_while_database_writes_time_out() {
    use nimon_hub::db_writer::{DbWriter, DbWriterOptions};
    let pool = nimon_core::db::connect(":memory:").await.unwrap();
    let writer = DbWriter::spawn_with(
        pool.clone(),
        DbWriterOptions {
            op_timeout: Duration::from_millis(50),
            ..Default::default()
        },
    );
    let state = HubState::with_config(HubConfig::default(), Some(pool), Some(writer.clone()));
    let am = nimon_hub::alert::manager::AlertManager::new(Default::default())
        .with_db_writer(writer.clone());
    state.set_alert_manager(actix::Actor::start(am));
    let app = nimon_hub::build_router(state);
    let health = |app: Router| async move {
        let response = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice::<Value>(&body).unwrap())
    };
    let (status, body) = health(app.clone()).await;
    assert_eq!(status, StatusCode::OK, "{}", body);

    // A write stuck behind an external lock: reads still work, writes don't
    let _ = writer
        .call(|_| async { tokio::time::sleep(Duration::from_secs(10)).await })
        .await;
    let (status, body) = health(app).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], "degraded");
    assert_eq!(body["db_ok"], true);
    assert_eq!(body["db_writer_ok"], false);
}

#[actix::test]
async fn alerts_unavailable_without_alert_manager() {
    let bare = nimon_hub::build_router(HubState::new());
    let response = bare
        .oneshot(Request::get("/api/v1/alerts").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["error"].is_string());
}

#[actix::test]
async fn auth_off_allows_writes() {
    let hub = hub().await;
    let (status, body) = hub.post("/api/v1/alerts/nope/acknowledge", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{}", body);
    let (_, settings) = hub.get("/api/v1/settings").await;
    assert_eq!(settings["auth"]["writes_require_token"], false);
}

#[actix::test]
async fn auth_on_requires_bearer_for_writes_only() {
    let hub = hub_with(
        HubConfig::default(),
        AuthConfig {
            api_token: Some("s3cret".to_string()),
            edge_token: Some("edge-secret".to_string()),
        },
    )
    .await;

    // Writes without / with a wrong token: 401 JSON
    let (status, body) = hub.post("/api/v1/alerts/nope/resolve", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["error"].is_string());
    let (status, _, _) = hub
        .raw(
            Method::POST,
            "/api/v1/alerts/nope/resolve",
            Some(json!({})),
            &[("authorization", "Bearer wrong")],
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Right token: reaches the handler (unknown alert)
    let (status, _, _) = hub
        .raw(
            Method::POST,
            "/api/v1/alerts/nope/resolve",
            Some(json!({})),
            &[("authorization", "Bearer s3cret")],
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Reads stay open
    let (status, settings) = hub.get("/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["auth"]["writes_require_token"], true);
    assert_eq!(hub.get("/api/v1/alerts").await.0, StatusCode::OK);

    // WebSocket: token required (header or query)
    let (status, _, _) = hub.raw(Method::GET, "/ws", None, &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = hub
        .raw(Method::GET, "/ws?token=edge-secret", None, &[])
        .await;
    assert_ne!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = hub
        .raw(
            Method::GET,
            "/ws",
            None,
            &[("authorization", "Bearer edge-secret")],
        )
        .await;
    assert_ne!(status, StatusCode::UNAUTHORIZED);
}

#[actix::test]
async fn cors_disabled_by_default_and_configurable() {
    let hub = hub().await;
    let (_, headers, _) = hub
        .raw(
            Method::GET,
            "/api/v1/settings",
            None,
            &[("origin", "https://evil.example.com")],
        )
        .await;
    assert!(headers.get("access-control-allow-origin").is_none());

    let config = HubConfig {
        cors_allowed_origins: vec!["https://ops.example.com".to_string()],
        ..Default::default()
    };
    let hub = hub_with(config, AuthConfig::default()).await;
    let (_, headers, _) = hub
        .raw(
            Method::GET,
            "/api/v1/settings",
            None,
            &[("origin", "https://ops.example.com")],
        )
        .await;
    assert_eq!(
        headers.get("access-control-allow-origin").unwrap(),
        "https://ops.example.com"
    );
}

#[actix::test]
async fn acknowledge_keeps_alert_active_resolve_removes_it() {
    let hub = hub().await;
    let am = hub.state.alert_manager().unwrap();
    // Default rules: temperature >= 65 (and < 75) fires only "rule-high-temp"
    am.send(hot_status("edge-1:dev-a", 70.0)).await.unwrap();

    let (status, body) = hub.get("/api/v1/alerts").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    let id = body["alerts"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(body["alerts"][0]["status"], "firing");
    assert!(body["alerts"][0]["last_fired_at"].is_string());
    assert!(body["alerts"][0]["acknowledged_at"].is_null());

    let (status, body) = hub
        .post(&format!("/api/v1/alerts/{}/acknowledge", id), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "status": "acknowledged" }));

    let (_, body) = hub.get("/api/v1/alerts").await;
    assert_eq!(body["total"], 1, "acknowledged alerts stay active");
    assert_eq!(body["alerts"][0]["status"], "acknowledged");
    assert!(body["alerts"][0]["acknowledged_at"].is_string());
    hub.flush().await;
    let record = AlertRepository::new(&hub.pool).get(&id).await.unwrap();
    assert_eq!(record.status(), AlertStatus::Acknowledged);

    let (status, body) = hub
        .post(&format!("/api/v1/alerts/{}/resolve", id), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "status": "resolved" }));
    let (_, body) = hub.get("/api/v1/alerts").await;
    assert_eq!(body["total"], 0);

    // Acknowledging a resolved alert conflicts; unknown ids are 404
    let (status, _) = hub
        .post(&format!("/api/v1/alerts/{}/acknowledge", id), json!({}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        hub.post("/api/v1/alerts/nope/resolve", json!({})).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        hub.post("/api/v1/alerts/nope/acknowledge", json!({}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[actix::test]
async fn active_alerts_sorted_by_severity_then_time() {
    let hub = hub().await;
    let am = hub.state.alert_manager().unwrap();
    am.send(hot_status("edge-1:warm", 70.0)).await.unwrap(); // warning
    am.send(hot_status("edge-1:hot", 90.0)).await.unwrap(); // warning + critical
    let (_, body) = hub.get("/api/v1/alerts").await;
    let severities: Vec<&str> = body["alerts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["severity"].as_str().unwrap())
        .collect();
    assert_eq!(severities, vec!["critical", "warning", "warning"]);
    // Same severity: newest first
    assert_eq!(body["alerts"][1]["device_id"], "edge-1:hot");
}

#[actix::test]
async fn alert_history_filters_before_limit() {
    let hub = hub().await;
    let repo = AlertRepository::new(&hub.pool);
    let base = Alert {
        id: String::new(),
        rule_id: "r".to_string(),
        edge_id: "edge-1".to_string(),
        device_id: String::new(),
        severity: Severity::Info,
        status: AlertStatus::Resolved,
        title: "t".to_string(),
        message: "m".to_string(),
        metric_name: None,
        metric_value: None,
        threshold: None,
        triggered_at: chrono::Utc::now(),
        resolved_at: None,
        fired_count: 1,
        notification_sent: false,
    };
    let t0 = chrono::Utc::now() - chrono::Duration::hours(5);
    for i in 0..10 {
        repo.insert(&Alert {
            id: format!("INFO{:02}", i),
            triggered_at: t0 + chrono::Duration::hours(2) + chrono::Duration::minutes(i),
            ..base.clone()
        })
        .await
        .unwrap();
    }
    for i in 0..2 {
        repo.insert(&Alert {
            id: format!("CRIT{:02}", i),
            severity: Severity::Critical,
            status: AlertStatus::Firing,
            triggered_at: t0 + chrono::Duration::minutes(i),
            ..base.clone()
        })
        .await
        .unwrap();
    }

    let (status, body) = hub
        .get("/api/v1/alerts/history?severity=critical&limit=5")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 2, "filters apply before LIMIT: {}", body);
    assert_eq!(body["alerts"][0]["id"], "CRIT01", "newest first");

    let (_, body) = hub.get("/api/v1/alerts/history?limit=3").await;
    assert_eq!(body["total"], 3);
    let (_, body) = hub.get("/api/v1/alerts/history?status=firing").await;
    assert_eq!(body["total"], 2);
    let (_, body) = hub.get("/api/v1/alerts/history?edge_id=edge-1").await;
    assert_eq!(body["total"], 12);

    let since = (t0 + chrono::Duration::hours(2)).to_rfc3339();
    let until = (t0 + chrono::Duration::hours(2) + chrono::Duration::minutes(4)).to_rfc3339();
    let uri = format!(
        "/api/v1/alerts/history?since={}&until={}",
        urlencode(&since),
        urlencode(&until)
    );
    let (_, body) = hub.get(&uri).await;
    assert_eq!(body["total"], 5, "{}", body);

    assert_eq!(
        hub.get("/api/v1/alerts/history?severity=extreme").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        hub.get("/api/v1/alerts/history?since=yesterday").await.0,
        StatusCode::BAD_REQUEST
    );
}

fn urlencode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            other => {
                let mut buf = [0u8; 4];
                other
                    .encode_utf8(&mut buf)
                    .bytes()
                    .map(|b| format!("%{:02X}", b))
                    .collect()
            }
        })
        .collect()
}

#[actix::test]
async fn settings_expose_thresholds() {
    let config: HubConfig = serde_yaml::from_str(
        "edges:\n  defaults:\n    temperature_warning: 60\n  overrides:\n    - edge_id: edge-7\n      temperature_critical: 88\n      poll_interval_secs: 5\nalert:\n  edge_offline_after_secs: 120\n",
    )
    .unwrap();
    let hub = hub_with(config, AuthConfig::default()).await;
    let (status, body) = hub.get("/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["thresholds"]["temperature_warning"], 60.0);
    assert_eq!(body["thresholds"]["temperature_critical"], 75.0);
    assert_eq!(body["prediction_alert_threshold"], 0.8);
    assert_eq!(body["edge_offline_after_secs"], 120);
    assert_eq!(
        body["edge_thresholds"]["edge-7"]["temperature_critical"],
        88.0
    );
    assert_eq!(body["edge_thresholds"]["edge-7"]["poll_interval_secs"], 5);
    // Known (DB) edge with defaults
    assert_eq!(
        body["edge_thresholds"]["edge-1"]["temperature_warning"],
        60.0
    );
    assert!(body["edge_thresholds"]["edge-1"]["poll_interval_secs"].is_null());
}

#[actix::test]
async fn devices_merge_live_and_offline() {
    let hub = hub().await;
    seed_edge(&hub.pool, "edge-2").await;
    let devices = DeviceRepository::new(&hub.pool);
    devices
        .upsert_snapshot("edge-2:old-dev", "edge-2")
        .await
        .unwrap();
    devices
        .upsert_status(&nimon_core::DeviceStatus {
            device_id: "edge-2:old-dev".to_string(),
            status: HealthStatus::Warning,
            last_poll: chrono::Utc::now() - chrono::Duration::hours(1),
            metrics: [("temperature".to_string(), MetricValue::Float(40.0))].into(),
            error_message: None,
            error_count: 0,
            uptime_seconds: 0,
        })
        .await
        .unwrap();

    let _rx = connect_session(&hub.state, "edge-1", "1.1");
    let session = hub.state.sessions().get("edge-1").unwrap();
    session
        .update_device(
            "edge-1:PXIe-6368#2".to_string(),
            HealthStatus::Healthy,
            [
                ("temperature".to_string(), MetricValue::Float(41.0)),
                (
                    "product".to_string(),
                    MetricValue::String("PXIe-6368".into()),
                ),
            ]
            .into(),
            true,
        )
        .await;

    let (status, body) = hub.get("/api/v1/devices").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 2, "{}", body);
    let live = &body["devices"][0];
    assert_eq!(live["device_id"], "edge-1:PXIe-6368#2");
    assert_eq!(live["edge_id"], "edge-1");
    assert_eq!(live["live"], true);
    assert_eq!(live["is_simulated"], true);
    assert_eq!(live["is_reachable"], true);
    assert_eq!(live["status"], "healthy");
    assert_eq!(live["model"], "PXIe-6368");
    assert!(live["last_seen"].is_string());
    let offline = &body["devices"][1];
    assert_eq!(offline["device_id"], "edge-2:old-dev");
    assert_eq!(offline["live"], false);
    assert_eq!(offline["is_reachable"], false);
    assert_eq!(offline["status"], "warning");
    assert!(
        offline["last_seen"].is_string(),
        "one timestamp field: last_seen"
    );
    assert!(offline.get("last_poll").is_none());

    let (_, body) = hub.get("/api/v1/devices?edge_id=edge-2").await;
    assert_eq!(body["total"], 1);

    // Per-edge endpoint: same shape, live flag
    let (status, body) = hub.get("/api/v1/edges/edge-2/devices").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["live"], false);
    assert_eq!(body["devices"][0]["last_seen"], offline["last_seen"]);
    let (_, body) = hub.get("/api/v1/edges/edge-1/devices").await;
    assert_eq!(body["live"], true);
    assert_eq!(
        hub.get("/api/v1/edges/nope/devices").await.0,
        StatusCode::NOT_FOUND
    );

    // Edges list: live + offline, both with last_seen and live flag
    let (_, body) = hub.get("/api/v1/edges").await;
    assert_eq!(body["total"], 2);
    assert_eq!(body["edges"][0]["live"], true);
    assert_eq!(body["edges"][1]["live"], false);
    assert_eq!(body["edges"][1]["device_count"], 1);
    assert_eq!(hub.get("/api/v1/edges/edge-2").await.0, StatusCode::OK);
    assert_eq!(hub.get("/api/v1/edges/nope").await.0, StatusCode::NOT_FOUND);
}

#[actix::test]
async fn device_metrics_with_encoded_id() {
    let hub = hub().await;
    let device_id = "edge-1:PXIe-6368#2";
    let repo = DeviceRepository::new(&hub.pool);
    repo.upsert_snapshot(device_id, "edge-1").await.unwrap();
    let t0 = chrono::Utc::now() - chrono::Duration::minutes(10);
    let rows: Vec<(String, String, f64, chrono::DateTime<chrono::Utc>)> = (0..3)
        .map(|i| {
            (
                device_id.to_string(),
                "temperature".to_string(),
                40.0 + i as f64,
                t0 + chrono::Duration::minutes(i),
            )
        })
        .collect();
    repo.insert_metrics_batch(&rows).await.unwrap();

    let (status, body) = hub
        .get("/api/v1/devices/edge-1%3APXIe-6368%232/metrics?metric=temperature")
        .await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    assert_eq!(
        body["device_id"], device_id,
        "path segment is percent-decoded"
    );
    assert_eq!(body["metric"], "temperature");
    let points = body["points"].as_array().unwrap();
    assert_eq!(points.len(), 3);
    assert_eq!(points[0]["value"], 40.0, "oldest first");
    assert!(points[0]["timestamp"].is_string());

    let (_, body) = hub
        .get("/api/v1/devices/edge-1%3APXIe-6368%232/metrics?limit=2")
        .await;
    assert_eq!(body["points"].as_array().unwrap().len(), 2);
    assert_eq!(body["points"][1]["value"], 42.0, "newest points kept");

    // Status updates are sampled into the history via the batch writer
    let am = hub.state.alert_manager().unwrap();
    am.send(hot_status("edge-1:sampled", 33.0)).await.unwrap();
    hub.flush().await;
    let (_, body) = hub.get("/api/v1/devices/edge-1%3Asampled/metrics").await;
    assert_eq!(body["points"].as_array().unwrap().len(), 1);
}

#[actix::test]
async fn manual_action_is_accepted_and_recorded() {
    let hub = hub().await;
    DeviceRepository::new(&hub.pool)
        .upsert_snapshot("edge-1:daq-9", "edge-1")
        .await
        .unwrap();

    let (status, body) = hub
        .post(
            "/api/v1/devices/edge-1%3Adaq-9/actions",
            json!({ "action_type": "reset_driver" }),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{}", body);
    let action_id = body["action_id"].as_str().unwrap().to_string();
    assert!(action_id.starts_with("manual-"));

    // Edge is offline: the executor fails fast and records history
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let (_, body) = hub.get("/api/v1/actions?device_id=edge-1%3Adaq-9").await;
        if body["total"] == 1 {
            assert_eq!(body["actions"][0]["action_id"], action_id.as_str());
            assert_eq!(body["actions"][0]["success"], false);
            break;
        }
        assert!(std::time::Instant::now() < deadline, "action not recorded");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (_, body) = hub.get("/api/v1/actions?edge_id=edge-1").await;
    assert_eq!(body["total"], 1);

    assert_eq!(
        hub.post(
            "/api/v1/devices/edge-1%3Adaq-9/actions",
            json!({ "action_type": "format_disk" })
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        hub.post(
            "/api/v1/devices/unknown/actions",
            json!({ "action_type": "power_cycle" })
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

fn config_update(msg: WsMessage) -> ConfigUpdate {
    assert_eq!(msg.msg_type, WsMessageType::ConfigUpdate);
    msg.into_payload().unwrap()
}

#[actix::test]
async fn edge_config_partial_push_and_store() {
    let hub = hub().await;
    let mut rx = connect_session(&hub.state, "edge-1", "1.1");

    // 1.1 edge: only the provided field is sent
    let (status, body) = hub
        .post(
            "/api/v1/edges/edge-1/config",
            json!({ "temperature_critical": 90.0 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "status": "pushed" }));
    let update = config_update(rx.try_recv().unwrap());
    assert_eq!(update.thresholds.temperature_critical, Some(90.0));
    assert_eq!(update.thresholds.temperature_warning, None);
    assert_eq!(update.poll_interval_secs, None);

    // 1.0 edge: full thresholds, resolved against the stored override
    let mut legacy_rx = connect_session(&hub.state, "edge-old", "1.0");
    hub.post(
        "/api/v1/edges/edge-old/config",
        json!({ "temperature_critical": 90.0 }),
    )
    .await;
    let _ = legacy_rx.try_recv();
    let (status, _) = hub
        .post(
            "/api/v1/edges/edge-old/config",
            json!({ "temperature_warning": 70.0, "poll_interval_secs": 10 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let update = config_update(legacy_rx.try_recv().unwrap());
    assert_eq!(update.thresholds, ThresholdConfig::full(70.0, 90.0));
    assert_eq!(update.poll_interval_secs, Some(10));

    // Offline edge: stored (202) and reported by settings
    let (status, body) = hub
        .post(
            "/api/v1/edges/edge-3/config",
            json!({ "poll_interval_secs": 15 }),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body, json!({ "status": "stored" }));
    let (_, settings) = hub.get("/api/v1/settings").await;
    assert_eq!(
        settings["edge_thresholds"]["edge-3"]["poll_interval_secs"],
        15
    );
    assert_eq!(
        settings["edge_thresholds"]["edge-1"]["temperature_critical"],
        90.0
    );
    assert_eq!(hub.state.desired_for("edge-3").poll_interval_secs, Some(15));

    // Validation
    assert_eq!(
        hub.post("/api/v1/edges/edge-1/config", json!({})).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        hub.post(
            "/api/v1/edges/edge-1/config",
            json!({ "temperature_warning": 95.0 })
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "warning must stay below the stored critical (90)"
    );
    let (status, _, _) = hub
        .raw(
            Method::POST,
            "/api/v1/edges/edge-1/config",
            None,
            &[("content-type", "application/json")],
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[actix::test]
async fn dashboard_is_compressed_and_cacheable() {
    let hub = hub().await;
    let (status, headers, body) = hub
        .raw(Method::GET, "/", None, &[("accept-encoding", "gzip")])
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    assert_eq!(headers["content-encoding"], "gzip");
    assert_eq!(headers["cache-control"], "no-cache");
    assert!(!body.is_empty());
    let etag = headers["etag"].to_str().unwrap().to_string();

    let (status, _, _) = hub
        .raw(Method::GET, "/", None, &[("if-none-match", etag.as_str())])
        .await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);

    // Legacy static routes are gone
    assert_eq!(hub.get("/app.js").await.0, StatusCode::NOT_FOUND);
    assert_eq!(hub.get("/style.css").await.0, StatusCode::NOT_FOUND);
}
