//! REST API handlers for the hub server.
//!
//! Errors are always JSON `{ "error": "<message>" }`.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use axum::{
    extract::{rejection::JsonRejection, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{debug, error, warn};

use nimon_core::alert::{AlertStatus, Severity};
use nimon_core::db::{
    ActionHistoryFilter, ActionRecord, ActionRepository, AlertHistoryFilter, AlertRecord,
    AlertRepository,
};
use nimon_core::protocol::WsMessage;
use nimon_core::{MetricValue, NimonError, DEFAULT_TEMP_CRITICAL_C, DEFAULT_TEMP_WARNING_C};

use crate::action::actions::{Action, ActionType};
use crate::action::executor::{ActionContext, ExecuteAction};
use crate::alert::manager::{
    AcknowledgeAlert, GetActiveAlertViews, GetRecentPredictions, Ping, ResolveAlert,
};
use crate::config::EdgeDesiredConfig;
use crate::queries::{self, DeviceStatusRow};
use crate::server::{desired_to_config_update, HubState};
use crate::session::DeviceSnapshot;

/// Default / maximum `limit` for list endpoints
const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 1000;
/// Default / maximum `limit` for metric points
const DEFAULT_METRIC_LIMIT: i64 = 500;
const MAX_METRIC_LIMIT: i64 = 10_000;
/// How long `/health` waits for the alert manager
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn json_error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

fn no_database() -> Response {
    json_error(StatusCode::SERVICE_UNAVAILABLE, "database not available")
}

fn db_error(e: impl std::fmt::Display) -> Response {
    error!("Database query failed: {}", e);
    json_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("database error: {}", e),
    )
}

/// `limit` query parameter (400 message on error).
fn parse_limit(params: &HashMap<String, String>, default: i64, max: i64) -> Result<i64, String> {
    match params.get("limit") {
        None => Ok(default),
        Some(v) => match v.parse::<i64>() {
            Ok(n) if n >= 1 => Ok(n.min(max)),
            _ => Err(format!(
                "invalid limit '{}': expected a positive integer",
                v
            )),
        },
    }
}

/// RFC3339 timestamp query parameter (400 message on error).
fn parse_time(
    params: &HashMap<String, String>,
    key: &str,
) -> Result<Option<DateTime<Utc>>, String> {
    match params.get(key).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) => queries::parse_ts(v)
            .map(Some)
            .ok_or_else(|| format!("invalid {} '{}': expected an RFC3339 timestamp", key, v)),
    }
}

fn bad_request(message: String) -> Response {
    json_error(StatusCode::BAD_REQUEST, message)
}

fn non_empty<'a>(params: &'a HashMap<String, String>, key: &str) -> Option<&'a String> {
    params.get(key).filter(|v| !v.is_empty())
}

// ----------------------------------------------------------------------
// Health / status / settings
// ----------------------------------------------------------------------

/// `GET /health`
pub async fn health_handler(State(state): State<HubState>) -> Response {
    let db_ok = match state.db_pool() {
        Some(pool) => sqlx::query("SELECT 1").execute(&pool).await.is_ok(),
        None => false,
    };
    let alert_manager_ok = match state.alert_manager() {
        Some(am) => matches!(
            tokio::time::timeout(HEALTH_PROBE_TIMEOUT, am.send(Ping)).await,
            Ok(Ok(()))
        ),
        None => false,
    };
    // Reads can succeed while writes are stuck (external write lock) or
    // the writer died: report the writer separately
    let writer = state.db_writer().map(|w| w.health());
    let db_writer_ok = writer.is_some_and(|h| h.is_ok());
    let healthy = db_ok && db_writer_ok && alert_manager_ok;
    let body = json!({
        "status": if healthy { "healthy" } else { "degraded" },
        "version": env!("CARGO_PKG_VERSION"),
        "db_ok": db_ok,
        "db_writer_ok": db_writer_ok,
        "db_writes_shed": writer.map(|h| h.shed),
        "alert_manager_ok": alert_manager_ok,
        "edges_connected": state.sessions().len(),
        "uptime_secs": state.uptime_secs(),
    });
    let status = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body)).into_response()
}

/// `GET /api/v1/status`: connected edges summary
pub async fn status_handler(State(state): State<HubState>) -> Json<Value> {
    let mut edges = Vec::new();
    for session in state.sessions().snapshot() {
        edges.push(json!({
            "edge_id": session.edge_id(),
            "name": session.name(),
            "connected_at": session.connected_at().to_rfc3339(),
            "device_count": session.device_count().await,
        }));
    }
    let total = edges.len();
    Json(json!({ "edges": edges, "total": total }))
}

fn thresholds_json(desired: &EdgeDesiredConfig) -> Value {
    json!({
        "temperature_warning": desired.temperature_warning.unwrap_or(DEFAULT_TEMP_WARNING_C),
        "temperature_critical": desired.temperature_critical.unwrap_or(DEFAULT_TEMP_CRITICAL_C),
        "poll_interval_secs": desired.poll_interval_secs,
    })
}

/// `GET /api/v1/settings`
pub async fn settings_handler(State(state): State<HubState>) -> Response {
    let config = state.config();
    let defaults = &config.edges.defaults;

    let mut edge_ids: Vec<String> = state.sessions().edge_ids();
    edge_ids.extend(config.edges.overrides.iter().map(|o| o.edge_id.clone()));
    edge_ids.extend(state.edges_with_overrides());
    if let Some(pool) = state.db_pool() {
        match nimon_core::db::edge_repo::EdgeRepository::new(&pool)
            .list()
            .await
        {
            Ok(edges) => edge_ids.extend(edges.into_iter().map(|e| e.id)),
            Err(e) => warn!("Failed to list edges for settings: {}", e),
        }
    }
    edge_ids.sort();
    edge_ids.dedup();

    let edge_thresholds: BTreeMap<String, Value> = edge_ids
        .into_iter()
        .map(|id| {
            let desired = state.desired_for(&id);
            (id, thresholds_json(&desired))
        })
        .collect();

    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "thresholds": {
            "temperature_warning": defaults.temperature_warning.unwrap_or(DEFAULT_TEMP_WARNING_C),
            "temperature_critical": defaults.temperature_critical.unwrap_or(DEFAULT_TEMP_CRITICAL_C),
        },
        "edge_thresholds": edge_thresholds,
        "prediction_alert_threshold": config.alert.prediction_alert_threshold,
        "edge_offline_after_secs": config.alert.edge_offline_after_secs,
        "auth": { "writes_require_token": state.auth().api_token.is_some() },
    }))
    .into_response()
}

// ----------------------------------------------------------------------
// Alerts
// ----------------------------------------------------------------------

/// `GET /api/v1/alerts`: active alerts (firing, pending, acknowledged)
pub async fn get_alerts_handler(State(state): State<HubState>) -> Response {
    let Some(am) = state.alert_manager() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "alert manager not available",
        );
    };
    match am.send(GetActiveAlertViews).await {
        Ok(alerts) => {
            let total = alerts.len();
            Json(json!({ "alerts": alerts, "total": total })).into_response()
        }
        Err(e) => {
            error!("Failed to query AlertManager: {}", e);
            json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("alert manager unavailable: {}", e),
            )
        }
    }
}

fn alert_record_json(r: &AlertRecord) -> Value {
    json!({
        "id": r.id,
        "rule_id": r.rule_name,
        "rule_name": r.rule_name,
        "device_id": r.device_id,
        "edge_id": r.edge_id,
        "severity": r.severity,
        "status": r.status,
        "title": r.title.clone().unwrap_or_else(|| r.rule_name.clone()),
        "message": r.message,
        "metric_name": r.metric_name,
        "metric_value": r.metric_value,
        "threshold": r.threshold,
        "fired_count": r.fired_count,
        "notification_sent": r.notification_sent != 0,
        "action_taken": r.action_taken,
        "action_result": r.action_result,
        "triggered_at": r.triggered_at,
        "created_at": r.created_at,
        "last_fired_at": r.last_fired_at,
        "acknowledged_at": r.acknowledged_at,
        "resolved_at": r.resolved_at,
    })
}

fn parse_alert_status(s: &str) -> Option<AlertStatus> {
    AlertStatus::ALL.into_iter().find(|v| v.as_str() == s)
}

/// `GET /api/v1/alerts/history?severity&edge_id&device_id&status&since&until&limit`
pub async fn get_alert_history_handler(
    State(state): State<HubState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db_pool() else {
        return no_database();
    };
    let severity = match non_empty(&params, "severity") {
        None => None,
        Some(s) => match Severity::parse(&s.to_lowercase()) {
            Ok(v) => Some(v),
            Err(e) => return json_error(StatusCode::BAD_REQUEST, e),
        },
    };
    let status = match non_empty(&params, "status") {
        None => None,
        Some(s) => match parse_alert_status(&s.to_lowercase()) {
            Some(v) => Some(v),
            None => return json_error(StatusCode::BAD_REQUEST, format!("unknown status '{}'", s)),
        },
    };
    let filter = AlertHistoryFilter {
        severity,
        edge_id: non_empty(&params, "edge_id").cloned(),
        device_id: non_empty(&params, "device_id").cloned(),
        status,
        since: match parse_time(&params, "since") {
            Ok(v) => v,
            Err(e) => return bad_request(e),
        },
        until: match parse_time(&params, "until") {
            Ok(v) => v,
            Err(e) => return bad_request(e),
        },
        limit: Some(match parse_limit(&params, DEFAULT_LIMIT, MAX_LIMIT) {
            Ok(v) => v,
            Err(e) => return bad_request(e),
        }),
    };
    match AlertRepository::new(&pool).list_history(&filter).await {
        Ok(records) => {
            let alerts: Vec<Value> = records.iter().map(alert_record_json).collect();
            let total = alerts.len();
            Json(json!({ "alerts": alerts, "total": total })).into_response()
        }
        Err(e) => db_error(e),
    }
}

fn alert_error(e: NimonError) -> Response {
    match e {
        NimonError::AlertNotFound(id) => {
            json_error(StatusCode::NOT_FOUND, format!("alert '{}' not found", id))
        }
        NimonError::InvalidState(msg) => json_error(StatusCode::CONFLICT, msg),
        other => json_error(StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
    }
}

/// `POST /api/v1/alerts/:alert_id/acknowledge`
pub async fn acknowledge_alert_handler(
    State(state): State<HubState>,
    Path(alert_id): Path<String>,
) -> Response {
    let Some(am) = state.alert_manager() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "alert manager not available",
        );
    };
    match am.send(AcknowledgeAlert { alert_id }).await {
        Ok(Ok(_)) => Json(json!({ "status": "acknowledged" })).into_response(),
        Ok(Err(e)) => alert_error(e),
        Err(e) => json_error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `POST /api/v1/alerts/:alert_id/resolve`
pub async fn resolve_alert_handler(
    State(state): State<HubState>,
    Path(alert_id): Path<String>,
) -> Response {
    let Some(am) = state.alert_manager() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "alert manager not available",
        );
    };
    match am.send(ResolveAlert { alert_id }).await {
        Ok(Ok(())) => Json(json!({ "status": "resolved" })).into_response(),
        Ok(Err(e)) => alert_error(e),
        Err(e) => json_error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

/// `GET /api/v1/predictions`: latest active prediction per (device, type)
pub async fn get_predictions_handler(State(state): State<HubState>) -> Response {
    let Some(am) = state.alert_manager() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "alert manager not available",
        );
    };
    match am.send(GetRecentPredictions { limit: 100 }).await {
        Ok(Ok(predictions)) => {
            let total = predictions.len();
            Json(json!({ "predictions": predictions, "total": total })).into_response()
        }
        Ok(Err(e)) => {
            error!("Failed to get predictions: {}", e);
            json_error(StatusCode::SERVICE_UNAVAILABLE, e)
        }
        Err(e) => json_error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()),
    }
}

// ----------------------------------------------------------------------
// Devices
// ----------------------------------------------------------------------

/// The edge-local part of a device id (`edge:PXIe-6368#2` -> `PXIe-6368#2`).
fn local_name(device_id: &str) -> &str {
    match device_id.rsplit_once(':') {
        Some((_, rest)) if !rest.is_empty() => rest,
        _ => device_id,
    }
}

fn metric_text(metrics: &HashMap<String, MetricValue>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match metrics.get(*k) {
        Some(MetricValue::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    })
}

/// Unified device JSON for live and offline devices.
struct DeviceView {
    device_id: String,
    edge_id: String,
    name: String,
    device_type: Option<String>,
    model: Option<String>,
    serial_number: Option<String>,
    slot: Option<i64>,
    status: String,
    metrics: HashMap<String, MetricValue>,
    last_seen: Option<DateTime<Utc>>,
    is_simulated: bool,
    live: bool,
}

impl DeviceView {
    fn from_row(row: &DeviceStatusRow) -> Self {
        let metrics = row.metrics();
        Self {
            device_id: row.id.clone(),
            edge_id: row.edge_id.clone(),
            name: row.device_name.clone(),
            device_type: Some(row.device_type.clone()),
            model: row
                .model
                .clone()
                .or_else(|| metric_text(&metrics, &["product", "model"])),
            serial_number: row.serial_number.clone(),
            slot: row.slot,
            status: row.status.clone().unwrap_or_else(|| "unknown".to_string()),
            metrics,
            last_seen: row.last_poll(),
            is_simulated: row.is_simulated.unwrap_or(0) != 0,
            live: false,
        }
    }

    fn from_live(edge_id: &str, snapshot: DeviceSnapshot, row: Option<&DeviceStatusRow>) -> Self {
        let mut view = match row {
            Some(row) => Self::from_row(row),
            None => Self {
                device_id: snapshot.device_id.clone(),
                edge_id: edge_id.to_string(),
                name: local_name(&snapshot.device_id).to_string(),
                device_type: None,
                model: None,
                serial_number: None,
                slot: None,
                status: String::new(),
                metrics: HashMap::new(),
                last_seen: None,
                is_simulated: false,
                live: true,
            },
        };
        if view.model.is_none() {
            view.model = metric_text(&snapshot.metrics, &["product", "model"]);
        }
        if view.slot.is_none() {
            view.slot = snapshot
                .metrics
                .get("slot")
                .and_then(|v| v.as_f64_finite())
                .map(|v| v as i64);
        }
        view.edge_id = edge_id.to_string();
        view.status = snapshot.status;
        view.metrics = snapshot.metrics;
        view.last_seen = Some(snapshot.last_seen);
        view.is_simulated = snapshot.is_simulated;
        view.live = true;
        view
    }

    fn to_json(&self) -> Value {
        let is_reachable = match self.metrics.get("is_reachable") {
            Some(MetricValue::Boolean(b)) => *b,
            _ => self.live && self.status != "offline",
        };
        json!({
            "device_id": self.device_id,
            "edge_id": self.edge_id,
            "name": self.name,
            "device_type": self.device_type,
            "model": self.model,
            "serial_number": self.serial_number,
            "slot": self.slot,
            "status": self.status,
            "metrics": self.metrics,
            "last_seen": self.last_seen.map(|t| t.to_rfc3339()),
            "is_simulated": self.is_simulated,
            "is_reachable": is_reachable,
            "live": self.live,
        })
    }
}

/// Live devices (from sessions) merged with the database registry, in one
/// DB query. Devices reported removed are hidden.
async fn collect_devices(
    state: &HubState,
    edge_filter: Option<&str>,
) -> Result<Vec<Value>, Response> {
    let rows = match state.db_pool() {
        Some(pool) => queries::list_devices(&pool, edge_filter)
            .await
            .map_err(db_error)?,
        None => Vec::new(),
    };
    let mut rows_by_id: HashMap<String, DeviceStatusRow> =
        rows.into_iter().map(|r| (r.id.clone(), r)).collect();

    let mut views: Vec<DeviceView> = Vec::new();
    for session in state.sessions().snapshot() {
        if edge_filter.is_some_and(|e| e != session.edge_id()) {
            continue;
        }
        for snapshot in session.devices().await {
            let row = rows_by_id.remove(&snapshot.device_id);
            views.push(DeviceView::from_live(
                session.edge_id(),
                snapshot,
                row.as_ref(),
            ));
        }
    }
    for row in rows_by_id.values() {
        if state.is_device_removed(&row.id) {
            continue;
        }
        views.push(DeviceView::from_row(row));
    }
    views.sort_by(|a, b| {
        a.edge_id
            .cmp(&b.edge_id)
            .then_with(|| a.device_id.cmp(&b.device_id))
    });
    Ok(views.iter().map(DeviceView::to_json).collect())
}

/// `GET /api/v1/devices?edge_id=`
pub async fn list_devices_handler(
    State(state): State<HubState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let edge = non_empty(&params, "edge_id").cloned();
    match collect_devices(&state, edge.as_deref()).await {
        Ok(devices) => {
            let total = devices.len();
            Json(json!({ "devices": devices, "total": total })).into_response()
        }
        Err(r) => r,
    }
}

/// `GET /api/v1/devices/:device_id/metrics?metric=temperature&since&until&limit=500`
pub async fn device_metrics_handler(
    State(state): State<HubState>,
    Path(device_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db_pool() else {
        return no_database();
    };
    let metric = non_empty(&params, "metric")
        .cloned()
        .unwrap_or_else(|| "temperature".to_string());
    let since = match parse_time(&params, "since") {
        Ok(v) => v,
        Err(e) => return bad_request(e),
    };
    let until = match parse_time(&params, "until") {
        Ok(v) => v,
        Err(e) => return bad_request(e),
    };
    let limit = match parse_limit(&params, DEFAULT_METRIC_LIMIT, MAX_METRIC_LIMIT) {
        Ok(v) => v,
        Err(e) => return bad_request(e),
    };
    match queries::metric_points(&pool, &device_id, &metric, since, until, limit).await {
        Ok(rows) => {
            let points: Vec<Value> = rows
                .into_iter()
                .map(|(timestamp, value)| json!({ "timestamp": timestamp, "value": value }))
                .collect();
            Json(json!({ "device_id": device_id, "metric": metric, "points": points }))
                .into_response()
        }
        Err(e) => db_error(e),
    }
}

/// Body of `POST /api/v1/devices/:device_id/actions`
#[derive(Debug, Deserialize)]
pub struct DeviceActionRequest {
    pub action_type: String,
    #[serde(default)]
    pub parameters: HashMap<String, String>,
}

/// Build the executable action for a manual request.
fn manual_action(request: &DeviceActionRequest, action_id: &str) -> Result<Action, String> {
    let mut action = match request.action_type.as_str() {
        "power_cycle" => {
            let delay = match request.parameters.get("delay_secs") {
                None => 0,
                Some(v) => v
                    .parse::<u64>()
                    .map_err(|_| format!("invalid delay_secs '{}'", v))?,
            };
            let mut action = Action::power_cycle(action_id, "Manual power cycle", "Requested via API");
            action.action_type = ActionType::PowerCycle { delay_secs: delay };
            action
        }
        "reset_driver" | "restart_services" | "custom_script" => {
            if request.action_type == "custom_script" && !request.parameters.contains_key("script") {
                return Err("custom_script requires parameters.script".to_string());
            }
            if request.action_type == "restart_services"
                && !["services", "service_name", "service"]
                    .iter()
                    .any(|k| request.parameters.contains_key(*k))
            {
                return Err("restart_services requires parameters.services".to_string());
            }
            let mut action =
                Action::edge_command(action_id, "Manual edge action", "Requested via API");
            action.action_type = ActionType::EdgeCommand {
                command: request.action_type.clone(),
                parameters: request.parameters.clone(),
            };
            action
        }
        other => {
            return Err(format!(
                "unknown action_type '{}' (expected power_cycle, reset_driver, restart_services or custom_script)",
                other
            ))
        }
    };
    // Manual actions run once; the operator decides about repeats
    action.retry_config.max_attempts = 1;
    Ok(action)
}

/// Find the edge owning a device: live sessions first, then the DB.
async fn device_edge(state: &HubState, device_id: &str) -> Result<Option<String>, Response> {
    for session in state.sessions().snapshot() {
        if session.device(device_id).await.is_some() {
            return Ok(Some(session.edge_id().to_string()));
        }
    }
    match state.db_pool() {
        Some(pool) => Ok(queries::get_device(&pool, device_id)
            .await
            .map_err(db_error)?
            .map(|row| row.edge_id)),
        None => Ok(None),
    }
}

/// `POST /api/v1/devices/:device_id/actions` -> 202 `{action_id}`
pub async fn device_action_handler(
    State(state): State<HubState>,
    Path(device_id): Path<String>,
    body: Result<Json<DeviceActionRequest>, JsonRejection>,
) -> Response {
    let request = match body {
        Ok(Json(r)) => r,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!("invalid request body: {}", e),
            )
        }
    };
    let action_id = format!("manual-{}", ulid::Ulid::new());
    let action = match manual_action(&request, &action_id) {
        Ok(a) => a,
        Err(e) => return json_error(StatusCode::BAD_REQUEST, e),
    };
    let Some(executor) = state.action_executor() else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "action executor not available",
        );
    };
    let edge_id = match device_edge(&state, &device_id).await {
        Ok(Some(edge)) => edge,
        Ok(None) => {
            return json_error(
                StatusCode::NOT_FOUND,
                format!("device '{}' not found", device_id),
            )
        }
        Err(r) => return r,
    };
    let context = ActionContext::new(&edge_id, &device_id, "")
        .with_variable("DEVICE_ID", &device_id)
        .with_variable("EDGE_ID", &edge_id);
    debug!(
        "Manual action {} ({}) for device {} on edge {}",
        action_id, request.action_type, device_id, edge_id
    );
    executor.do_send(ExecuteAction { action, context });
    (
        StatusCode::ACCEPTED,
        Json(json!({ "action_id": action_id })),
    )
        .into_response()
}

fn action_record_json(r: &ActionRecord) -> Value {
    json!({
        "id": r.id,
        "alert_id": r.alert_id,
        "device_id": r.device_id,
        "action_id": r.action_id,
        "action_type": r.action_type,
        "command": r.command,
        "exit_code": r.exit_code,
        "output": r.output,
        "duration_ms": r.duration_ms,
        "success": r.success.map(|s| s != 0),
        "retry_count": r.retry_count,
        "executed_at": r.executed_at,
    })
}

/// `GET /api/v1/actions?device_id&edge_id&limit`
pub async fn list_actions_handler(
    State(state): State<HubState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db_pool() else {
        return no_database();
    };
    let limit = match parse_limit(&params, DEFAULT_LIMIT, MAX_LIMIT) {
        Ok(v) => v,
        Err(e) => return bad_request(e),
    };
    let filter = ActionHistoryFilter {
        device_id: non_empty(&params, "device_id").cloned(),
        edge_id: non_empty(&params, "edge_id").cloned(),
        limit: Some(limit),
    };
    match ActionRepository::new(&pool).list(&filter).await {
        Ok(records) => {
            let actions: Vec<Value> = records.iter().map(action_record_json).collect();
            let total = actions.len();
            Json(json!({ "actions": actions, "total": total })).into_response()
        }
        Err(e) => db_error(e),
    }
}

// ----------------------------------------------------------------------
// Edges
// ----------------------------------------------------------------------

async fn live_edge_json(session: &crate::session::EdgeSession) -> Value {
    let info = session.heartbeat_info().await;
    json!({
        "edge_id": session.edge_id(),
        "name": session.name(),
        "hostname": session.hostname(),
        "ip_address": session.ip_address(),
        "status": "online",
        "live": true,
        "connected_at": session.connected_at().to_rfc3339(),
        "last_seen": session.last_heartbeat().await.to_rfc3339(),
        "device_count": session.device_count().await,
        "protocol_version": session.protocol_version(),
        "version": info.version,
        "uptime_secs": info.uptime_secs,
    })
}

fn offline_edge_json(edge: &nimon_core::EdgeNode, device_count: i64) -> Value {
    json!({
        "edge_id": edge.id,
        "name": edge.name,
        "hostname": edge.hostname,
        "ip_address": edge.ip_address,
        "status": "offline",
        "live": false,
        "connected_at": null,
        "last_seen": edge.last_seen.map(|t| t.to_rfc3339()),
        "device_count": device_count,
        "protocol_version": null,
        "version": null,
        "uptime_secs": null,
    })
}

/// `GET /api/v1/edges`: live sessions merged with the database registry
pub async fn list_edges(State(state): State<HubState>) -> Response {
    let sessions = state.sessions();
    let mut edges = Vec::new();
    for session in sessions.snapshot() {
        edges.push(live_edge_json(&session).await);
    }

    if let Some(pool) = state.db_pool() {
        let counts = queries::device_counts_by_edge(&pool)
            .await
            .unwrap_or_else(|e| {
                warn!("Failed to count devices per edge: {}", e);
                HashMap::new()
            });
        match nimon_core::db::edge_repo::EdgeRepository::new(&pool)
            .list()
            .await
        {
            Ok(known) => {
                for edge in known.iter().filter(|e| !sessions.contains(&e.id)) {
                    edges.push(offline_edge_json(
                        edge,
                        counts.get(&edge.id).copied().unwrap_or(0),
                    ));
                }
            }
            Err(e) => warn!("Failed to list edges from database: {}", e),
        }
    }

    let total = edges.len();
    Json(json!({ "edges": edges, "total": total })).into_response()
}

/// `GET /api/v1/edges/:edge_id`
pub async fn get_edge(State(state): State<HubState>, Path(edge_id): Path<String>) -> Response {
    if let Some(session) = state.sessions().get(&edge_id) {
        return Json(live_edge_json(&session).await).into_response();
    }
    if let Some(pool) = state.db_pool() {
        match nimon_core::db::edge_repo::EdgeRepository::new(&pool)
            .get(&edge_id)
            .await
        {
            Ok(edge) => {
                let count = queries::device_counts_by_edge(&pool)
                    .await
                    .ok()
                    .and_then(|c| c.get(&edge_id).copied())
                    .unwrap_or(0);
                return Json(offline_edge_json(&edge, count)).into_response();
            }
            Err(NimonError::EdgeNotFound(_)) => {}
            Err(e) => return db_error(e),
        }
    }
    json_error(
        StatusCode::NOT_FOUND,
        format!("edge '{}' not found", edge_id),
    )
}

/// `GET /api/v1/edges/:edge_id/devices`
pub async fn get_edge_devices(
    State(state): State<HubState>,
    Path(edge_id): Path<String>,
) -> Response {
    let live = state.sessions().contains(&edge_id);
    let devices = match collect_devices(&state, Some(&edge_id)).await {
        Ok(d) => d,
        Err(r) => return r,
    };
    if !live && devices.is_empty() {
        let known = match state.db_pool() {
            Some(pool) => nimon_core::db::edge_repo::EdgeRepository::new(&pool)
                .get(&edge_id)
                .await
                .is_ok(),
            None => false,
        };
        if !known {
            return json_error(
                StatusCode::NOT_FOUND,
                format!("edge '{}' not found", edge_id),
            );
        }
    }
    let total = devices.len();
    Json(json!({
        "edge_id": edge_id,
        "devices": devices,
        "total": total,
        "live": live,
    }))
    .into_response()
}

/// Body of `POST /api/v1/edges/:edge_id/config`
#[derive(Debug, Deserialize)]
pub struct EdgeConfigRequest {
    pub poll_interval_secs: Option<u64>,
    pub temperature_warning: Option<f64>,
    pub temperature_critical: Option<f64>,
}

impl EdgeConfigRequest {
    fn as_desired(&self) -> EdgeDesiredConfig {
        EdgeDesiredConfig {
            poll_interval_secs: self.poll_interval_secs,
            temperature_warning: self.temperature_warning,
            temperature_critical: self.temperature_critical,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.as_desired().is_empty() {
            return Err("no configuration fields provided".to_string());
        }
        if self.poll_interval_secs == Some(0) {
            return Err("poll_interval_secs must be >= 1".to_string());
        }
        for (name, value) in [
            ("temperature_warning", self.temperature_warning),
            ("temperature_critical", self.temperature_critical),
        ] {
            if value.is_some_and(|v| !v.is_finite()) {
                return Err(format!("{} must be a finite number", name));
            }
        }
        Ok(())
    }
}

/// Effective thresholds must satisfy warning < critical.
pub(crate) fn check_threshold_order(effective: &EdgeDesiredConfig) -> Result<(), String> {
    let w = effective
        .temperature_warning
        .unwrap_or(DEFAULT_TEMP_WARNING_C);
    let c = effective
        .temperature_critical
        .unwrap_or(DEFAULT_TEMP_CRITICAL_C);
    if w >= c {
        return Err(format!(
            "temperature_warning ({}) must be below temperature_critical ({})",
            w, c
        ));
    }
    Ok(())
}

/// `POST /api/v1/edges/:edge_id/config`: store as the edge's desired
/// override, push immediately when connected.
pub async fn push_edge_config_handler(
    State(state): State<HubState>,
    Path(edge_id): Path<String>,
    body: Result<Json<EdgeConfigRequest>, JsonRejection>,
) -> Response {
    let request = match body {
        Ok(Json(r)) => r,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!("invalid request body: {}", e),
            )
        }
    };
    if let Err(e) = request.validate() {
        return json_error(StatusCode::BAD_REQUEST, e);
    }

    // Validate warning < critical against the merged effective state and
    // store it under the same lock (no check-then-act race)
    let effective =
        match state.try_merge_edge_override(&edge_id, &request.as_desired(), check_threshold_order)
        {
            Ok(effective) => effective,
            Err(e) => return json_error(StatusCode::BAD_REQUEST, e),
        };

    let Some(session) = state.sessions().get(&edge_id) else {
        return (StatusCode::ACCEPTED, Json(json!({ "status": "stored" }))).into_response();
    };

    // 1.1+: only the provided fields; 1.0: full thresholds from the
    // effective desired state
    let update = if session.is_legacy_protocol() {
        let mut legacy = effective.clone();
        legacy.poll_interval_secs = request.poll_interval_secs;
        desired_to_config_update(&legacy, true)
    } else {
        desired_to_config_update(&request.as_desired(), false)
    };
    let Some(update) = update else {
        return (StatusCode::ACCEPTED, Json(json!({ "status": "stored" }))).into_response();
    };
    match session.send(WsMessage::config_update(update)) {
        Ok(()) => Json(json!({ "status": "pushed" })).into_response(),
        Err(e) => json_error(
            StatusCode::BAD_GATEWAY,
            format!("stored, but failed to push config: {}", e),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manual_action_mapping() {
        let req = |t: &str, params: &[(&str, &str)]| DeviceActionRequest {
            action_type: t.to_string(),
            parameters: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let a = manual_action(&req("power_cycle", &[("delay_secs", "3")]), "m1").unwrap();
        assert_eq!(a.action_type, ActionType::PowerCycle { delay_secs: 3 });
        assert_eq!(a.retry_config.max_attempts, 1);
        assert!(manual_action(&req("custom_script", &[]), "m").is_err());
        assert!(manual_action(&req("restart_services", &[]), "m").is_err());
        assert!(manual_action(&req("reset_driver", &[]), "m").is_ok());
        assert!(manual_action(&req("format_disk", &[]), "m").is_err());
    }

    #[test]
    fn test_local_name() {
        assert_eq!(local_name("edge-1:PXIe-6368#2"), "PXIe-6368#2");
        assert_eq!(local_name("plain"), "plain");
    }
}
