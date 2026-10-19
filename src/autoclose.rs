//! Closes apps the user chose once they have gone unused for a while.
//!
//! An app is "unused" when it has not been the app in front and has stayed
//! under 1% of one CPU core for the chosen number of minutes. Apps with open
//! windows are only asked to close (like clicking X) and only when the rule
//! allows it; background apps without windows are ended.

use crate::{collector, db};
use std::collections::HashMap;
use sysinfo::System;
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetForegroundWindow, GetWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
    WM_CLOSE,
};

/// Processes Windows needs, or that would break the desktop if ended.
const PROTECTED: &[&str] = &[
    "system", "registry", "idle", "system idle process", "memory compression", "smss", "csrss", "wininit",
    "winlogon", "services", "lsass", "lsaiso", "svchost", "dwm", "explorer", "fontdrvhost", "sihost", "ctfmon",
    "taskhostw", "conhost", "dllhost", "runtimebroker", "applicationframehost", "shellexperiencehost",
    "startmenuexperiencehost", "searchhost", "searchindexer", "textinputhost", "spoolsv", "audiodg", "msmpeng",
    "mpdefendercoreservice", "nissrv", "securityhealthservice", "securityhealthsystray", "smartscreen", "wudfhost",
    "wmiprvse", "jperformance",
];

/// True for apps that may never be closed automatically.
pub fn is_protected(app: &str, exe_path: Option<&str>) -> bool {
    let name = app.to_ascii_lowercase();
    let name = name.trim_end_matches(".exe");
    if PROTECTED.contains(&name) {
        return true;
    }
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()).to_ascii_lowercase();
    exe_path.is_some_and(|p| p.to_ascii_lowercase().starts_with(&format!("{windir}\\")))
}

/// Visible top-level windows grouped by process id.
pub fn visible_windows() -> HashMap<u32, Vec<HWND>> {
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> i32 {
        unsafe {
            let out = &mut *(lparam as *mut HashMap<u32, Vec<HWND>>);
            if IsWindowVisible(hwnd) != 0 && GetWindow(hwnd, GW_OWNER).is_null() {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, &mut pid);
                out.entry(pid).or_default().push(hwnd);
            }
        }
        1
    }
    let mut out: HashMap<u32, Vec<HWND>> = HashMap::new();
    unsafe {
        EnumWindows(Some(cb), &mut out as *mut _ as LPARAM);
    }
    out
}

/// Process id of the window the user is using.
pub fn foreground_pid() -> Option<u32> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        (pid != 0).then_some(pid)
    }
}

/// Applies the user's rules. `last_active` maps app name to the last time it was in front or busy.
pub fn run(conn: &rusqlite::Connection, sys: &System, last_active: &mut HashMap<String, i64>, foreground: Option<&str>, now: i64) {
    if !db::autoclose_enabled(conn) {
        return;
    }
    let rules = match db::close_rules(conn) {
        Ok(r) if !r.is_empty() => r,
        Ok(_) => return,
        Err(e) => {
            collector::log(&format!("auto close: could not read rules: {e}"));
            return;
        }
    };
    let windows = visible_windows();
    for rule in rules {
        if foreground == Some(rule.app.as_str()) {
            continue;
        }
        let Some(&since) = last_active.get(&rule.app) else { continue };
        if now - since < rule.idle_minutes * 60 {
            continue;
        }
        let procs: Vec<&sysinfo::Process> = sys
            .processes()
            .values()
            .filter(|p| p.name().to_string_lossy() == rule.app.as_str())
            .collect();
        if procs.is_empty() {
            continue;
        }
        let exe = procs.iter().find_map(|p| p.exe()).map(|e| e.display().to_string());
        if is_protected(&rule.app, exe.as_deref()) {
            continue;
        }
        let has_window = procs.iter().any(|p| windows.contains_key(&p.pid().as_u32()));
        if has_window && !rule.windowed {
            continue;
        }
        let mem: u64 = procs.iter().map(|p| p.memory()).sum();
        let (count, method) = if has_window {
            let mut n = 0;
            for p in &procs {
                for &hwnd in windows.get(&p.pid().as_u32()).into_iter().flatten() {
                    unsafe {
                        PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    }
                    n += 1;
                }
            }
            (n, "asked to close")
        } else {
            (procs.iter().filter(|p| p.kill()).count() as i64, "ended")
        };
        let method = if count == 0 { "could not close (it may need admin rights)" } else { method };
        collector::log(&format!("auto close: {} {method} after {} min unused", rule.app, (now - since) / 60));
        if let Err(e) = db::log_close(conn, &rule.app, procs.len() as i64, if count == 0 { 0 } else { mem }, method) {
            collector::log(&format!("auto close: could not log: {e}"));
        }
        // Wait a full idle period before trying this app again.
        last_active.insert(rule.app.clone(), now);
    }
}

#[cfg(test)]
mod tests {
    use super::is_protected;

    #[test]
    fn windows_processes_are_protected() {
        assert!(is_protected("svchost.exe", None));
        assert!(is_protected("Explorer.EXE", None));
        assert!(is_protected("jperformance.exe", None));
        assert!(is_protected("notepad.exe", Some(r"C:\Windows\System32\notepad.exe")));
    }

    #[test]
    fn user_apps_are_not_protected() {
        assert!(!is_protected("Discord.exe", Some(r"C:\Users\Joe\AppData\Local\Discord\app\Discord.exe")));
        assert!(!is_protected("spotify.exe", None));
    }
}
