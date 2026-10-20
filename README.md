# JPerformance

A Windows app that records how your PC performs over time and tells you which apps used the most resources, and when.

Task Manager shows the last minute. JPerformance keeps a history.

## What it does

- **Right now**: live CPU, memory, disk and file I/O, network, and how many processes and apps are running, plus the apps using the most, with a short explanation of what each one is.
- **History**: CPU, memory, I/O and network charts for the last hour up to 30 days, and a weekday by hour map of when your PC is busiest. Show the whole PC or pick one app. Hover any chart to see which apps were behind that point, and click to keep that moment open. A processes chart shows how many were running at once.
- **Track an app**: pick a running app, choose 10 minutes, 30 minutes or until you stop it, and press Start. It is recorded every second (CPU, memory, I/O, processes) with live charts and a summary. Recordings are saved until you delete them, and keep going if you close the window.
- **Apps**: every app ranked by CPU, memory or I/O over a time range. Click one to see its own usage over time.
- **Insights**: plain findings such as your busiest hour and what caused it, long stretches of high CPU, times memory was nearly full, and which app used the most of each resource.
- **Running apps**: every running app with its processes, CPU and memory, and a Stop app button. Stopping closes the app the same way as quitting it; nothing is uninstalled or deleted. If an app does not close, Force stop ends it after you confirm. Windows system processes cannot be stopped here.
- **Background tracking**: always on. A small helper process (`JPerformance.exe --collector`) keeps recording after you close the window. Tick **Start with Windows** at the bottom of the menu to keep tracking after a restart.

All data stays on your PC in `%APPDATA%\JPerformance\jperformance.db` (SQLite). It takes a sample every 5 seconds and removes history older than 30 days automatically. There are no settings to manage. History recorded under the old name (PerfLens) is copied over the first time JPerformance starts.

## Build

Requires Rust (MSVC toolchain).

```
cargo build --release
```

The app is `target\release\JPerformance.exe`.

## Command line

```
JPerformance.exe                 open the app (starts background tracking if it is on)
JPerformance.exe --collector     run only the background recorder
JPerformance.exe --report [h]    print the insights for the last h hours (default 24)
JPerformance.exe --stop <app.exe> [--force]   stop a running app, like the Stop app button
```

## Notes

- CPU per app is a share of the whole processor, so 100% means every core is busy.
- Processes with the same name are grouped, so all `chrome.exe` processes count as one app.
- "Disk and file I/O" comes from Windows process I/O counters, which include some non-disk I/O.
- GPU usage is not tracked yet.
