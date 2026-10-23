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
$Apps = @(
  @{ Name = 'Simple Recorder'; Repo = 'simple-recorder'; Asset = 'SimpleRecorder-Setup.exe'; Kind = 'setup'; Exe = 'Simple Recorder.exe'; About = 'records your screen, a window or an area to MP4' }
  @{ Name = 'JAgents';         Repo = 'JAgents';         Asset = 'JAgents-Setup.exe';        Kind = 'setup'; Exe = 'JAgents.exe'; About = 'runs coding agents like Claude Code and Codex side by side' }
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

function Save-Asset($release, $dest) {
  Invoke-WebRequest $release.Url -OutFile $dest -UseBasicParsing -Headers @{ 'User-Agent' = 'jsystems-install' }
  # GitHub publishes a sha256 for each asset; refuse a download that does not match.
  # .NET directly: Get-FileHash is a script function in Windows PowerShell 5.1 and
  # does not load on some locked-down accounts.
  if ($release.Digest -match '^sha256:(.+)$') {
    $want = $Matches[1]
    $stream = [IO.File]::OpenRead($dest)
    try { $hash = -join ([Security.Cryptography.SHA256]::Create().ComputeHash($stream) | ForEach-Object { $_.ToString('x2') }) }
    finally { $stream.Dispose() }
    if ($hash -ne $want) { throw "checksum mismatch for $(Split-Path $dest -Leaf)" }
  }
  Unblock-File $dest
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

function Install-App($app) {
  $release = if ($app.Release) { $app.Release } else { Get-Release $app }
  if ($release.Note) { Write-Host "  ($($release.Note))" }
  Write-Host "  $($app.Name) $($release.Tag)" -NoNewline
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("jsystems-" + [guid]::NewGuid())
  New-Item -ItemType Directory $tmp | Out-Null
  try {
    $file = Join-Path $tmp $app.Asset
    $clock = [Diagnostics.Stopwatch]::StartNew()
    Write-Host " ($($release.MB) MB) downloading..." -NoNewline
    Save-Asset $release $file
    Write-Host ' installing...' -NoNewline
    if ($app.Kind -eq 'setup') {
      # WaitForExit, not -Wait: the setup opens the app when it finishes, and
      # -Wait would also wait for the app to close. /S setups show no window.
      $p = Start-Process $file -ArgumentList '/S' -PassThru
      $p.WaitForExit()
      if ($p.ExitCode -ne 0) { throw "setup exited with code $($p.ExitCode). Close $($app.Name) and run this again." }
    } else {
      $dir = Join-Path $Programs $app.Name
      New-Item -ItemType Directory $dir -Force | Out-Null
      $exe = Join-Path $dir $app.Exe
      try { Copy-Item $file $exe -Force }
      catch { throw "$($app.Exe) is in use. Close $($app.Name) (and its background recorder) and run this again." }
      New-Shortcut (Join-Path $StartMenu "$($app.Name).lnk") $exe
    }
    Write-Host " installed in $([int]$clock.Elapsed.TotalSeconds)s" -ForegroundColor Green
    return $true
  } catch {
    Write-Host "  failed: $($_.Exception.Message)" -ForegroundColor Red
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

Write-Host 'Installing JSystems apps from GitHub Releases'
$done = @()
foreach ($app in $Apps) { if (Install-App $app) { $done += $app.Name } }

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
Write-Host "Done. $($done.Count) of $($Apps.Count) apps installed."
}
