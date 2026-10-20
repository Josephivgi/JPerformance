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
    ts       INTEGER NOT NULL,
    last_active INTEGER NOT NULL DEFAULT 0,
    titles   TEXT
);
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    id         INTEGER PRIMARY KEY,
    app        TEXT    NOT NULL,
    started    INTEGER NOT NULL,
    ended      INTEGER,
    end_reason TEXT,
    planned_end INTEGER
);
CREATE TABLE IF NOT EXISTS session_samples (
    session_id INTEGER NOT NULL,
    ts         INTEGER NOT NULL,
    cpu        REAL    NOT NULL,
    mem        INTEGER NOT NULL,
    io_rate    REAL    NOT NULL,
    procs      INTEGER NOT NULL,
    PRIMARY KEY (session_id, ts)
) WITHOUT ROWID;
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
    migrate(&conn)?;
    Ok(conn)
}

/// Adds columns introduced after the first release.
fn migrate(conn: &Connection) -> Result<()> {
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('system_samples')")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_>>()?;
    // live_apps only holds the latest sample, so it can be rebuilt freely.
    let live_ok = conn
        .prepare("SELECT 1 FROM pragma_table_info('live_apps') WHERE name = 'titles'")?
        .exists([])?;
    if !live_ok {
        conn.execute_batch(
            "DROP TABLE live_apps;
             CREATE TABLE live_apps (
                app TEXT PRIMARY KEY, cpu REAL NOT NULL, mem INTEGER NOT NULL, io_rate REAL NOT NULL,
                procs INTEGER NOT NULL, ts INTEGER NOT NULL, last_active INTEGER NOT NULL DEFAULT 0, titles TEXT);",
        )?;
    }
    let has_title = conn
        .prepare("SELECT 1 FROM pragma_table_info('apps') WHERE name = 'window_title'")?
        .exists([])?;
    if !has_title {
        conn.execute_batch("ALTER TABLE apps ADD COLUMN window_title TEXT")?;
    }
    // Apps that run on a shared runtime (Electron, Java, Python...) all have the
    // runtime's description, so their taskbar window title names them better.
    conn.execute_batch(
        "DROP VIEW IF EXISTS app_names;
         CREATE VIEW app_names AS
         SELECT app, exe_path, window_title,
                CASE WHEN window_title IS NOT NULL AND window_title <> ''
                          AND (description IS NULL OR lower(description) IN ('electron', 'java(tm) platform se binary', 'openjdk platform binary', 'java', 'javaw', 'python', 'pythonw', 'node.js', 'node.js javascript runtime', 'nw.js', 'chromium', 'microsoft edge webview2', 'tauri app', 'cefsharp.browsersubprocess', 'qtwebengineprocess'))
                     THEN window_title ELSE description END AS description
         FROM apps;",
    )?;
    let has_planned = conn
        .prepare("SELECT 1 FROM pragma_table_info('sessions') WHERE name = 'planned_end'")?
        .exists([])?;
    if !has_planned {
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN planned_end INTEGER")?;
    }
    for col in ["proc_count", "app_count"] {
        if !cols.iter().any(|c| c == col) {
            conn.execute_batch(&format!(
                "ALTER TABLE system_samples ADD COLUMN {col} INTEGER NOT NULL DEFAULT 0"
            ))?;
        }
    }
    Ok(())
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
    pub proc_count: i64,
    pub app_count: i64,
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
    /// Taskbar window titles, joined with " | ".
    pub titles: Option<String>,
}

pub fn write_sample(conn: &mut Connection, s: &SystemSample, live: &[LiveApp]) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO system_samples
         (ts, cpu, mem_used, mem_total, io_read, io_write, net_rx, net_tx, interval_ms, proc_count, app_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            s.ts,
            s.cpu,
            s.mem_used as i64,
            s.mem_total as i64,
            s.io_read as i64,
            s.io_write as i64,
            s.net_rx as i64,
            s.net_tx as i64,
            s.interval_ms,
            s.proc_count,
            s.app_count
        ],
    )?;
    tx.execute("DELETE FROM live_apps", [])?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO live_apps(app, cpu, mem, io_rate, procs, ts, titles) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for a in live {
            stmt.execute(params![a.app, a.cpu, a.mem as i64, a.io_rate, a.procs, s.ts, a.titles])?;
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

pub fn set_window_title(conn: &Connection, app: &str, title: &str) -> Result<()> {
    conn.execute("UPDATE apps SET window_title = ?2 WHERE app = ?1", params![app, title])?;
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
    /// processes running; 0 when not recorded
    pub procs: f64,
    /// distinct apps running; 0 when not recorded
    pub apps: f64,
}

/// System samples between `from` and `to`, averaged into `bucket`-second buckets.
pub fn system_series(conn: &Connection, from: i64, to: i64, bucket: i64) -> Result<Vec<SysPoint>> {
    let bucket = bucket.max(1);
    let mut stmt = conn.prepare(
        "SELECT (ts / ?3) * ?3 AS b,
                AVG(cpu), MAX(cpu),
                AVG(100.0 * mem_used / mem_total), AVG(mem_used),
                1000.0 * SUM(io_read + io_write) / SUM(interval_ms),
                1000.0 * SUM(net_rx + net_tx) / SUM(interval_ms),
                COALESCE(AVG(NULLIF(proc_count, 0)), 0), COALESCE(AVG(NULLIF(app_count, 0)), 0)
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
            procs: r.get(7)?,
            apps: r.get(8)?,
        })
    })?;
    rows.collect()
}

pub fn latest_sample(conn: &Connection) -> Result<Option<(SysPoint, u64)>> {
    conn.query_row(
        "SELECT ts, cpu, 100.0 * mem_used / mem_total, mem_used, mem_total,
                1000.0 * (io_read + io_write) / interval_ms,
                1000.0 * (net_rx + net_tx) / interval_ms, proc_count, app_count
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
                    procs: r.get::<_, i64>(7)? as f64,
                    apps: r.get::<_, i64>(8)? as f64,
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
    pub exe_path: Option<String>,
    /// Taskbar window titles right now, joined with " | ".
    pub titles: Option<String>,
}

pub fn live_apps(conn: &Connection) -> Result<Vec<LiveRow>> {
    let mut stmt = conn.prepare(
        "SELECT l.app, a.description, l.cpu, l.mem, l.io_rate, l.procs, a.exe_path, l.titles
         FROM live_apps l LEFT JOIN app_names a ON a.app = l.app
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
            exe_path: r.get(6)?,
            titles: r.get(7)?,
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
    /// average number of processes while running
    pub procs: f64,
}

pub fn app_summaries(conn: &Connection, from: i64, to: i64) -> Result<Vec<AppSummary>> {
    let mut stmt = conn.prepare(
        "SELECT m.app, a.description, AVG(m.cpu_avg), MAX(m.cpu_max), SUM(m.cpu_avg) / 100.0,
                AVG(m.mem_avg), MAX(m.mem_max), SUM(m.io_bytes), COUNT(*), AVG(m.procs)
         FROM app_minutes m LEFT JOIN app_names a ON a.app = m.app
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
            procs: r.get(9)?,
        })
    })?;
    rows.collect()
}

#[derive(Clone, Debug)]
pub struct AppPoint {
    pub ts: i64,
    pub cpu: f64,
    pub mem: f64,
    /// bytes per second
    pub io_rate: f64,
    pub procs: f64,
}

pub fn app_series(conn: &Connection, app: &str, from: i64, to: i64, bucket: i64) -> Result<Vec<AppPoint>> {
    let bucket = bucket.max(60);
    let mut stmt = conn.prepare(
        "SELECT (minute / ?4) * ?4 AS b, AVG(cpu_avg), AVG(mem_avg),
                SUM(io_bytes) / (COUNT(*) * 60.0), AVG(procs)
         FROM app_minutes WHERE app = ?1 AND minute >= ?2 AND minute <= ?3
         GROUP BY b ORDER BY b",
    )?;
    let rows = stmt.query_map(params![app, from, to, bucket], |r| {
        Ok(AppPoint { ts: r.get(0)?, cpu: r.get(1)?, mem: r.get(2)?, io_rate: r.get(3)?, procs: r.get(4)? })
    })?;
    rows.collect()
}

/// Average CPU of one app by (weekday 0 = Sunday, local hour), counting minutes it was running.
pub fn weekday_hour_app_cpu(conn: &Connection, app: &str, from: i64, to: i64) -> Result<[[Option<f64>; 24]; 7]> {
    let mut grid = [[None; 24]; 7];
    let mut stmt = conn.prepare(
        "SELECT CAST(strftime('%w', minute, 'unixepoch', 'localtime') AS INTEGER) AS d,
                CAST(strftime('%H', minute, 'unixepoch', 'localtime') AS INTEGER) AS h,
                AVG(cpu_avg)
         FROM app_minutes WHERE app = ?1 AND minute >= ?2 AND minute <= ?3
         GROUP BY d, h",
    )?;
    let rows = stmt.query_map(params![app, from, to], |r| {
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

/// Top apps by CPU between `from` and `to` (inclusive).
pub fn top_apps_in(conn: &Connection, from: i64, to: i64, n: usize) -> Result<Vec<(String, Option<String>, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT m.app, a.description, SUM(m.cpu_avg) / MAX(1, (SELECT COUNT(DISTINCT minute) FROM app_minutes WHERE minute >= ?1 AND minute <= ?2))
         FROM app_minutes m LEFT JOIN app_names a ON a.app = m.app
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
         FROM app_minutes m LEFT JOIN app_names a ON a.app = m.app
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

/// Number of minutes with per-app data between `from` and `to`.
pub fn minutes_recorded(conn: &Connection, from: i64, to: i64) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(DISTINCT minute) FROM app_minutes WHERE minute >= ?1 AND minute <= ?2",
        params![from, to],
        |r| r.get(0),
    )
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
    pub heartbeat: i64,
}

pub fn collector_status(conn: &Connection) -> Result<Option<CollectorStatus>> {
    conn.query_row(
        "SELECT heartbeat FROM collector_status WHERE id = 1",
        [],
        |r| Ok(CollectorStatus { heartbeat: r.get(0)? }),
    )
    .optional()
}

// ---------- app recordings ("Track an app") ----------

/// Starts recording `app`. With `minutes`, the recording stops by itself after that long.
pub fn start_session(conn: &Connection, app: &str, minutes: Option<i64>) -> Result<i64> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO sessions(app, started, planned_end) VALUES (?1, ?2, ?3)",
        params![app, now, minutes.map(|m| now + m * 60)],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn end_session(conn: &Connection, id: i64, reason: &str) -> Result<()> {
    conn.execute(
        "UPDATE sessions SET ended = ?2, end_reason = ?3 WHERE id = ?1 AND ended IS NULL",
        params![id, chrono::Utc::now().timestamp(), reason],
    )?;
    Ok(())
}

pub fn delete_session(conn: &Connection, id: i64) -> Result<()> {
    conn.execute_batch("BEGIN")?;
    let r = (|| {
        conn.execute("DELETE FROM session_samples WHERE session_id = ?1", [id])?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
        Ok(())
    })();
    conn.execute_batch(if r.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
    r
}

/// (id, app, planned end) of recordings still running.
pub fn active_sessions(conn: &Connection) -> Result<Vec<(i64, String, Option<i64>)>> {
    let mut stmt = conn.prepare("SELECT id, app, planned_end FROM sessions WHERE ended IS NULL")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    rows.collect()
}

pub struct SessionSample {
    pub ts: i64,
    pub cpu: f64,
    pub mem: u64,
    pub io_rate: f64,
    pub procs: i64,
}

pub fn write_session_samples(conn: &mut Connection, rows: &[(i64, SessionSample)]) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT OR REPLACE INTO session_samples(session_id, ts, cpu, mem, io_rate, procs) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for (id, s) in rows {
            stmt.execute(params![id, s.ts, s.cpu, s.mem as i64, s.io_rate, s.procs])?;
        }
    }
    tx.commit()
}

#[derive(Clone, Debug)]
pub struct SessionSummary {
    pub id: i64,
    pub app: String,
    pub description: Option<String>,
    pub started: i64,
    pub ended: Option<i64>,
    pub end_reason: Option<String>,
    pub planned_end: Option<i64>,
    pub samples: i64,
    pub cpu_avg: f64,
    pub cpu_peak: f64,
    pub mem_avg: f64,
    pub mem_peak: u64,
    /// bytes read and written during the recording
    pub io_total: f64,
    pub procs_peak: i64,
}

pub fn sessions(conn: &Connection, limit: usize) -> Result<Vec<SessionSummary>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.app, a.description, s.started, s.ended, s.end_reason, s.planned_end,
                COUNT(x.ts), COALESCE(AVG(x.cpu), 0), COALESCE(MAX(x.cpu), 0),
                COALESCE(AVG(x.mem), 0), COALESCE(MAX(x.mem), 0), COALESCE(SUM(x.io_rate), 0),
                COALESCE(MAX(x.procs), 0)
         FROM sessions s
         LEFT JOIN app_names a ON a.app = s.app
         LEFT JOIN session_samples x ON x.session_id = s.id
         GROUP BY s.id ORDER BY (s.ended IS NULL) DESC, s.started DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| {
        Ok(SessionSummary {
            id: r.get(0)?,
            app: r.get(1)?,
            description: r.get(2)?,
            started: r.get(3)?,
            ended: r.get(4)?,
            end_reason: r.get(5)?,
            planned_end: r.get(6)?,
            samples: r.get(7)?,
            cpu_avg: r.get(8)?,
            cpu_peak: r.get(9)?,
            mem_avg: r.get(10)?,
            mem_peak: r.get::<_, i64>(11)? as u64,
            io_total: r.get(12)?,
            procs_peak: r.get(13)?,
        })
    })?;
    rows.collect()
}

/// Samples of one recording, averaged into `bucket`-second buckets.
pub fn session_series(conn: &Connection, id: i64, bucket: i64) -> Result<Vec<AppPoint>> {
    let mut stmt = conn.prepare(
        "SELECT (ts / ?2) * ?2 AS b, AVG(cpu), AVG(mem), AVG(io_rate), AVG(procs)
         FROM session_samples WHERE session_id = ?1 GROUP BY b ORDER BY b",
    )?;
    let rows = stmt.query_map(params![id, bucket.max(1)], |r| {
        Ok(AppPoint { ts: r.get(0)?, cpu: r.get(1)?, mem: r.get(2)?, io_rate: r.get(3)?, procs: r.get(4)? })
    })?;
    rows.collect()
}
