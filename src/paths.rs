use std::path::PathBuf;

fn appdata() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// `%APPDATA%\JPerformance`, created on first use.
pub fn data_dir() -> PathBuf {
    let dir = appdata().join("JPerformance");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn db_path() -> PathBuf {
    data_dir().join("jperformance.db")
}

pub fn lock_path() -> PathBuf {
    data_dir().join("collector.lock")
}

/// Copies history recorded under the app's old name (PerfLens) into the new
/// database the first time JPerformance runs. The old folder is left untouched.
pub fn migrate_legacy() {
    let new = db_path();
    let old = appdata().join("PerfLens").join("perflens.db");
    if new.exists() || !old.exists() {
        return;
    }
    let result = rusqlite::Connection::open(&old)
        .and_then(|c| c.execute("VACUUM INTO ?1", [new.to_string_lossy().as_ref()]).map(|_| ()));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&new); // drop a partial copy so the next start retries
        crate::collector::log(&format!("could not copy PerfLens history: {e}"));
    } else {
        crate::collector::log("copied history from PerfLens");
    }
}
