//! Updates from GitHub Releases. The app checks for a newer release, and the
//! user decides whether to install it. Installing downloads the new exe,
//! checks its SHA-256 against the one GitHub reports, swaps it in place of the
//! running exe and restarts the app and the background recorder.

use crate::{collector, db, paths};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const REPO: &str = "Josephivgi/JPerformance";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const ASSET: &str = "JPerformance.exe";

#[derive(Clone)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    asset_url: String,
    size: u64,
    sha256: String,
}

#[derive(Clone)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    /// Bytes downloaded so far and the total.
    Downloading(u64, u64),
    /// The new version is in place and has been started; this window should close.
    Restarting,
    Failed(String),
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

/// "v1.2.3" or "1.2.3" as comparable numbers.
fn parse_version(v: &str) -> Option<Vec<u64>> {
    v.trim().trim_start_matches(['v', 'V']).split('.').map(|p| p.parse().ok()).collect()
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .user_agent(concat!("JPerformance/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// The latest published release, or None if there is none yet.
fn latest() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp = match agent().get(&url).header("Accept", "application/vnd.github+json").call() {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(format!("Could not reach GitHub: {e}")),
    };
    let rel: GhRelease = resp.body_mut().read_json().map_err(|e| format!("Unexpected reply from GitHub: {e}"))?;
    let asset = rel
        .assets
        .into_iter()
        .find(|a| a.name.eq_ignore_ascii_case(ASSET))
        .ok_or_else(|| format!("Release {} has no {ASSET} to download.", rel.tag_name))?;
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| format!("Release {} has no checksum, so it was not trusted.", rel.tag_name))?;
    Ok(Some(Release {
        version: rel.tag_name.trim_start_matches(['v', 'V']).to_string(),
        notes: rel.body.unwrap_or_default().trim().to_string(),
        page: rel.html_url,
        asset_url: asset.browser_download_url,
        size: asset.size,
        sha256,
    }))
}

pub struct Updater {
    state: Arc<Mutex<State>>,
    last_check: Option<Instant>,
}

impl Updater {
    pub fn new() -> Self {
        Self { state: Arc::new(Mutex::new(State::Idle)), last_check: None }
    }

    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    fn set(state: &Mutex<State>, s: State, ctx: &eframe::egui::Context) {
        *state.lock().unwrap() = s;
        ctx.request_repaint();
    }

    fn busy(&self) -> bool {
        matches!(self.state(), State::Checking | State::Downloading(..) | State::Restarting)
    }

    /// Checks at launch and then every 6 hours.
    pub fn tick(&mut self, ctx: &eframe::egui::Context) {
        if self.last_check.is_none_or(|t| t.elapsed() >= Duration::from_secs(6 * 3600))
            && !matches!(self.state(), State::Available(_))
        {
            self.check(ctx);
        }
    }

    pub fn check(&mut self, ctx: &eframe::egui::Context) {
        if self.busy() {
            return;
        }
        self.last_check = Some(Instant::now());
        Self::set(&self.state, State::Checking, ctx);
        let (state, ctx) = (self.state.clone(), ctx.clone());
        std::thread::spawn(move || {
            let s = match latest() {
                Ok(Some(r)) if is_newer(&r.version, CURRENT) => State::Available(r),
                Ok(_) => State::UpToDate,
                Err(e) => {
                    collector::log(&format!("update check failed: {e}"));
                    State::Failed(e)
                }
            };
            Self::set(&state, s, &ctx);
        });
    }

    /// Downloads, verifies and installs `rel`, then starts the new version.
    pub fn install(&mut self, rel: Release, ctx: &eframe::egui::Context) {
        if self.busy() {
            return;
        }
        Self::set(&self.state, State::Downloading(0, rel.size), ctx);
        let (state, ctx) = (self.state.clone(), ctx.clone());
        std::thread::spawn(move || {
            let progress = |done, total| Self::set(&state, State::Downloading(done, total), &ctx);
            let s = match download(&rel, progress).and_then(|file| swap_and_restart(&file)) {
                Ok(()) => {
                    collector::log(&format!("updated to {}", rel.version));
                    State::Restarting
                }
                Err(e) => {
                    collector::log(&format!("update to {} failed: {e}", rel.version));
                    State::Failed(e)
                }
            };
            Self::set(&state, s, &ctx);
        });
    }
}

/// `--check-update` prints whether a newer release exists; `--update` also installs it.
pub fn cli(install: bool) -> Result<(), String> {
    println!("JPerformance {CURRENT}");
    let rel = match latest()? {
        Some(r) if is_newer(&r.version, CURRENT) => r,
        Some(r) => {
            println!("Up to date (latest release is {}).", r.version);
            return Ok(());
        }
        None => {
            println!("No releases published yet.");
            return Ok(());
        }
    };
    println!("Version {} is available: {}", rel.version, rel.page);
    if !install {
        return Ok(());
    }
    let file = download(&rel, |_, _| {})?;
    println!("Downloaded and verified {} bytes, sha256 {}", rel.size, rel.sha256);
    swap_and_restart(&file)?;
    println!("Installed {}. JPerformance is restarting.", rel.version);
    Ok(())
}

fn update_dir() -> PathBuf {
    let dir = paths::data_dir().join("update");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn download(rel: &Release, progress: impl Fn(u64, u64)) -> Result<PathBuf, String> {
    let fail = |e: &dyn std::fmt::Display| format!("Download failed: {e}");
    let mut resp = agent().get(&rel.asset_url).call().map_err(|e| fail(&e))?;
    let path = update_dir().join(format!("JPerformance-{}.exe", rel.version));
    let mut file = std::fs::File::create(&path).map_err(|e| fail(&e))?;
    let mut reader = resp.body_mut().with_config().limit(rel.size.max(1) + 1).reader();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let mut last_report = Instant::now();
    loop {
        let n = reader.read(&mut buf).map_err(|e| fail(&e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| fail(&e))?;
        done += n as u64;
        if last_report.elapsed() >= Duration::from_millis(100) {
            last_report = Instant::now();
            progress(done, rel.size);
        }
    }
    file.flush().map_err(|e| fail(&e))?;
    drop(file);
    let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if done != rel.size || got != rel.sha256 {
        let _ = std::fs::remove_file(&path);
        return Err("The download did not match the release checksum, so it was not installed.".into());
    }
    progress(done, rel.size);
    Ok(path)
}

fn old_exe(exe: &Path) -> PathBuf {
    exe.with_file_name("JPerformance.old.exe")
}

/// Stops the background recorder so it restarts from the new exe.
fn stop_collector() {
    let Ok(conn) = db::open(&paths::db_path()) else { return };
    let _ = db::set_setting(&conn, db::KEY_TRACKING, "0");
    // The recorder checks this setting every 5 seconds.
    let deadline = Instant::now() + Duration::from_secs(12);
    while collector::is_running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
    }
    if collector::is_running() {
        if let Ok(Some(pid)) = db::collector_pid(&conn) {
            let mut sys = sysinfo::System::new();
            let pid = sysinfo::Pid::from_u32(pid);
            sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
            if let Some(p) = sys.process(pid) {
                p.kill();
            }
        }
    }
    let _ = db::set_setting(&conn, db::KEY_TRACKING, "1");
}

fn swap_and_restart(new_exe: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = old_exe(&exe);
    stop_collector();
    let _ = std::fs::remove_file(&old);
    // Windows lets a running exe be renamed but not overwritten.
    std::fs::rename(&exe, &old).map_err(|e| {
        let _ = collector::spawn_detached();
        format!("Could not replace {}: {e}. Try moving JPerformance to a folder you can write to.", exe.display())
    })?;
    if let Err(e) = std::fs::copy(new_exe, &exe) {
        let _ = std::fs::rename(&old, &exe);
        let _ = collector::spawn_detached();
        return Err(format!("Could not install the new version: {e}"));
    }
    let _ = std::fs::remove_file(new_exe);
    std::process::Command::new(&exe)
        .creation_flags(DETACHED_PROCESS)
        .spawn()
        .map_err(|e| format!("Installed, but could not start the new version: {e}. Open JPerformance again."))?;
    Ok(())
}

/// Removes the exe left behind by the last update. It may still be running
/// for a moment while the old window closes, so this retries briefly.
pub fn cleanup_after_update() {
    let Ok(exe) = std::env::current_exe() else { return };
    let old = old_exe(&exe);
    if !old.exists() {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..30 {
            if std::fs::remove_file(&old).is_ok() || !old.exists() {
                let _ = std::fs::remove_dir_all(update_dir());
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_versions_by_number() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }
}
