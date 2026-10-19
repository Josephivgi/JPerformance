use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

pub type Result<T> = rusqlite::Result<T>;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS system_samples (
    ts          INTEGER PRIMARY KEY,
    cpu         REAL    NOT NULL,
    mem_used    INTEGER NOT NULL,
    mem_total   INTEGER NOT NULL,
    io_read     INTEGER NOT NULL,
    io_write    INTEGER NOT NULL,
    net_rx      INTEGER NOT NULL,
    net_tx      INTEGER NOT NULL,
    interval_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS app_minutes (
    minute   INTEGER NOT NULL,
    app      TEXT    NOT NULL,
    cpu_avg  REAL    NOT NULL,
    cpu_max  REAL    NOT NULL,
    mem_avg  INTEGER NOT NULL,
    mem_max  INTEGER NOT NULL,
    io_bytes INTEGER NOT NULL,
    procs    INTEGER NOT NULL,
    PRIMARY KEY (minute, app)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS app_minutes_app ON app_minutes(app, minute);
CREATE TABLE IF NOT EXISTS apps (
    app         TEXT PRIMARY KEY,
    exe_path    TEXT,
    description TEXT,
    last_seen   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS live_apps (
    app      TEXT PRIMARY KEY,
    cpu      REAL    NOT NULL,
    mem      INTEGER NOT NULL,
    io_rate  REAL    NOT NULL,
    procs    INTEGER NOT NULL,
    ts       INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS collector_status (
    id        INTEGER PRIMARY KEY CHECK (id = 1),
    pid       INTEGER NOT NULL,
    started   INTEGER NOT NULL,
    heartbeat INTEGER NOT NULL
);
"#;

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

// ---------- settings ----------

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .optional()
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn setting_i64(conn: &Connection, key: &str, default: i64) -> i64 {
    get_setting(conn, key)
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

pub const KEY_TRACKING: &str = "tracking_enabled";
pub const KEY_INTERVAL: &str = "sample_interval_secs";
pub const KEY_RETENTION: &str = "retention_days";

pub const DEFAULT_INTERVAL: i64 = 5;
pub const DEFAULT_RETENTION: i64 = 30;

pub fn tracking_enabled(conn: &Connection) -> bool {
    setting_i64(conn, KEY_TRACKING, 1) != 0
}

// ---------- collector writes ----------

pub struct SystemSample {
    pub ts: i64,
    pub cpu: f64,
    pub mem_used: u64,
    pub mem_total: u64,
    pub io_read: u64,
    pub io_write: u64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub interval_ms: i64,
}

pub struct AppMinute {
    pub app: String,
    pub cpu_avg: f64,
    pub cpu_max: f64,
    pub mem_avg: u64,
    pub mem_max: u64,
    pub io_bytes: u64,
    pub procs: i64,
}

pub struct LiveApp {
    pub app: String,
    pub cpu: f64,
    pub mem: u64,
    pub io_rate: f64,
    pub procs: i64,
}

pub fn write_sample(conn: &mut Connection, s: &SystemSample, live: &[LiveApp]) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO system_samples
         (ts, cpu, mem_used, mem_total, io_read, io_write, net_rx, net_tx, interval_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            s.ts,
            s.cpu,
            s.mem_used as i64,
            s.mem_total as i64,
            s.io_read as i64,
            s.io_write as i64,
            s.net_rx as i64,
            s.net_tx as i64,
            s.interval_ms
        ],
    )?;
    tx.execute("DELETE FROM live_apps", [])?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO live_apps(app, cpu, mem, io_rate, procs, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for a in live {
            stmt.execute(params![a.app, a.cpu, a.mem as i64, a.io_rate, a.procs, s.ts])?;
        }
    }
    tx.execute(
        "UPDATE collector_status SET heartbeat = ?1 WHERE id = 1",
        [s.ts],
    )?;
    tx.commit()
}

pub fn write_minute(conn: &mut Connection, minute: i64, rows: &[AppMinute]) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT OR REPLACE INTO app_minutes
             (minute, app, cpu_avg, cpu_max, mem_avg, mem_max, io_bytes, procs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for r in rows {
            stmt.execute(params![
                minute,
                r.app,
                r.cpu_avg,
                r.cpu_max,
                r.mem_avg as i64,
                r.mem_max as i64,
                r.io_bytes as i64,
                r.procs
            ])?;
        }
    }
    tx.commit()
}

pub fn upsert_app(conn: &Connection, app: &str, exe: Option<&str>, desc: Option<&str>, ts: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO apps(app, exe_path, description, last_seen) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(app) DO UPDATE SET
            exe_path = COALESCE(excluded.exe_path, apps.exe_path),
            description = COALESCE(excluded.description, apps.description),
            last_seen = excluded.last_seen",
        params![app, exe, desc, ts],
    )?;
    Ok(())
}

pub fn mark_collector_started(conn: &Connection, pid: u32, ts: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO collector_status(id, pid, started, heartbeat) VALUES (1, ?1, ?2, ?2)
         ON CONFLICT(id) DO UPDATE SET pid = excluded.pid, started = excluded.started, heartbeat = excluded.heartbeat",
        params![pid as i64, ts],
    )?;
    Ok(())
}

pub fn prune(conn: &Connection, older_than: i64) -> Result<usize> {
    let a = conn.execute("DELETE FROM system_samples WHERE ts < ?1", [older_than])?;
    let b = conn.execute("DELETE FROM app_minutes WHERE minute < ?1", [older_than])?;
    Ok(a + b)
}

pub fn clear_history(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "BEGIN;
         DELETE FROM system_samples;
         DELETE FROM app_minutes;
         DELETE FROM live_apps;
         COMMIT;
         VACUUM;",
    )
}

// ---------- reads ----------

#[derive(Clone, Debug)]
pub struct SysPoint {
    pub ts: i64,
    pub cpu: f64,
    pub cpu_max: f64,
    pub mem_pct: f64,
    pub mem_used: f64,
    /// bytes per second
    pub io_rate: f64,
    /// bytes per second
    pub net_rate: f64,
}

/// System samples between `from` and `to`, averaged into `bucket`-second buckets.
pub fn system_series(conn: &Connection, from: i64, to: i64, bucket: i64) -> Result<Vec<SysPoint>> {
    let bucket = bucket.max(1);
    let mut stmt = conn.prepare(
        "SELECT (ts / ?3) * ?3 AS b,
                AVG(cpu), MAX(cpu),
                AVG(100.0 * mem_used / mem_total), AVG(mem_used),
                1000.0 * SUM(io_read + io_write) / SUM(interval_ms),
                1000.0 * SUM(net_rx + net_tx) / SUM(interval_ms)
         FROM system_samples
         WHERE ts >= ?1 AND ts <= ?2 AND mem_total > 0 AND interval_ms > 0
         GROUP BY b ORDER BY b",
    )?;
    let rows = stmt.query_map(params![from, to, bucket], |r| {
        Ok(SysPoint {
            ts: r.get(0)?,
            cpu: r.get(1)?,
            cpu_max: r.get(2)?,
            mem_pct: r.get(3)?,
            mem_used: r.get(4)?,
            io_rate: r.get(5)?,
            net_rate: r.get(6)?,
        })
    })?;
    rows.collect()
}

pub fn latest_sample(conn: &Connection) -> Result<Option<(SysPoint, u64)>> {
    conn.query_row(
        "SELECT ts, cpu, 100.0 * mem_used / mem_total, mem_used, mem_total,
                1000.0 * (io_read + io_write) / interval_ms,
                1000.0 * (net_rx + net_tx) / interval_ms
         FROM system_samples WHERE mem_total > 0 AND interval_ms > 0
         ORDER BY ts DESC LIMIT 1",
        [],
        |r| {
            let cpu: f64 = r.get(1)?;
            let mem_used: i64 = r.get(3)?;
            let mem_total: i64 = r.get(4)?;
            Ok((
                SysPoint {
                    ts: r.get(0)?,
                    cpu,
                    cpu_max: cpu,
                    mem_pct: r.get(2)?,
                    mem_used: mem_used as f64,
                    io_rate: r.get(5)?,
                    net_rate: r.get(6)?,
                },
                mem_total as u64,
            ))
        },
    )
    .optional()
}

#[derive(Clone, Debug)]
pub struct LiveRow {
    pub app: String,
    pub description: Option<String>,
    pub cpu: f64,
    pub mem: u64,
    pub io_rate: f64,
    pub procs: i64,
}

pub fn live_apps(conn: &Connection) -> Result<Vec<LiveRow>> {
    let mut stmt = conn.prepare(
        "SELECT l.app, a.description, l.cpu, l.mem, l.io_rate, l.procs
         FROM live_apps l LEFT JOIN apps a ON a.app = l.app
         ORDER BY l.cpu DESC, l.mem DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(LiveRow {
            app: r.get(0)?,
            description: r.get(1)?,
            cpu: r.get(2)?,
            mem: r.get::<_, i64>(3)? as u64,
            io_rate: r.get(4)?,
            procs: r.get(5)?,
        })
    })?;
    rows.collect()
}

#[derive(Clone, Debug)]
pub struct AppSummary {
    pub app: String,
    pub description: Option<String>,
    /// average share of the whole machine's CPU over the minutes it was running
    pub cpu_avg: f64,
    pub cpu_peak: f64,
    /// CPU-minutes of one full machine, used to rank total cost
    pub cpu_load: f64,
    pub mem_avg: f64,
    pub mem_peak: u64,
    pub io_total: u64,
    pub minutes: i64,
}

pub fn app_summaries(conn: &Connection, from: i64, to: i64) -> Result<Vec<AppSummary>> {
    let mut stmt = conn.prepare(
        "SELECT m.app, a.description, AVG(m.cpu_avg), MAX(m.cpu_max), SUM(m.cpu_avg) / 100.0,
                AVG(m.mem_avg), MAX(m.mem_max), SUM(m.io_bytes), COUNT(*)
         FROM app_minutes m LEFT JOIN apps a ON a.app = m.app
         WHERE m.minute >= ?1 AND m.minute <= ?2
         GROUP BY m.app
         ORDER BY SUM(m.cpu_avg) DESC",
    )?;
    let rows = stmt.query_map(params![from, to], |r| {
        Ok(AppSummary {
            app: r.get(0)?,
            description: r.get(1)?,
            cpu_avg: r.get(2)?,
            cpu_peak: r.get(3)?,
            cpu_load: r.get(4)?,
            mem_avg: r.get(5)?,
            mem_peak: r.get::<_, i64>(6)? as u64,
            io_total: r.get::<_, i64>(7)? as u64,
            minutes: r.get(8)?,
        })
    })?;
    rows.collect()
}

#[derive(Clone, Debug)]
pub struct AppPoint {
    pub ts: i64,
    pub cpu: f64,
    pub mem: f64,
}

pub fn app_series(conn: &Connection, app: &str, from: i64, to: i64, bucket: i64) -> Result<Vec<AppPoint>> {
    let bucket = bucket.max(60);
    let mut stmt = conn.prepare(
        "SELECT (minute / ?4) * ?4 AS b, AVG(cpu_avg), AVG(mem_avg)
         FROM app_minutes WHERE app = ?1 AND minute >= ?2 AND minute <= ?3
         GROUP BY b ORDER BY b",
    )?;
    let rows = stmt.query_map(params![app, from, to, bucket], |r| {
        Ok(AppPoint { ts: r.get(0)?, cpu: r.get(1)?, mem: r.get(2)? })
    })?;
    rows.collect()
}

/// Top apps by CPU between `from` and `to` (inclusive).
pub fn top_apps_in(conn: &Connection, from: i64, to: i64, n: usize) -> Result<Vec<(String, Option<String>, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT m.app, a.description, SUM(m.cpu_avg) / MAX(1, (SELECT COUNT(DISTINCT minute) FROM app_minutes WHERE minute >= ?1 AND minute <= ?2))
         FROM app_minutes m LEFT JOIN apps a ON a.app = m.app
         WHERE m.minute >= ?1 AND m.minute <= ?2
         GROUP BY m.app ORDER BY SUM(m.cpu_avg) DESC LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![from, to, n as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    rows.collect()
}

/// Top apps by memory between `from` and `to`.
pub fn top_mem_apps_in(conn: &Connection, from: i64, to: i64, n: usize) -> Result<Vec<(String, Option<String>, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT m.app, a.description, AVG(m.mem_avg)
         FROM app_minutes m LEFT JOIN apps a ON a.app = m.app
         WHERE m.minute >= ?1 AND m.minute <= ?2
         GROUP BY m.app ORDER BY AVG(m.mem_avg) DESC LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![from, to, n as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    rows.collect()
}

/// Average CPU by (weekday 0 = Sunday, local hour).
pub fn weekday_hour_cpu(conn: &Connection, from: i64, to: i64) -> Result<[[Option<f64>; 24]; 7]> {
    let mut grid = [[None; 24]; 7];
    let mut stmt = conn.prepare(
        "SELECT CAST(strftime('%w', ts, 'unixepoch', 'localtime') AS INTEGER) AS d,
                CAST(strftime('%H', ts, 'unixepoch', 'localtime') AS INTEGER) AS h,
                AVG(cpu)
         FROM system_samples WHERE ts >= ?1 AND ts <= ?2
         GROUP BY d, h",
    )?;
    let rows = stmt.query_map(params![from, to], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, f64>(2)?))
    })?;
    for row in rows {
        let (d, h, v) = row?;
        if (0..7).contains(&d) && (0..24).contains(&h) {
            grid[d as usize][h as usize] = Some(v);
        }
    }
    Ok(grid)
}

pub struct Coverage {
    pub first: Option<i64>,
    pub last: Option<i64>,
    pub samples: i64,
}

pub fn coverage(conn: &Connection, from: i64, to: i64) -> Result<Coverage> {
    conn.query_row(
        "SELECT MIN(ts), MAX(ts), COUNT(*) FROM system_samples WHERE ts >= ?1 AND ts <= ?2",
        params![from, to],
        |r| Ok(Coverage { first: r.get(0)?, last: r.get(1)?, samples: r.get(2)? }),
    )
}

pub struct CollectorStatus {
    pub pid: i64,
    pub started: i64,
    pub heartbeat: i64,
}

pub fn collector_status(conn: &Connection) -> Result<Option<CollectorStatus>> {
    conn.query_row(
        "SELECT pid, started, heartbeat FROM collector_status WHERE id = 1",
        [],
        |r| Ok(CollectorStatus { pid: r.get(0)?, started: r.get(1)?, heartbeat: r.get(2)? }),
    )
    .optional()
}

pub fn db_size_bytes(path: &Path) -> u64 {
    let mut total = 0;
    for suffix in ["", "-wal", "-shm"] {
        let p = std::path::PathBuf::from(format!("{}{}", path.display(), suffix));
        total += std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    }
    total
}
