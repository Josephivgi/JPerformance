# PerfLens

A Windows app that records how your PC performs over time and tells you which apps used the most resources, and when.

Task Manager shows the last minute. PerfLens keeps a history.

## What it does

- **Right now**: live CPU, memory, disk and file I/O, and network, plus the apps using the most, with a short explanation of what each one is.
- **History**: CPU, memory, I/O and network charts for the last hour up to 30 days, and a weekday by hour map of when your PC is busiest.
- **Apps**: every app ranked by CPU, memory or I/O over a time range. Click one to see its own usage over time.
- **Insights**: plain findings such as your busiest hour and what caused it, long stretches of high CPU, times memory was nearly full, and which app used the most of each resource.
- **Background tracking**: a small helper process (`PerfLens.exe --collector`) keeps recording after you close the window. It can start when you sign in to Windows.

All data stays on your PC in `%APPDATA%\PerfLens\perflens.db` (SQLite). History older than 30 days is removed automatically; change that in Settings.

## Build

Requires Rust (MSVC toolchain).

```
cargo build --release
```

The app is `target\release\PerfLens.exe`.

## Command line

```
PerfLens.exe                 open the app (starts background tracking if it is on)
PerfLens.exe --collector     run only the background recorder
PerfLens.exe --report [h]    print the insights for the last h hours (default 24)
```

## Notes

- CPU per app is a share of the whole processor, so 100% means every core is busy.
- Processes with the same name are grouped, so all `chrome.exe` processes count as one app.
- "Disk and file I/O" comes from Windows process I/O counters, which include some non-disk I/O.
- GPU usage is not tracked yet.
