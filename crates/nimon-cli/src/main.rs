//! NIMon CLI
//!
//! Command-line client for the NIMon hub REST API: health, edges,
//! devices and metric export, alerts, predictions, actions and settings.
//! `--json` prints raw API responses for scripting.

mod api;
mod cli;
mod format;

use std::io::{BufRead, IsTerminal, Write};
use std::time::Duration;

use anyhow::{bail, Result};
use clap::Parser;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use api::{api_path, Client};
use cli::{
    ActionsCommands, AlertsCommands, Cli, Commands, DevicesCommands, EdgesCommands, HistoryArgs,
    MetricsArgs, PredictionsCommands,
};
use format::{age, opt, timestamp, Table};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("error: {:#}", e);
        std::process::exit(1);
    }
}

/// Output mode shared by every command.
struct Ctx {
    client: Client,
    json: bool,
}

impl Ctx {
    fn print_json(&self, value: &Value) -> Result<()> {
        println!("{}", serde_json::to_string_pretty(value)?);
        Ok(())
    }
}

fn decode<T: DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(value.clone())
        .map_err(|e| anyhow::anyhow!("unexpected response shape from hub: {}", e))
}

async fn run(cli: Cli) -> Result<()> {
    let client = Client::new(
        &cli.hub,
        cli.token.clone(),
        Duration::from_secs(cli.timeout.max(1)),
    )?;
    let ctx = Ctx {
        client,
        json: cli.json,
    };

    match cli.command {
        Commands::Status => status(&ctx).await,
        Commands::Edges { command } => match command.unwrap_or(EdgesCommands::List) {
            EdgesCommands::List => edges_list(&ctx).await,
            EdgesCommands::Show { edge_id } => edge_show(&ctx, &edge_id).await,
            EdgesCommands::Devices { edge_id } => {
                let value = ctx
                    .client
                    .get(&api_path(&["api", "v1", "edges", &edge_id, "devices"]), &[])
                    .await?;
                if ctx.json {
                    return ctx.print_json(&value);
                }
                let list: api::DeviceList = decode(&value)?;
                if list.live == Some(false) {
                    println!("Edge {} is offline: showing last-known state\n", edge_id);
                }
                print_devices(&list.devices, false);
                Ok(())
            }
            EdgesCommands::PushConfig {
                edge_id,
                poll_interval,
                warning,
                critical,
            } => push_config(&ctx, &edge_id, poll_interval, warning, critical).await,
        },
        Commands::Devices { command } => match command
            .unwrap_or(DevicesCommands::List { edge: None })
        {
            DevicesCommands::List { edge } => {
                let query: Vec<(&str, String)> = edge.into_iter().map(|e| ("edge_id", e)).collect();
                let value = ctx.client.get("/api/v1/devices", &query).await?;
                if ctx.json {
                    return ctx.print_json(&value);
                }
                let list: api::DeviceList = decode(&value)?;
                print_devices(&list.devices, true);
                Ok(())
            }
            DevicesCommands::Metrics(args) => metrics(&ctx, args).await,
        },
        Commands::Alerts { command } => match command.unwrap_or(AlertsCommands::List) {
            AlertsCommands::List => alerts_list(&ctx).await,
            AlertsCommands::History(args) => alerts_history(&ctx, args).await,
            AlertsCommands::Ack { alert_id } => {
                alert_transition(&ctx, &alert_id, "acknowledge", "acknowledged").await
            }
            AlertsCommands::Resolve { alert_id } => {
                alert_transition(&ctx, &alert_id, "resolve", "resolved").await
            }
        },
        Commands::Predictions { command } => match command.unwrap_or(PredictionsCommands::List) {
            PredictionsCommands::List => predictions(&ctx).await,
        },
        Commands::Actions { command } => match command.unwrap_or(ActionsCommands::List {
            device: None,
            edge: None,
            limit: 50,
        }) {
            ActionsCommands::List {
                device,
                edge,
                limit,
            } => actions_list(&ctx, device, edge, limit).await,
            ActionsCommands::Run {
                device_id,
                action_type,
                params,
                yes,
            } => action_run(&ctx, &device_id, action_type, params, yes).await,
        },
        Commands::Settings => settings(&ctx).await,
    }
}

// ----------------------------------------------------------------------
// status
// ----------------------------------------------------------------------

async fn status(ctx: &Ctx) -> Result<()> {
    let c = &ctx.client;
    let health = c.health().await?;
    let (edges, devices, alerts, predictions) = tokio::join!(
        c.get("/api/v1/edges", &[]),
        c.get("/api/v1/devices", &[]),
        c.get("/api/v1/alerts", &[]),
        c.get("/api/v1/predictions", &[]),
    );

    let count = |r: &Result<Value>, key: &str| -> Option<usize> {
        r.as_ref().ok()?.get(key)?.as_array().map(Vec::len)
    };
    let edge_list = edges.as_ref().ok().and_then(|v| v["edges"].as_array());
    let edges_online = edge_list.map(|l| l.iter().filter(|e| e["live"] == true).count());
    let device_list = devices.as_ref().ok().and_then(|v| v["devices"].as_array());
    let devices_live = device_list.map(|l| l.iter().filter(|d| d["live"] == true).count());
    let by_severity = |sev: &str| {
        alerts
            .as_ref()
            .ok()
            .and_then(|v| v["alerts"].as_array())
            .map(|l| l.iter().filter(|a| a["severity"] == sev).count())
    };
    let errors: Vec<String> = [
        ("edges", &edges),
        ("devices", &devices),
        ("alerts", &alerts),
        ("predictions", &predictions),
    ]
    .iter()
    .filter_map(|(name, r)| r.as_ref().err().map(|e| format!("{}: {:#}", name, e)))
    .collect();

    if ctx.json {
        return ctx.print_json(&json!({
            "hub": c.base_url(),
            "health": health,
            "edges": { "total": count(&edges, "edges"), "online": edges_online },
            "devices": { "total": count(&devices, "devices"), "live": devices_live },
            "alerts": {
                "active": count(&alerts, "alerts"),
                "critical": by_severity("critical"),
                "warning": by_severity("warning"),
                "info": by_severity("info"),
            },
            "predictions": { "active": count(&predictions, "predictions") },
            "errors": errors,
        }));
    }

    let h: api::Health = decode(&health)?;
    let ok = |b: Option<bool>| match b {
        Some(true) => "ok",
        Some(false) => "FAILED",
        None => "-",
    };
    println!("Hub          {}", c.base_url());
    println!(
        "Status       {}{}",
        h.status,
        if h.status == "healthy" { "" } else { "  (!)" }
    );
    println!("Version      {}", opt(h.version));
    println!("Uptime       {}", opt(h.uptime_secs.map(format::uptime)));
    println!(
        "Components   database {}, alert manager {}",
        ok(h.db_ok),
        ok(h.alert_manager_ok)
    );
    println!(
        "Edges        {} online / {} known",
        opt(edges_online.or(h.edges_connected.map(|n| n as usize))),
        opt(count(&edges, "edges"))
    );
    println!(
        "Devices      {} live / {} known",
        opt(devices_live),
        opt(count(&devices, "devices"))
    );
    println!(
        "Alerts       {} active ({} critical, {} warning, {} info)",
        opt(count(&alerts, "alerts")),
        opt(by_severity("critical")),
        opt(by_severity("warning")),
        opt(by_severity("info"))
    );
    println!(
        "Predictions  {} active",
        opt(count(&predictions, "predictions"))
    );
    for e in errors {
        eprintln!("warning: {}", e);
    }
    Ok(())
}

// ----------------------------------------------------------------------
// edges
// ----------------------------------------------------------------------

async fn edges_list(ctx: &Ctx) -> Result<()> {
    let value = ctx.client.get("/api/v1/edges", &[]).await?;
    if ctx.json {
        return ctx.print_json(&value);
    }
    let list: api::EdgeList = decode(&value)?;
    if list.edges.is_empty() {
        println!("No edges known to the hub");
        return Ok(());
    }
    let mut t = Table::new(&[
        ("EDGE ID", 32),
        ("NAME", 28),
        ("STATUS", 8),
        ("DEVICES", 7),
        ("VERSION", 10),
        ("HOST", 24),
        ("LAST SEEN", 12),
    ]);
    for e in &list.edges {
        t.row(vec![
            e.edge_id.clone(),
            e.name.clone().unwrap_or_default(),
            e.status.clone().unwrap_or_else(|| "-".into()),
            opt(e.device_count),
            opt(e.version.clone()),
            e.hostname
                .clone()
                .or_else(|| e.ip_address.clone())
                .unwrap_or_else(|| "-".into()),
            age(e.last_seen.as_deref()),
        ]);
    }
    t.print();
    Ok(())
}

async fn edge_show(ctx: &Ctx, edge_id: &str) -> Result<()> {
    let value = ctx
        .client
        .get(&api_path(&["api", "v1", "edges", edge_id]), &[])
        .await?;
    if ctx.json {
        return ctx.print_json(&value);
    }
    let e: api::Edge = decode(&value)?;
    let rows = [
        ("ID", e.edge_id.clone()),
        ("Name", opt(e.name.clone())),
        (
            "Status",
            format!(
                "{}{}",
                e.status.clone().unwrap_or_else(|| "-".into()),
                if e.live { " (connected)" } else { "" }
            ),
        ),
        ("Hostname", opt(e.hostname.clone())),
        ("IP address", opt(e.ip_address.clone())),
        ("Devices", opt(e.device_count)),
        ("Edge version", opt(e.version.clone())),
        ("Protocol", opt(e.protocol_version.clone())),
        ("Edge uptime", opt(e.uptime_secs.map(format::uptime))),
        ("Connected at", timestamp(e.connected_at.as_deref())),
        ("Last seen", timestamp(e.last_seen.as_deref())),
    ];
    for (k, v) in rows {
        println!("{:<14}{}", k, v);
    }
    Ok(())
}

async fn push_config(
    ctx: &Ctx,
    edge_id: &str,
    poll_interval: Option<u64>,
    warning: Option<f64>,
    critical: Option<f64>,
) -> Result<()> {
    if let (Some(w), Some(c)) = (warning, critical) {
        if w >= c {
            bail!("--warning ({}) must be below --critical ({})", w, c);
        }
    }
    let mut body = serde_json::Map::new();
    if let Some(p) = poll_interval {
        body.insert("poll_interval_secs".into(), json!(p));
    }
    if let Some(w) = warning {
        body.insert("temperature_warning".into(), json!(w));
    }
    if let Some(c) = critical {
        body.insert("temperature_critical".into(), json!(c));
    }
    let (_, value) = ctx
        .client
        .post(
            &api_path(&["api", "v1", "edges", edge_id, "config"]),
            &Value::Object(body),
        )
        .await?;
    if ctx.json {
        return ctx.print_json(&value);
    }
    match value["status"].as_str() {
        Some("pushed") => println!("Config pushed to edge {}", edge_id),
        Some("stored") => println!(
            "Edge {} is not connected: config stored, it is applied when the edge registers",
            edge_id
        ),
        other => println!("Hub answered: {}", other.unwrap_or("?")),
    }
    Ok(())
}

// ----------------------------------------------------------------------
// devices
// ----------------------------------------------------------------------

fn print_devices(devices: &[api::Device], show_edge: bool) {
    if devices.is_empty() {
        println!("No devices");
        return;
    }
    let mut columns = vec![("DEVICE ID", 40)];
    if show_edge {
        columns.push(("EDGE", 20));
    }
    columns.extend([
        ("MODEL", 18),
        ("SLOT", 4),
        ("STATUS", 8),
        ("TEMP C", 6),
        ("STATE", 11),
        ("LAST SEEN", 12),
    ]);
    let mut t = Table::new(&columns);
    for d in devices {
        let mut row = vec![d.device_id.clone()];
        if show_edge {
            row.push(d.edge_id.clone().unwrap_or_else(|| "-".into()));
        }
        let state = if !d.live {
            "last-known"
        } else if !d.is_reachable {
            "unreachable"
        } else if d.is_simulated {
            "live (sim)"
        } else {
            "live"
        };
        row.extend([
            d.model
                .clone()
                .or_else(|| d.name.clone())
                .unwrap_or_else(|| "-".into()),
            opt(d.slot),
            d.status.clone().unwrap_or_else(|| "-".into()),
            d.metrics
                .get("temperature")
                .map(format::metric_value)
                .unwrap_or_else(|| "-".into()),
            state.to_string(),
            age(d.last_seen.as_deref()),
        ]);
        t.row(row);
    }
    t.print();
}

async fn metrics(ctx: &Ctx, args: MetricsArgs) -> Result<()> {
    let mut query: Vec<(&str, String)> = vec![("metric", args.metric.clone())];
    let since = match (&args.since, args.last) {
        (Some(s), _) => Some(s.clone()),
        (None, Some(window)) => Some((chrono::Utc::now() - window).to_rfc3339()),
        (None, None) => None,
    };
    if let Some(s) = since {
        query.push(("since", s));
    }
    if let Some(u) = &args.until {
        query.push(("until", u.clone()));
    }
    if let Some(l) = args.limit {
        query.push(("limit", l.to_string()));
    }
    let value = ctx
        .client
        .get(
            &api_path(&["api", "v1", "devices", &args.device_id, "metrics"]),
            &query,
        )
        .await?;
    if ctx.json {
        return ctx.print_json(&value);
    }
    let series: api::MetricSeries = decode(&value)?;
    if args.csv {
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        writeln!(out, "timestamp,value")?;
        for p in &series.points {
            writeln!(out, "{},{}", format::csv_field(&p.timestamp), p.value)?;
        }
        return Ok(());
    }
    if series.points.is_empty() {
        println!(
            "No '{}' points for {} in the selected window",
            series.metric, series.device_id
        );
        return Ok(());
    }
    let mut t = Table::new(&[("TIMESTAMP", usize::MAX), ("VALUE", usize::MAX)]);
    for p in &series.points {
        t.row(vec![
            timestamp(Some(&p.timestamp)),
            format!("{:.2}", p.value),
        ]);
    }
    t.print();
    let values: Vec<f64> = series.points.iter().map(|p| p.value).collect();
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let avg = values.iter().sum::<f64>() / values.len() as f64;
    println!(
        "\n{} point{} of '{}' for {}: min {:.2}, avg {:.2}, max {:.2}",
        values.len(),
        if values.len() == 1 { "" } else { "s" },
        series.metric,
        series.device_id,
        min,
        avg,
        max
    );
    Ok(())
}

// ----------------------------------------------------------------------
// alerts
// ----------------------------------------------------------------------

fn print_alerts(alerts: &[api::Alert], history: bool) {
    let mut columns = vec![
        ("ID", usize::MAX),
        ("SEVERITY", 8),
        ("STATUS", 12),
        ("DEVICE", 34),
        ("TITLE", 40),
        ("TRIGGERED", 10),
    ];
    if history {
        columns.push(("RESOLVED", 10));
    } else {
        columns.push(("FIRED", 5));
    }
    let mut t = Table::new(&columns);
    for a in alerts {
        let title = if a.title.is_empty() {
            &a.message
        } else {
            &a.title
        };
        let mut row = vec![
            a.id.clone(),
            a.severity.to_uppercase(),
            a.status.clone(),
            a.target().to_string(),
            title.clone(),
            age(a.triggered_at.as_deref()),
        ];
        if history {
            row.push(age(a.resolved_at.as_deref()));
        } else {
            row.push(opt(a.fired_count));
        }
        t.row(row);
    }
    t.print();
}

async fn alerts_list(ctx: &Ctx) -> Result<()> {
    let value = ctx.client.get("/api/v1/alerts", &[]).await?;
    if ctx.json {
        return ctx.print_json(&value);
    }
    let list: api::AlertList = decode(&value)?;
    if list.alerts.is_empty() {
        println!("No active alerts");
        return Ok(());
    }
    print_alerts(&list.alerts, false);
    Ok(())
}

/// The CLI spelling of a value enum (matches the hub's lowercase names).
fn enum_name<T: clap::ValueEnum>(v: &T) -> String {
    v.to_possible_value()
        .map(|p| p.get_name().to_string())
        .unwrap_or_default()
}

async fn alerts_history(ctx: &Ctx, args: HistoryArgs) -> Result<()> {
    let mut query: Vec<(&str, String)> = vec![("limit", args.limit.to_string())];
    if let Some(s) = &args.severity {
        query.push(("severity", enum_name(s)));
    }
    if let Some(s) = &args.status {
        query.push(("status", enum_name(s)));
    }
    if let Some(e) = args.edge {
        query.push(("edge_id", e));
    }
    if let Some(d) = args.device {
        query.push(("device_id", d));
    }
    let since = match (args.since, args.last) {
        (Some(s), _) => Some(s),
        (None, Some(window)) => Some((chrono::Utc::now() - window).to_rfc3339()),
        (None, None) => None,
    };
    if let Some(s) = since {
        query.push(("since", s));
    }
    if let Some(u) = args.until {
        query.push(("until", u));
    }
    let value = ctx.client.get("/api/v1/alerts/history", &query).await?;
    if ctx.json {
        return ctx.print_json(&value);
    }
    let list: api::AlertList = decode(&value)?;
    if list.alerts.is_empty() {
        println!("No alerts match");
        return Ok(());
    }
    print_alerts(&list.alerts, true);
    Ok(())
}

async fn alert_transition(ctx: &Ctx, alert_id: &str, verb: &str, done: &str) -> Result<()> {
    let (_, value) = ctx
        .client
        .post(
            &api_path(&["api", "v1", "alerts", alert_id, verb]),
            &json!({}),
        )
        .await?;
    if ctx.json {
        return ctx.print_json(&value);
    }
    println!("Alert {} {}", alert_id, done);
    Ok(())
}

// ----------------------------------------------------------------------
// predictions / actions / settings
// ----------------------------------------------------------------------

async fn predictions(ctx: &Ctx) -> Result<()> {
    let value = ctx.client.get("/api/v1/predictions", &[]).await?;
    if ctx.json {
        return ctx.print_json(&value);
    }
    let list: api::PredictionList = decode(&value)?;
    if list.predictions.is_empty() {
        println!("No active predictions");
        return Ok(());
    }
    let mut t = Table::new(&[
        ("DEVICE", 40),
        ("EDGE", 20),
        ("TYPE", 22),
        ("PROBABILITY", 11),
        ("ETA", 8),
        ("CREATED", 10),
    ]);
    for p in &list.predictions {
        t.row(vec![
            p.device_id.clone(),
            p.edge_id.clone().unwrap_or_else(|| "-".into()),
            format::plain(&p.prediction_type),
            format!("{:.0}%", p.probability * 100.0),
            p.eta_minutes
                .map(|m| format!("{}m", m))
                .unwrap_or_else(|| "-".into()),
            age(p.created_at.as_deref()),
        ]);
    }
    t.print();
    Ok(())
}

async fn actions_list(
    ctx: &Ctx,
    device: Option<String>,
    edge: Option<String>,
    limit: u32,
) -> Result<()> {
    let mut query: Vec<(&str, String)> = vec![("limit", limit.to_string())];
    if let Some(d) = device {
        query.push(("device_id", d));
    }
    if let Some(e) = edge {
        query.push(("edge_id", e));
    }
    let value = ctx.client.get("/api/v1/actions", &query).await?;
    if ctx.json {
        return ctx.print_json(&value);
    }
    let list: api::ActionList = decode(&value)?;
    if list.actions.is_empty() {
        println!("No actions recorded");
        return Ok(());
    }
    let mut t = Table::new(&[
        ("EXECUTED", 10),
        ("DEVICE", 34),
        ("TYPE", 16),
        ("RESULT", 7),
        ("EXIT", 4),
        ("TIME", 7),
        ("ACTION ID", usize::MAX),
        ("OUTPUT", 40),
    ]);
    for a in &list.actions {
        t.row(vec![
            age(a.executed_at.as_deref()),
            a.device_id.clone(),
            a.action_type.clone(),
            match a.success {
                Some(true) => "ok".into(),
                Some(false) => "FAILED".into(),
                None => "-".into(),
            },
            opt(a.exit_code),
            a.duration_ms
                .map(|ms| format!("{:.1}s", ms as f64 / 1000.0))
                .unwrap_or_else(|| "-".into()),
            a.action_id.clone(),
            a.output.clone().unwrap_or_default(),
        ]);
    }
    t.print();
    Ok(())
}

/// Ask `prompt` on the terminal; false unless the answer is y/yes.
fn confirm(prompt: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("refusing to run without confirmation: stdin is not a terminal (pass --yes)");
    }
    eprint!("{} [y/N] ", prompt);
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

async fn action_run(
    ctx: &Ctx,
    device_id: &str,
    action_type: cli::ActionTypeArg,
    params: Vec<(String, String)>,
    yes: bool,
) -> Result<()> {
    let parameters: serde_json::Map<String, Value> = params
        .into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();
    let described = if parameters.is_empty() {
        String::new()
    } else {
        format!(" with {}", Value::Object(parameters.clone()))
    };
    if !yes
        && !confirm(&format!(
            "Run {} on device {}{}?",
            action_type.as_str(),
            device_id,
            described
        ))?
    {
        bail!("aborted");
    }
    let (_, value) = ctx
        .client
        .post(
            &api_path(&["api", "v1", "devices", device_id, "actions"]),
            &json!({ "action_type": action_type.as_str(), "parameters": parameters }),
        )
        .await?;
    if ctx.json {
        return ctx.print_json(&value);
    }
    let action_id = value["action_id"].as_str().unwrap_or("?");
    println!("Action accepted: {}", action_id);
    println!(
        "Follow its result with: nimon-cli actions list --device '{}'",
        device_id
    );
    Ok(())
}

async fn settings(ctx: &Ctx) -> Result<()> {
    let value = ctx.client.get("/api/v1/settings", &[]).await?;
    if ctx.json {
        return ctx.print_json(&value);
    }
    let s: api::Settings = decode(&value)?;
    println!("Hub version                 {}", opt(s.version));
    println!(
        "Default thresholds          warning {} C, critical {} C",
        opt(s.thresholds.temperature_warning),
        opt(s.thresholds.temperature_critical)
    );
    println!(
        "Prediction alert threshold  {}",
        opt(s
            .prediction_alert_threshold
            .map(|p| format!("{:.0}%", p * 100.0)))
    );
    println!(
        "Edge offline after          {}",
        opt(s.edge_offline_after_secs.map(|v| if v == 0 {
            "disabled".to_string()
        } else {
            format!("{}s", v)
        }))
    );
    println!(
        "Writes require token        {}",
        s.auth
            .map(|a| if a.writes_require_token { "yes" } else { "no" })
            .unwrap_or("-")
    );
    if !s.edge_thresholds.is_empty() {
        println!();
        let mut t = Table::new(&[
            ("EDGE", 32),
            ("WARNING C", 9),
            ("CRITICAL C", 10),
            ("POLL SECS", 12),
        ]);
        for (edge, th) in &s.edge_thresholds {
            t.row(vec![
                edge.clone(),
                opt(th.temperature_warning),
                opt(th.temperature_critical),
                th.poll_interval_secs
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "edge default".into()),
            ]);
        }
        t.print();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::truncate;

    #[test]
    fn truncation_used_for_titles_is_char_safe() {
        // The old byte-slicing (`&title[..17]`) panicked on this input
        let title = "Température critique dépassée sur le châssis";
        let t = truncate(title, 20);
        assert_eq!(t.chars().count(), 20);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn alerts_table_renders_non_ascii() {
        let alerts: api::AlertList = serde_json::from_value(json!({
            "alerts": [{
                "id": "01JABCDEF", "device_id": "edge-1:PXIe-6368#2", "edge_id": "edge-1",
                "severity": "critical", "status": "firing",
                "title": "Überhitzung – Temperatur über 85 °C im Steckplatz 2 des Chassis",
                "message": "", "triggered_at": "2026-09-24T10:00:00Z", "fired_count": 2
            }]
        }))
        .unwrap();
        // Must not panic
        print_alerts(&alerts.alerts, false);
        print_alerts(&alerts.alerts, true);
    }
}
