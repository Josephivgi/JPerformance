//! Plain-language explanations for what a process is.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

/// What a well-known Windows process does, and why it might use resources.
pub fn known(app: &str) -> Option<&'static str> {
    let key = app.to_ascii_lowercase();
    let key = key.trim_end_matches(".exe");
    Some(match key {
        "system" => "The Windows kernel. High use usually means a driver or heavy disk activity.",
        "system idle process" | "idle" => "Not a real app. It shows how much of the CPU is free.",
        "registry" => "Holds the Windows registry in memory.",
        "memory compression" => "Windows compresses memory here when RAM is running low.",
        "svchost" => "Hosts Windows services such as updates, networking and audio. Many copies is normal.",
        "msmpeng" => "Microsoft Defender Antivirus. Spikes during scans and when new files are downloaded or installed.",
        "mpdefendercoreservice" => "Part of Microsoft Defender Antivirus.",
        "nissrv" => "Microsoft Defender network inspection.",
        "searchindexer" | "searchhost" | "searchprotocolhost" | "searchfilterhost" => {
            "Windows Search. Reads your files to build the search index, mostly after file changes."
        }
        "dwm" => "Desktop Window Manager. Draws every window on screen. Rises with animations, video and many monitors.",
        "explorer" => "File Explorer and the taskbar.",
        "csrss" | "wininit" | "winlogon" | "smss" | "services" | "lsass" => "A core Windows process. Required for Windows to run.",
        "audiodg" => "Windows audio engine. Rises when audio effects or enhancements are on.",
        "tiworker" | "trustedinstaller" | "wuauclt" | "usocoreworker" | "musnotification" => {
            "Windows Update. Heavy while installing updates, then goes quiet."
        }
        "compattelrunner" => "Windows compatibility telemetry. Runs on a schedule and can use CPU and disk for a while.",
        "wmiprvse" => "Windows Management Instrumentation. Other apps use it to ask for system info.",
        "runtimebroker" => "Manages permissions for Store apps.",
        "spoolsv" => "Print spooler.",
        "ctfmon" => "Text input and keyboard language support.",
        "sihost" | "shellexperiencehost" | "startmenuexperiencehost" | "textinputhost" => "Part of the Windows shell (Start menu, notifications, input).",
        "msedge" => "Microsoft Edge. Each tab and extension runs as its own process.",
        "msedgewebview2" => "Edge WebView2. Embedded browser used by apps such as Teams, Widgets and Outlook.",
        "chrome" => "Google Chrome. Each tab and extension runs as its own process.",
        "firefox" => "Mozilla Firefox. Tabs run in separate content processes.",
        "brave" | "opera" | "vivaldi" => "A web browser. Each tab runs as its own process.",
        "discord" => "Discord. Video, screen share and animated content raise its use.",
        "teams" | "ms-teams" => "Microsoft Teams. Calls and video raise its use.",
        "spotify" => "Spotify music player.",
        "steam" | "steamwebhelper" => "Steam. The web helper draws the store and library pages.",
        "onedrive" => "OneDrive. Uses disk and network while syncing files.",
        "code" => "Visual Studio Code. Extensions and language servers run as child processes.",
        "node" => "Node.js. Runs JavaScript tools, dev servers and some app backends.",
        "python" | "pythonw" => "Python interpreter running a script.",
        "java" | "javaw" => "Java runtime running an app (for example Minecraft or an IDE).",
        "vmmem" | "vmmemwsl" => "Memory and CPU used by a virtual machine, WSL or Docker.",
        "virtualboxvm" | "vboxheadless" => "A running VirtualBox virtual machine.",
        "nvcontainer" | "nvdisplay.container" => "NVIDIA driver services.",
        "obs64" => "OBS Studio. Recording and streaming are CPU and GPU heavy.",
        "jperformance" => "JPerformance itself.",
        _ => return None,
    })
}

/// Reads the "FileDescription" from an executable's version resource,
/// for example "Google Chrome" for chrome.exe.
pub fn file_description(path: &Path) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut handle = 0u32;
        let size = GetFileVersionInfoSizeW(wide.as_ptr(), &mut handle);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if GetFileVersionInfoW(wide.as_ptr(), 0, size, buf.as_mut_ptr().cast()) == 0 {
            return None;
        }

        // Find the first language/codepage pair.
        let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let sub: Vec<u16> = "\\VarFileInfo\\Translation".encode_utf16().chain(Some(0)).collect();
        let mut candidates = Vec::new();
        if VerQueryValueW(buf.as_ptr().cast(), sub.as_ptr(), &mut ptr, &mut len) != 0 && len >= 4 {
            let pairs = std::slice::from_raw_parts(ptr as *const u16, (len / 2) as usize);
            for chunk in pairs.chunks(2) {
                if chunk.len() == 2 {
                    candidates.push(format!("{:04x}{:04x}", chunk[0], chunk[1]));
                }
            }
        }
        candidates.push("040904b0".into());
        candidates.push("040904e4".into());

        for lang in candidates {
            let q: Vec<u16> = format!("\\StringFileInfo\\{lang}\\FileDescription")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut l = 0u32;
            if VerQueryValueW(buf.as_ptr().cast(), q.as_ptr(), &mut p, &mut l) != 0 && l > 1 {
                let s = std::slice::from_raw_parts(p as *const u16, l as usize);
                let text = String::from_utf16_lossy(s).trim_end_matches('\0').trim().to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        None
    }
}

/// Friendly name for display: file description if known, otherwise the process name.
pub fn display_name(app: &str, description: Option<&str>) -> String {
    match description {
        Some(d) if !d.is_empty() && !d.eq_ignore_ascii_case(app) => d.to_string(),
        _ => app.to_string(),
    }
}

/// One-line explanation shown under an app's name.
pub fn explain(app: &str, description: Option<&str>) -> String {
    if let Some(k) = known(app) {
        return k.to_string();
    }
    let stem = app.trim_end_matches(".exe").trim_end_matches(".EXE");
    match description {
        Some(d) if !d.is_empty() && !d.eq_ignore_ascii_case(stem) && !d.eq_ignore_ascii_case(app) => {
            format!("{d}. Program file: {app}.")
        }
        _ => format!("Program file: {app}. It has no description of what it does."),
    }
}
