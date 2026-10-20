//! Records apps the user chose to track ("Track an app"), once a second.
//! Runs as a thread inside the background collector so recordings continue
//! when the window is closed.

use crate::{collector, db, paths};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

/// A recording ends on its own once the app has been gone this long.
const GONE_SECS: u64 = 10;

pub fn spawn() {
    std::thread::spawn(|| {
        if let Err(e) = run() {
            collector::log(&format!("recorder stopped: {e}"));
        }
    });
}

fn run() -> db::Result<()> {
    let mut conn = db::open(&paths::db_path())?;
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    let kind = ProcessRefreshKind::nothing().with_cpu().with_memory().with_disk_usage();
    let mut sys = System::new();
    let mut warm = false;
    let mut last = Instant::now();
    let mut gone_since: HashMap<i64, Instant> = HashMap::new();

    loop {
        std::thread::sleep(Duration::from_secs(1).saturating_sub(last.elapsed()));
        let active = match db::active_sessions(&conn) {
            Ok(a) => a,
            Err(e) => {
                collector::log(&format!("recorder: could not read recordings: {e}"));
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
        };
        if active.is_empty() {
            warm = false;
            gone_since.clear();
            last = Instant::now();
            continue;
        }

        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        let elapsed = last.elapsed().as_secs_f64().max(0.001);
        last = Instant::now();
        if !warm {
            // CPU usage needs two refreshes to be meaningful.
            warm = true;
            continue;
        }

        let ts = chrono::Utc::now().timestamp();
        let mut rows = Vec::new();
        for (id, app, planned_end) in &active {
            if planned_end.is_some_and(|end| ts >= end) {
                let _ = db::end_session(&conn, *id, "time up");
                continue;
            }
            let mut s = db::SessionSample { ts, cpu: 0.0, mem: 0, io_rate: 0.0, procs: 0 };
            for p in sys.processes().values().filter(|p| p.name().to_string_lossy() == app.as_str()) {
                let du = p.disk_usage();
                s.cpu += p.cpu_usage() as f64 / cores;
                s.mem += p.memory();
                s.io_rate += (du.read_bytes + du.written_bytes) as f64 / elapsed;
                s.procs += 1;
            }
            if s.procs == 0 {
                let since = *gone_since.entry(*id).or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_secs(GONE_SECS) {
                    let _ = db::end_session(&conn, *id, "app closed");
                    gone_since.remove(id);
                }
                continue;
            }
            gone_since.remove(id);
            rows.push((*id, s));
        }
        if !rows.is_empty() {
            if let Err(e) = db::write_session_samples(&mut conn, &rows) {
                collector::log(&format!("recorder: could not save samples: {e}"));
            }
        }
    }
}
