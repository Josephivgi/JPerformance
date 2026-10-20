#![windows_subsystem = "windows"]

mod autostart;
mod collector;
mod db;
mod describe;
mod fmt;
mod insights;
mod paths;
mod procs;
mod recorder;
mod theme;
mod ui;
mod update;

fn attach_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// `JPerformance.exe --report [hours]` prints the same findings the Insights page shows.
fn report(hours: i64) -> Result<(), String> {
    attach_console();
    let conn = db::open(&paths::db_path()).map_err(|e| e.to_string())?;
    let to = chrono::Utc::now().timestamp();
    let from = to - hours * 3600;
    println!("\nJPerformance report, last {hours} h  ({})", paths::db_path().display());
    println!("Collector running: {}", collector::is_running());
    for i in insights::compute(&conn, from, to).map_err(|e| e.to_string())? {
        println!("\n[{:?}] {}\n    {}", i.severity, i.title, i.detail);
    }
    println!("\nTop apps by CPU:");
    for a in db::app_summaries(&conn, from, to).map_err(|e| e.to_string())?.iter().take(10) {
        println!(
            "  {:<34} avg {:>6}  peak {:>6}  mem avg {:>9}  active {}",
            describe::display_name(&a.app, a.description.as_deref()),
            fmt::pct(a.cpu_avg),
            fmt::pct(a.cpu_peak),
            fmt::bytes(a.mem_avg),
            fmt::duration(a.minutes * 60)
        );
    }
    Ok(())
}

fn main() {
    paths::migrate_legacy();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--collector") => {
            if let Err(e) = collector::run() {
                collector::log(&format!("collector error: {e}"));
            }
        }
        Some("--report") => {
            let hours = args.get(1).and_then(|h| h.parse().ok()).unwrap_or(24);
            if let Err(e) = report(hours) {
                eprintln!("report failed: {e}");
                std::process::exit(1);
            }
        }
        Some("--stop") => {
            attach_console();
            let Some(app) = args.get(1) else {
                eprintln!("usage: JPerformance.exe --stop <app.exe> [--force]");
                std::process::exit(2);
            };
            let force = args.get(2).is_some_and(|a| a == "--force");
            let msg = match procs::stop(app, force) {
                procs::StopResult::AskedToClose => "asked to close".to_string(),
                procs::StopResult::Ended(n) => format!("stopped {n} process(es)"),
                procs::StopResult::NotRunning => "not running".to_string(),
                procs::StopResult::Denied => "Windows refused (it may run as administrator)".to_string(),
                procs::StopResult::Protected => "part of Windows, not stopped".to_string(),
            };
            println!("{app}: {msg}");
        }
        _ => {
            if let Err(e) = ui::run() {
                collector::log(&format!("ui error: {e}"));
            }
        }
    }
}
