<#
  JSystems installer: installs the latest release of JSystems apps straight
  from GitHub Releases and pins them to the Windows taskbar. The same file
  is in each app's repo; on its own it installs that repo's app.

    This app:      irm https://raw.githubusercontent.com/Josephivgi/JPerformance/master/install.ps1 | iex
    Every app:     $env:JSYSTEMS_APPS='all'; irm <same url> | iex
    Some apps:     $env:JSYSTEMS_APPS='JAgents,JServer'; irm <same url> | iex
    Skip pinning:  $env:JSYSTEMS_NOPIN='1'

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
  @{ Name = 'Simple Recorder'; Repo = 'simple-recorder'; Asset = 'SimpleRecorder-Setup.exe'; Kind = 'setup'; Exe = 'Simple Recorder.exe' }
  @{ Name = 'JAgents';         Repo = 'JAgents';         Asset = 'JAgents-Setup.exe';        Kind = 'setup'; Exe = 'JAgents.exe' }
  @{ Name = 'JPerformance';    Repo = 'JPerformance';    Asset = 'JPerformance.exe';         Kind = 'exe';   Exe = 'JPerformance.exe' }
  @{ Name = 'JServer';         Repo = 'JServer';         Asset = 'JServer.exe';              Kind = 'exe';   Exe = 'JServer.exe' }
)
$want = if ($env:JSYSTEMS_APPS) { $env:JSYSTEMS_APPS -split ',' | ForEach-Object { $_.Trim() } } else { @($ThisApp) }
if ($want -notcontains 'all') { $Apps = @($Apps | Where-Object { $want -contains $_.Name -or $want -contains $_.Repo }) }
$NoPin = [bool]$env:JSYSTEMS_NOPIN
# The variables last for the whole PowerShell session; clear them so a later run starts clean.
Remove-Item Env:JSYSTEMS_APPS, Env:JSYSTEMS_NOPIN -ErrorAction SilentlyContinue

function Get-Release($app) {
  $rel = Invoke-RestMethod "https://api.github.com/repos/$Owner/$($app.Repo)/releases/latest" -Headers @{ 'User-Agent' = 'jsystems-install' }
  $asset = $rel.assets | Where-Object name -eq $app.Asset | Select-Object -First 1
  if (-not $asset) { throw "$($app.Asset) is missing from $($app.Repo) $($rel.tag_name)" }
  [pscustomobject]@{ Tag = $rel.tag_name; Url = $asset.browser_download_url; Digest = $asset.digest }
}

function Save-Asset($release, $dest) {
  Invoke-WebRequest $release.Url -OutFile $dest -UseBasicParsing -Headers @{ 'User-Agent' = 'jsystems-install' }
  # GitHub publishes a sha256 for each asset; refuse a download that does not match.
  if ($release.Digest -match '^sha256:(.+)$') {
    $hash = (Get-FileHash $dest -Algorithm SHA256).Hash
    if ($hash -ne $Matches[1]) { throw "checksum mismatch for $(Split-Path $dest -Leaf)" }
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

function Install-App($app) {
  $release = Get-Release $app
  Write-Host "  $($app.Name) $($release.Tag)" -NoNewline
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("jsystems-" + [guid]::NewGuid())
  New-Item -ItemType Directory $tmp | Out-Null
  try {
    $file = Join-Path $tmp $app.Asset
    Save-Asset $release $file
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
    Write-Host '  installed' -ForegroundColor Green
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
  New-Item $key -Force | Out-Null
  Set-ItemProperty $key StartLayoutFile $layout -Type ExpandString
  Set-ItemProperty $key LockedStartLayout 1 -Type DWord
  # Explorer applies the layout when it starts; this briefly redraws the taskbar.
  Get-Process explorer -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 2
  if (-not (Get-Process explorer -ErrorAction SilentlyContinue)) { Start-Process explorer.exe }
  Write-Host "  $($all -join ', ') added to the taskbar layout. If one is missing, sign out and back in." -ForegroundColor Green
}

Write-Host 'Installing JSystems apps from GitHub Releases'
$done = @()
foreach ($app in $Apps) { if (Install-App $app) { $done += $app.Name } }

if ($done -and -not $NoPin) {
  Write-Host 'Pinning to the taskbar'
  Set-TaskbarPins $done
}
if (-not $Apps) { Write-Host 'No app matched JSYSTEMS_APPS. Use all, or names like JAgents,JServer.' -ForegroundColor Red }
Write-Host "Done. $($done.Count) of $($Apps.Count) apps installed."
}
