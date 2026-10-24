<#
  JSystems installer: installs the latest release of JSystems apps straight
  from GitHub Releases and pins them to the Windows taskbar. The same file
  is in each app's repo. It asks yes or no for each app, then installs the
  ones you picked.

    Ask:           irm https://raw.githubusercontent.com/Josephivgi/JPerformance/master/install.ps1 | iex
    No questions:  $env:JSYSTEMS_APPS='all'; irm <same url> | iex
    Some apps:     $env:JSYSTEMS_APPS='JAgents,JServer'; irm <same url> | iex
    Skip pinning:  $env:JSYSTEMS_NOPIN='1'
    Skip the JAgents terminal app:  $env:JSYSTEMS_NOTERMINAL='1'

  Per user, no admin. Downloads are made with PowerShell, so Windows does
  not mark them as "from the internet" and SmartScreen does not stop them.
#>
# Runs in its own scope so irm | iex leaves the caller's session unchanged.
& {
$ThisApp = 'JPerformance'

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$Owner = 'Josephivgi'
$Programs = Join-Path $env:LOCALAPPDATA 'Programs'
$StartMenu = [Environment]::GetFolderPath('Programs')

# Kind 'setup': an NSIS setup run with /S. Kind 'exe': a single portable exe.
# WriteRatio: how many bytes a setup writes to disk per byte of setup file,
# measured on a full install (Simple Recorder 1.3.0: 1306 of 110 MB; JAgents
# 0.6.2: 985 of 281 MB). The install percentage is written / expected.
$Apps = @(
  @{ Name = 'Simple Recorder'; Repo = 'simple-recorder'; Asset = 'SimpleRecorder-Setup.exe'; Kind = 'setup'; Exe = 'Simple Recorder.exe'; WriteRatio = 11.9; About = 'records your screen, a window or an area to MP4' }
  @{ Name = 'JAgents';         Repo = 'JAgents';         Asset = 'JAgents-Setup.exe';        Kind = 'setup'; Exe = 'JAgents.exe'; WriteRatio = 3.5; About = 'runs coding agents like Claude Code and Codex side by side' }
  @{ Name = 'JPerformance';    Repo = 'JPerformance';    Asset = 'JPerformance.exe';         Kind = 'exe';   Exe = 'JPerformance.exe'; About = 'tracks CPU, memory, disk and network use over 30 days' }
  @{ Name = 'JServer';         Repo = 'JServer';         Asset = 'JServer.exe';              Kind = 'exe';   Exe = 'JServer.exe'; About = 'watches the Linux servers in your SSH config' }
)
# JSYSTEMS_APPS picks apps with no questions; without it the installer asks.
$Ask = -not $env:JSYSTEMS_APPS
if (-not $Ask) {
  $want = $env:JSYSTEMS_APPS -split ',' | ForEach-Object { $_.Trim() }
  if ($want -notcontains 'all') { $Apps = @($Apps | Where-Object { $want -contains $_.Name -or $want -contains $_.Repo }) }
}
$NoPin = [bool]$env:JSYSTEMS_NOPIN
$NoTerminal = [bool]$env:JSYSTEMS_NOTERMINAL
# The variables last for the whole PowerShell session; clear them so a later run starts clean.
Remove-Item Env:JSYSTEMS_APPS, Env:JSYSTEMS_NOPIN, Env:JSYSTEMS_NOTERMINAL -ErrorAction SilentlyContinue

function Get-Release($app) {
  # Newest published release that has the file. A brand-new release can be
  # missing it for a few minutes while GitHub Actions builds the setup.
  $all = Invoke-RestMethod "https://api.github.com/repos/$Owner/$($app.Repo)/releases?per_page=10" -Headers @{ 'User-Agent' = 'jsystems-install' }
  $all = @($all | Where-Object { -not $_.draft -and -not $_.prerelease })
  foreach ($rel in $all) {
    $asset = $rel.assets | Where-Object name -eq $app.Asset | Select-Object -First 1
    if ($asset) {
      $note = if ($rel.tag_name -ne $all[0].tag_name) { "$($all[0].tag_name) is still being built, so this is $($rel.tag_name)" } else { '' }
      return [pscustomobject]@{ Tag = $rel.tag_name; Url = $asset.browser_download_url; Digest = $asset.digest; MB = [math]::Round($asset.size / 1MB); Note = $note }
    }
  }
  throw "no release of $($app.Repo) has $($app.Asset)"
}

$ProgressEvery = 5   # seconds between progress lines

# Downloads in 1 MB pieces so it can print how far along it is, and hashes as
# it goes. GitHub publishes a sha256 for each asset; a download that does not
# match is refused. (.NET directly: Get-FileHash is a script function in
# Windows PowerShell 5.1 and does not load on some locked-down accounts.)
function Save-Asset($release, $dest) {
  Add-Type -AssemblyName System.Net.Http -ErrorAction SilentlyContinue
  $client = New-Object System.Net.Http.HttpClient
  $client.Timeout = [TimeSpan]::FromMinutes(30)
  $client.DefaultRequestHeaders.UserAgent.ParseAdd('jsystems-install')
  $sha = [Security.Cryptography.SHA256]::Create()
  $out = $null
  try {
    $resp = $client.GetAsync($release.Url, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
    [void]$resp.EnsureSuccessStatusCode()
    $total = [double]$resp.Content.Headers.ContentLength
    $in = $resp.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
    $out = [IO.File]::Create($dest)
    $buf = New-Object byte[] 1MB
    $got = 0
    $tick = [Diagnostics.Stopwatch]::StartNew()
    while (($n = $in.Read($buf, 0, $buf.Length)) -gt 0) {
      $out.Write($buf, 0, $n)
      [void]$sha.TransformBlock($buf, 0, $n, $null, 0)
      $got += $n
      if ($tick.Elapsed.TotalSeconds -ge $ProgressEvery -and $total -gt 0) {
        Write-Host ("    downloading {0}% ({1} of {2} MB)" -f [int](100 * $got / $total), [int]($got / 1MB), [int]($total / 1MB))
        $tick.Restart()
      }
    }
    [void]$sha.TransformFinalBlock($buf, 0, 0)
  } finally {
    if ($out) { $out.Dispose() }
    $client.Dispose()
  }
  if ($release.Digest -match '^sha256:(.+)$') {
    $hash = -join ($sha.Hash | ForEach-Object { $_.ToString('x2') })
    if ($hash -ne $Matches[1]) { throw "checksum mismatch for $(Split-Path $dest -Leaf)" }
  }
  Unblock-File $dest
}

# Bytes written to disk so far by a process and everything it started. The
# setups run hidden, so this is what shows how far an install has got.
function Get-TreeWrites($rootId, $seen) {
  $all = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue)
  $ids = @($rootId)
  do {
    $count = $ids.Count
    $ids = @($ids + @($all | Where-Object { $ids -contains $_.ParentProcessId } | ForEach-Object ProcessId) | Select-Object -Unique)
  } while ($ids.Count -gt $count)
  foreach ($p in $all | Where-Object { $ids -contains $_.ProcessId }) {
    $key = [string]$p.ProcessId
    if (-not $seen.ContainsKey($key) -or $seen[$key] -lt [double]$p.WriteTransferCount) { $seen[$key] = [double]$p.WriteTransferCount }
  }
  ($seen.Values | Measure-Object -Sum).Sum
}

function New-Shortcut($lnk, $target) {
  $ws = New-Object -ComObject WScript.Shell
  $sc = $ws.CreateShortcut($lnk)
  $sc.TargetPath = $target
  $sc.WorkingDirectory = Split-Path $target
  $sc.IconLocation = "$target,0"
  $sc.Save()
}

# The installed version, or '' when the app is not installed.
function Get-InstalledVersion($app) {
  if ($app.Kind -eq 'setup') {
    $entry = Get-ChildItem HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall -ErrorAction SilentlyContinue |
      Get-ItemProperty -ErrorAction SilentlyContinue | Where-Object DisplayName -eq $app.Name | Select-Object -First 1
    if ($entry) { return [string]$entry.DisplayVersion }
    return ''
  }
  $exe = Join-Path (Join-Path $Programs $app.Name) $app.Exe
  if (Test-Path $exe) { $v = (Get-Item $exe).VersionInfo.ProductVersion; if ($v) { return $v } else { return 'yes' } }
  return ''
}

# One yes/no question per app. Enter means yes.
function Select-Apps($apps) {
  Write-Host 'JSystems apps. Answer y or n for each one (Enter = yes).'
  Write-Host ''
  $picked = @()
  foreach ($app in $apps) {
    try { $app.Release = Get-Release $app }
    catch { Write-Host "  $($app.Name): not available right now ($($_.Exception.Message))" -ForegroundColor Yellow; continue }
    $have = Get-InstalledVersion $app
    $status = if (-not $have) { 'not installed' } elseif ($have -eq 'yes') { 'installed' } elseif ($have.TrimStart('v') -eq $app.Release.Tag.TrimStart('v')) { 'installed, up to date' } else { "installed: $have" }
    Write-Host "  $($app.Name) $($app.Release.Tag), $($app.Release.MB) MB" -ForegroundColor Cyan -NoNewline
    Write-Host " ($status)"
    Write-Host "    $($app.About)"
    if ($app.Release.Note) { Write-Host "    $($app.Release.Note)" -ForegroundColor Yellow }
    do {
      $answer = (Read-Host "    Install $($app.Name)? [Y/n]").Trim().ToLower()
    } until ($answer -in '', 'y', 'yes', 'n', 'no')
    if ($answer -in '', 'y', 'yes') { $picked += $app }
    Write-Host ''
  }
  return $picked
}

function Install-App($app, $number, $count) {
  $release = if ($app.Release) { $app.Release } else { Get-Release $app }
  Write-Host "[$number/$count] $($app.Name) $($release.Tag) ($($release.MB) MB)" -ForegroundColor Cyan
  if ($release.Note) { Write-Host "    ($($release.Note))" }
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("jsystems-" + [guid]::NewGuid())
  New-Item -ItemType Directory $tmp | Out-Null
  try {
    $file = Join-Path $tmp $app.Asset
    $clock = [Diagnostics.Stopwatch]::StartNew()
    Write-Host '    downloading...'
    Save-Asset $release $file
    Write-Host "    downloaded in $([int]$clock.Elapsed.TotalSeconds)s, installing..."
    if ($app.Kind -eq 'setup') {
      # Not -Wait: the setup opens the app when it finishes, and -Wait would
      # also wait for the app to close. /S setups show no window. While it
      # runs, print how much of what it normally writes has been written.
      $p = Start-Process $file -ArgumentList '/S' -PassThru
      $expected = $app.WriteRatio * (Get-Item $file).Length
      $seen = @{}
      $tick = [Diagnostics.Stopwatch]::StartNew()
      while (-not $p.WaitForExit(500)) {
        $written = Get-TreeWrites $p.Id $seen
        if ($tick.Elapsed.TotalSeconds -ge $ProgressEvery) {
          $pct = if ($expected -gt 0) { [math]::Min(99, [int](100 * $written / $expected)) } else { 0 }
          Write-Host "    installing about $pct% ($([int]$clock.Elapsed.TotalSeconds)s so far)"
          $tick.Restart()
        }
      }
      if ($p.ExitCode -ne 0) { throw "setup exited with code $($p.ExitCode). Close $($app.Name) and run this again." }
    } else {
      $dir = Join-Path $Programs $app.Name
      New-Item -ItemType Directory $dir -Force | Out-Null
      $exe = Join-Path $dir $app.Exe
      $old = Join-Path $dir ($app.Exe -replace '\.exe$', '.old.exe')
      Remove-Item $old -Force -ErrorAction SilentlyContinue
      # The background recorder (--collector) has no window but keeps the exe
      # open. Windows lets a running exe be renamed, not overwritten, so this
      # swaps the file the way the app's own updater does, then restarts the
      # recorder from the new exe.
      $running = @(Get-CimInstance Win32_Process -Filter "Name='$($app.Exe)'" -ErrorAction SilentlyContinue | Where-Object ExecutablePath -eq $exe)
      $collectors = @($running | Where-Object CommandLine -match '--collector')
      if (Test-Path $exe) {
        try { Rename-Item $exe $old -ErrorAction Stop }
        catch { throw "$($app.Exe) could not be replaced: $($_.Exception.Message)" }
      }
      try { Copy-Item $file $exe -Force }
      catch {
        if (Test-Path $old) { Rename-Item $old $exe -ErrorAction SilentlyContinue }
        throw "$($app.Exe) could not be written: $($_.Exception.Message)"
      }
      if ($collectors) {
        $collectors | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
        Start-Process $exe -ArgumentList '--collector'
        Write-Host '    background recorder restarted'
      }
      Start-Sleep -Milliseconds 500
      Remove-Item $old -Force -ErrorAction SilentlyContinue
      if ($running.Count -gt $collectors.Count) { Write-Host "    reopen $($app.Name) to use the new version" }
      New-Shortcut (Join-Path $StartMenu "$($app.Name).lnk") $exe
    }
    Write-Host "    installed in $([int]$clock.Elapsed.TotalSeconds)s (100%)" -ForegroundColor Green
    return $true
  } catch {
    Write-Host "    failed: $($_.Exception.Message)" -ForegroundColor Red
    return $false
  } finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
  }
}

# Windows 11 has no public "pin to taskbar" call for desktop apps. The supported
# route is a taskbar layout file set by user policy: Explorer reads it when it
# starts and adds the listed Start Menu shortcuts to the taskbar. Existing pins
# stay, and anything pinned this way can still be unpinned by hand.
function Set-TaskbarPins($names) {
  $key = 'HKCU:\Software\Policies\Microsoft\Windows\Explorer'
  $layout = Join-Path $env:LOCALAPPDATA 'JSystems\TaskbarLayout.xml'
  $current = (Get-ItemProperty $key -ErrorAction SilentlyContinue).StartLayoutFile
  if ($current -and $current -ne $layout) {
    Write-Host "  skipped: another taskbar layout policy is already set ($current)" -ForegroundColor Yellow
    return
  }
  # Keep apps pinned by an earlier run, so -Only never unpins the others.
  $all = @($names)
  if (Test-Path $layout) {
    $all += ([xml](Get-Content $layout -Raw)).SelectNodes("//*[local-name()='DesktopApp']") |
      ForEach-Object { [IO.Path]::GetFileNameWithoutExtension($_.DesktopApplicationLinkPath) }
  }
  $all = $all | Where-Object { Test-Path (Join-Path $StartMenu "$_.lnk") } | Sort-Object -Unique
  $pins = ($all | ForEach-Object {
    "        <taskbar:DesktopApp DesktopApplicationLinkPath=""%APPDATA%\Microsoft\Windows\Start Menu\Programs\$_.lnk"" />"
  }) -join "`r`n"
  New-Item -ItemType Directory (Split-Path $layout) -Force | Out-Null
  @"
<?xml version="1.0" encoding="utf-8"?>
<LayoutModificationTemplate
    xmlns="http://schemas.microsoft.com/Start/2014/LayoutModification"
    xmlns:defaultlayout="http://schemas.microsoft.com/Start/2014/FullDefaultLayout"
    xmlns:start="http://schemas.microsoft.com/Start/2014/StartLayout"
    xmlns:taskbar="http://schemas.microsoft.com/Start/2014/TaskbarLayout"
    Version="1">
  <CustomTaskbarLayoutCollection>
    <defaultlayout:TaskbarLayout>
      <taskbar:TaskbarPinList>
$pins
      </taskbar:TaskbarPinList>
    </defaultlayout:TaskbarLayout>
  </CustomTaskbarLayoutCollection>
</LayoutModificationTemplate>
"@ | Set-Content $layout -Encoding UTF8

  # Only admins can write under Software\Policies, even in HKCU. The layout file
  # above is the user's own, so once the policy points at it later runs need no
  # permission. The first run asks once, and writes to this user's hive by SID
  # so it still lands here if a different admin account approves the prompt.
  $policy = Get-ItemProperty $key -ErrorAction SilentlyContinue
  if ($policy.StartLayoutFile -ne $layout -or $policy.LockedStartLayout -ne 1) {
    Write-Host '  Windows will ask once for permission to add taskbar pins.'
    $hive = "Registry::HKEY_USERS\$([Security.Principal.WindowsIdentity]::GetCurrent().User.Value)\Software\Policies\Microsoft\Windows\Explorer"
    $set = "`$k = '$hive'; if (-not (Test-Path `$k)) { New-Item `$k -Force | Out-Null }; " +
      "Set-ItemProperty `$k StartLayoutFile '$layout' -Type ExpandString; Set-ItemProperty `$k LockedStartLayout 1 -Type DWord"
    $enc = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($set))
    try {
      Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -Wait -ArgumentList '-NoProfile', '-EncodedCommand', $enc
    } catch {
      Write-Host '  skipped: permission was not given. The apps are installed; pin them from the Start Menu.' -ForegroundColor Yellow
      return
    }
    $policy = Get-ItemProperty $key -ErrorAction SilentlyContinue
    if ($policy.StartLayoutFile -ne $layout) {
      Write-Host '  skipped: the taskbar setting could not be saved. Pin the apps from the Start Menu.' -ForegroundColor Yellow
      return
    }
  }

  # Explorer applies the layout when it starts; this briefly redraws the taskbar.
  # Windows restarts Explorer on its own. Starting it from here is a last resort,
  # since an Explorer started by an admin window would itself run as admin.
  Get-Process explorer -ErrorAction SilentlyContinue | Stop-Process -Force
  for ($i = 0; $i -lt 20 -and -not (Get-Process explorer -ErrorAction SilentlyContinue); $i++) { Start-Sleep -Milliseconds 500 }
  if (-not (Get-Process explorer -ErrorAction SilentlyContinue)) { Start-Process explorer.exe }
  Write-Host "  $($all -join ', ') added to the taskbar." -ForegroundColor Green
}

if ($Ask) {
  # Read-Host fails where nobody can answer (a hidden or -NonInteractive
  # PowerShell); install this repo's own app there, as before.
  try { $Apps = @(Select-Apps $Apps) }
  catch {
    Write-Host "No one to answer the questions, so installing $ThisApp only. Set JSYSTEMS_APPS to choose." -ForegroundColor Yellow
    $Apps = @($Apps | Where-Object Name -eq $ThisApp)
  }
  if (-not $Apps) { Write-Host 'Nothing picked, so nothing was installed.'; return }
  Write-Host "Installing: $(($Apps | ForEach-Object Name) -join ', ')"
}

Write-Host ''
Write-Host 'This can take up to 10 minutes, depending on how many apps you picked and' -ForegroundColor Yellow
Write-Host 'how busy your PC is. That is normal. It is not stuck: progress is shown' -ForegroundColor Yellow
Write-Host "every $ProgressEvery seconds. Keep this window open until it says Done." -ForegroundColor Yellow
Write-Host ''
Write-Host 'Installing JSystems apps from GitHub Releases'
$done = @()
$total = [Diagnostics.Stopwatch]::StartNew()
for ($i = 0; $i -lt $Apps.Count; $i++) { if (Install-App $Apps[$i] ($i + 1) $Apps.Count) { $done += $Apps[$i].Name } }

# JAgents also has a terminal app (the jagents command), installed by
# install-terminal.ps1. It needs Node.js 22 or newer; without it the desktop
# app is still installed and this says how to add the terminal app later.
if ($done -contains 'JAgents' -and -not $NoTerminal) {
  Write-Host 'Installing the JAgents terminal app'
  $major = if (Get-Command node -ErrorAction SilentlyContinue) { [int](node -p "process.versions.node.split('.')[0]") } else { 0 }
  if ($major -lt 22) {
    Write-Host '  skipped: it needs Node.js 22 or newer from https://nodejs.org. Run this again after installing it.' -ForegroundColor Yellow
  } else {
    try {
      Invoke-Expression (Invoke-RestMethod "https://raw.githubusercontent.com/$Owner/JAgents/main/install-terminal.ps1" -Headers @{ 'User-Agent' = 'jsystems-install' })
    } catch {
      Write-Host "  failed: $($_.Exception.Message)" -ForegroundColor Red
    }
  }
}

if ($done -and -not $NoPin) {
  Write-Host 'Pinning to the taskbar'
  try { Set-TaskbarPins $done }
  catch { Write-Host "  skipped: $($_.Exception.Message) Pin the apps from the Start Menu." -ForegroundColor Yellow }
}
if (-not $Apps) { Write-Host 'No app matched JSYSTEMS_APPS. Use all, or names like JAgents,JServer.' -ForegroundColor Red }
Write-Host "Done. $($done.Count) of $($Apps.Count) apps installed in $([math]::Floor($total.Elapsed.TotalMinutes)) min $($total.Elapsed.Seconds) s."
}
