use std::path::PathBuf;

/// `%APPDATA%\PerfLens`, created on first use.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir());
    let dir = base.join("PerfLens");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn db_path() -> PathBuf {
    data_dir().join("perflens.db")
}

pub fn lock_path() -> PathBuf {
    data_dir().join("collector.lock")
}
