//! Output helpers: char-safe truncation, padded tables, local timestamps
//! with relative age, and duration parsing for `--last`.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};

/// Truncate to at most `max` characters (not bytes), ending with `…`
/// when shortened. Safe for any UTF-8 input.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Display width approximation: characters, not bytes.
fn width(s: &str) -> usize {
    s.chars().count()
}

/// A simple left-aligned text table with per-column max widths.
pub struct Table {
    headers: Vec<String>,
    max_widths: Vec<usize>,
    rows: Vec<Vec<String>>,
}

impl Table {
    /// `columns`: (header, max width); cells longer than the max are
    /// truncated with an ellipsis. Use `usize::MAX` for "never truncate".
    pub fn new(columns: &[(&str, usize)]) -> Self {
        Self {
            headers: columns.iter().map(|(h, _)| h.to_string()).collect(),
            max_widths: columns.iter().map(|(_, w)| *w).collect(),
            rows: Vec::new(),
        }
    }

    pub fn row(&mut self, cells: Vec<String>) {
        let cells = cells
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                // Keep cells on one line
                let c = c.replace(['\r', '\n', '\t'], " ");
                match self.max_widths.get(i) {
                    Some(&max) => truncate(&c, max),
                    None => c,
                }
            })
            .collect();
        self.rows.push(cells);
    }

    pub fn render(&self) -> String {
        let mut widths: Vec<usize> = self.headers.iter().map(|h| width(h)).collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < widths.len() {
                    widths[i] = widths[i].max(width(cell));
                }
            }
        }
        let line = |cells: &[String]| {
            let mut out = String::new();
            for (i, cell) in cells.iter().enumerate() {
                let last = i + 1 == cells.len();
                out.push_str(cell);
                if !last {
                    let pad = widths
                        .get(i)
                        .copied()
                        .unwrap_or(0)
                        .saturating_sub(width(cell));
                    out.push_str(&" ".repeat(pad + 2));
                }
            }
            out.trim_end().to_string()
        };
        let mut out = String::new();
        out.push_str(&line(&self.headers));
        out.push('\n');
        for row in &self.rows {
            out.push_str(&line(row));
            out.push('\n');
        }
        out
    }

    pub fn print(&self) {
        print!("{}", self.render());
    }
}

/// Parse a hub timestamp: RFC3339, or SQLite `YYYY-MM-DD HH:MM:SS[.f]` (UTC).
pub fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(Utc.from_utc_datetime(&naive));
        }
    }
    None
}

/// Compact relative age: `just now`, `42s ago`, `5m ago`, `3h ago`,
/// `2d ago` (or `in 5m` for future times).
pub fn relative_age(t: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = now.signed_duration_since(t).num_seconds();
    let (future, secs) = (secs < 0, secs.unsigned_abs());
    if secs < 5 {
        return "just now".to_string();
    }
    let span = compact_duration(secs);
    if future {
        format!("in {}", span)
    } else {
        format!("{} ago", span)
    }
}

/// `42s`, `5m`, `3h`, `2d` (largest whole unit).
pub fn compact_duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{}s", s),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// Uptime such as `3d 4h`, `2h 5m`, `42s`.
pub fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{}d {}h", d, h)
    } else if h > 0 {
        format!("{}h {}m", h, m)
    } else if m > 0 {
        format!("{}m {}s", m, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

/// Local time plus relative age, e.g. `2026-09-24 10:15:03 (5m ago)`.
/// Unparseable input is shown as-is; `None`/empty as `-`.
pub fn timestamp(s: Option<&str>) -> String {
    match s.filter(|v| !v.trim().is_empty()) {
        None => "-".to_string(),
        Some(raw) => match parse_timestamp(raw) {
            Some(t) => format!(
                "{} ({})",
                t.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S"),
                relative_age(t, Utc::now())
            ),
            None => raw.to_string(),
        },
    }
}

/// Relative age only (for dense tables), `-` when missing.
pub fn age(s: Option<&str>) -> String {
    match s.and_then(parse_timestamp) {
        Some(t) => relative_age(t, Utc::now()),
        None => "-".to_string(),
    }
}

/// Parse `90s`, `15m`, `1h`, `2d`, `1w` (a bare number is seconds).
pub fn parse_duration(s: &str) -> Result<chrono::Duration, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: i64 = num
        .parse()
        .map_err(|_| format!("invalid duration '{}': expected e.g. 30m, 1h, 7d", s))?;
    let secs = match unit.trim() {
        "" | "s" | "sec" | "secs" => n,
        "m" | "min" | "mins" => n * 60,
        "h" | "hr" | "hrs" => n * 3600,
        "d" | "day" | "days" => n * 86_400,
        "w" | "week" | "weeks" => n * 604_800,
        other => {
            return Err(format!(
                "invalid duration unit '{}' in '{}': use s, m, h, d or w",
                other, s
            ))
        }
    };
    if secs <= 0 {
        return Err(format!("duration '{}' must be positive", s));
    }
    Ok(chrono::Duration::seconds(secs))
}

/// Validate an RFC3339 timestamp argument (normalized to UTC).
pub fn parse_rfc3339_arg(s: &str) -> Result<String, String> {
    DateTime::parse_from_rfc3339(s.trim())
        .map(|t| t.with_timezone(&Utc).to_rfc3339())
        .map_err(|_| {
            format!(
                "invalid timestamp '{}': expected RFC3339, e.g. 2026-09-24T08:00:00Z",
                s
            )
        })
}

/// Render a metric JSON value compactly.
pub fn metric_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() != 0.0 => format!("{:.1}", f),
            _ => n.to_string(),
        },
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "-".to_string(),
        other => other.to_string(),
    }
}

/// Plain string of a JSON value (strings unquoted).
pub fn plain(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "-".to_string(),
        other => other.to_string(),
    }
}

/// Escape one CSV field (RFC 4180).
pub fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exactly10!", 10), "exactly10!");
        assert_eq!(truncate("abcdefghijk", 5), "abcd…");
        // Multi-byte characters never split (byte slicing would panic here)
        assert_eq!(truncate("Température élevée", 6), "Tempé…");
        assert_eq!(truncate("温度过高警报温度过高", 4), "温度过…");
        assert_eq!(truncate("🔥🔥🔥🔥", 3), "🔥🔥…");
        assert_eq!(truncate("abc", 0), "");
        assert_eq!(truncate("abc", 1), "…");
    }

    #[test]
    fn table_pads_by_chars() {
        let mut t = Table::new(&[("ID", usize::MAX), ("TITLE", 6)]);
        t.row(vec!["1".into(), "Überhitzung".into()]);
        t.row(vec!["22".into(), "ok".into()]);
        let out = t.render();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "ID  TITLE");
        assert_eq!(lines[1], "1   Überh…");
        assert_eq!(lines[2], "22  ok");
    }

    #[test]
    fn parses_hub_timestamps() {
        let rfc = parse_timestamp("2026-09-24T10:00:00+02:00").unwrap();
        assert_eq!(rfc.to_rfc3339(), "2026-09-24T08:00:00+00:00");
        let sqlite = parse_timestamp("2026-09-24 08:00:00").unwrap();
        assert_eq!(sqlite, rfc);
        assert!(parse_timestamp("yesterday").is_none());
        assert_eq!(timestamp(None), "-");
        assert_eq!(timestamp(Some("garbage")), "garbage");
    }

    #[test]
    fn relative_ages() {
        let now = parse_timestamp("2026-09-24T12:00:00Z").unwrap();
        let ago = |s: i64| relative_age(now - chrono::Duration::seconds(s), now);
        assert_eq!(ago(2), "just now");
        assert_eq!(ago(42), "42s ago");
        assert_eq!(ago(300), "5m ago");
        assert_eq!(ago(3 * 3600 + 10), "3h ago");
        assert_eq!(ago(2 * 86_400), "2d ago");
        assert_eq!(ago(-600), "in 10m");
        assert_eq!(uptime(3 * 86_400 + 4 * 3600), "3d 4h");
        assert_eq!(uptime(125), "2m 5s");
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90").unwrap().num_seconds(), 90);
        assert_eq!(parse_duration("15m").unwrap().num_seconds(), 900);
        assert_eq!(parse_duration("1h").unwrap().num_seconds(), 3600);
        assert_eq!(parse_duration("7d").unwrap().num_seconds(), 604_800);
        assert!(parse_duration("0m").is_err());
        assert!(parse_duration("1y").is_err());
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn rfc3339_args_and_csv() {
        assert_eq!(
            parse_rfc3339_arg("2026-09-24T10:00:00+02:00").unwrap(),
            "2026-09-24T08:00:00+00:00"
        );
        assert!(parse_rfc3339_arg("2026-09-24").is_err());
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(metric_value(&serde_json::json!(41.25)), "41.2");
        assert_eq!(metric_value(&serde_json::json!(3)), "3");
    }
}
