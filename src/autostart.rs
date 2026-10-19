//! "Start tracking when I sign in" via the per-user Run key.

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const NAME: &str = "PerfLens";

fn command() -> std::io::Result<String> {
    Ok(format!("\"{}\" --collector", std::env::current_exe()?.display()))
}

pub fn is_enabled() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN, KEY_READ)
        .and_then(|k| k.get_value::<String, _>(NAME))
        .is_ok()
}

pub fn set_enabled(on: bool) -> std::io::Result<()> {
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN, KEY_READ | KEY_WRITE)?;
    if on {
        key.set_value(NAME, &command()?)
    } else {
        match key.delete_value(NAME) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
}

/// Keeps the Run entry pointing at this exe if the app was moved.
pub fn refresh_path() {
    if is_enabled() {
        let _ = set_enabled(true);
    }
}
