use chrono::{Local, TimeZone};

pub fn bytes(b: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b.max(0.0);
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{v:.0} {}", UNITS[i])
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

pub fn rate(bps: f64) -> String {
    format!("{}/s", bytes(bps))
}

pub fn pct(v: f64) -> String {
    if v < 10.0 { format!("{v:.1}%") } else { format!("{v:.0}%") }
}

pub fn local(ts: i64) -> chrono::DateTime<Local> {
    Local.timestamp_opt(ts, 0).single().unwrap_or_else(Local::now)
}

pub fn day_time(ts: i64) -> String {
    local(ts).format("%a %b %-d, %H:%M").to_string()
}

pub fn duration(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 90 {
        format!("{secs} s")
    } else if secs < 90 * 60 {
        format!("{} min", (secs + 30) / 60)
    } else if secs < 48 * 3600 {
        format!("{:.1} h", secs as f64 / 3600.0)
    } else {
        format!("{:.1} days", secs as f64 / 86_400.0)
    }
}

pub fn ago(ts: i64) -> String {
    let d = chrono::Utc::now().timestamp() - ts;
    if d < 5 { "just now".into() } else { format!("{} ago", duration(d)) }
}

/// Axis label for a timestamp, with detail based on how wide the visible span is.
pub fn axis(ts: i64, span_secs: f64) -> String {
    let t = local(ts);
    if span_secs <= 2.0 * 86_400.0 {
        t.format("%H:%M").to_string()
    } else {
        t.format("%a %-d").to_string()
    }
}
