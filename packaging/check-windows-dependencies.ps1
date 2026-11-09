# StreamVault — Windows 10/11 Portable Dependency & Folder Health Checker
# Run via PowerShell or double-click Check-Dependencies.bat in the StreamVault root.

$ErrorActionPreference = "SilentlyContinue"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$AppRoot   = Split-Path -Parent $ScriptDir

Write-Host "================================================================" -ForegroundColor Cyan
Write-Host "  StreamVault Portable — Windows Runtime & Dependency Check" -ForegroundColor Cyan
Write-Host "================================================================" -ForegroundColor Cyan
Write-Host "Portable Root : $AppRoot"
Write-Host "OS Version    : $([System.Environment]::OSVersion.VersionString) (64-bit: $([Environment]::Is64BitOperatingSystem))"
Write-Host ""

$AllOk = $true

# 1. Architecture check
if ([Environment]::Is64BitOperatingSystem) {
    Write-Host "[PASS] 64-bit Windows OS detected." -ForegroundColor Green
} else {
    Write-Host "[FAIL] StreamVault requires 64-bit Windows 10 or Windows 11." -ForegroundColor Red
    $AllOk = $false
}

# 2. StreamVault.exe check
$ExePath = Join-Path $AppRoot "StreamVault.exe"
if (Test-Path $ExePath) {
    $ExeSizeMb = [math]::Round((Get-Item $ExePath).Length / 1MB, 2)
    Write-Host "[PASS] StreamVault.exe present ($ExeSizeMb MB, statically linked VC++ runtime)." -ForegroundColor Green
} else {
    Write-Host "[FAIL] StreamVault.exe not found at $ExePath" -ForegroundColor Red
    $AllOk = $false
}

# 3. Portable folder write permission check
$ProbeFile = Join-Path $AppRoot "data\.write_probe.tmp"
try {
    New-Item -ItemType Directory -Force -Path (Join-Path $AppRoot "data") | Out-Null
    [System.IO.File]::WriteAllText($ProbeFile, "ok")
    Remove-Item $ProbeFile -Force
    Write-Host "[PASS] Portable data directory is writable (no AppData dependency)." -ForegroundColor Green
} catch {
    Write-Host "[WARN] Portable folder appears read-only! Move StreamVault out of Program Files to Desktop, D:\, or a USB SSD." -ForegroundColor Yellow
    $AllOk = $false
}

# 4. Free disk space check
$DriveRoot = [System.IO.Path]::GetPathRoot($AppRoot)
$DriveInfo = New-Object System.IO.DriveInfo($DriveRoot)
if ($DriveInfo.AvailableFreeSpace) {
    $FreeGb = [math]::Round($DriveInfo.AvailableFreeSpace / 1GB, 2)
    Write-Host "[PASS] Available disk space on $DriveRoot : $FreeGb GB" -ForegroundColor Green
}

# 5. Microsoft Edge WebView2 Runtime check
$Wv2ClientGuid = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
$Wv2Paths = @(
    "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$Wv2ClientGuid",
    "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$Wv2ClientGuid",
    "HKCU:\Software\Microsoft\EdgeUpdate\Clients\$Wv2ClientGuid"
)
$Wv2Version = $null
foreach ($RegPath in $Wv2Paths) {
    if (Test-Path $RegPath) {
        $pv = (Get-ItemProperty -Path $RegPath -Name "pv").pv
        if ($pv -and $pv -ne "0.0.0.0") {
            $Wv2Version = $pv
            break
        }
    }
}
$FixedWv2 = Join-Path $AppRoot "runtime\webview2"
if ($Wv2Version) {
    Write-Host "[PASS] Microsoft Edge WebView2 Evergreen Runtime installed (v$Wv2Version)." -ForegroundColor Green
} elseif (Test-Path $FixedWv2) {
    Write-Host "[PASS] Bundled WebView2 Fixed Version Runtime found in runtime\webview2." -ForegroundColor Green
} else {
    Write-Host "[FAIL] Microsoft Edge WebView2 Runtime was NOT detected." -ForegroundColor Red
    Write-Host "       -> Install WebView2 Evergreen Runtime from:" -ForegroundColor Yellow
    Write-Host "          https://developer.microsoft.com/en-us/microsoft-edge/webview2/" -ForegroundColor Yellow
    Write-Host "       -> OR extract a WebView2 Fixed Version x64 archive into: $FixedWv2" -ForegroundColor Yellow
    $AllOk = $false
}

# 6. libmpv-2.dll media engine check
$MpvDll = Join-Path $AppRoot "runtime\media-engine\libmpv-2.dll"
if (Test-Path $MpvDll) {
    $MpvSizeMb = [math]::Round((Get-Item $MpvDll).Length / 1MB, 2)
    Write-Host "[PASS] Bundled libmpv-2.dll media engine present ($MpvSizeMb MB — MKV/HEVC/AV1/ASS/FLAC ready)." -ForegroundColor Green
} else {
    Write-Host "[FAIL] runtime\media-engine\libmpv-2.dll is missing!" -ForegroundColor Red
    $AllOk = $false
}

# 7. Detected External Media Players (Informational)
Write-Host ""
Write-Host "Optional External Media Players Detected on This PC:" -ForegroundColor Cyan
$CandidatePlayers = @(
    @{ Name = "PotPlayer (64-bit)"; Path = "$env:ProgramFiles\DAUM\PotPlayer\PotPlayerMini64.exe" },
    @{ Name = "PotPlayer (32-bit)"; Path = "${env:ProgramFiles(x86)}\DAUM\PotPlayer\PotPlayerMini.exe" },
    @{ Name = "VLC Media Player";   Path = "$env:ProgramFiles\VideoLAN\VLC\vlc.exe" },
    @{ Name = "MPC-HC (64-bit)";    Path = "$env:ProgramFiles\MPC-HC\mpc-hc64.exe" },
    @{ Name = "MPC-BE (64-bit)";    Path = "$env:ProgramFiles\MPC-BE x64\mpc-be64.exe" },
    @{ Name = "Windows Media Player"; Path = "$env:ProgramFiles\Windows Media Player\wmplayer.exe" }
)
$FoundAny = $false
foreach ($p in $CandidatePlayers) {
    if (Test-Path $p.Path) {
        Write-Host "  + $($p.Name) -> $($p.Path)" -ForegroundColor Gray
        $FoundAny = $true
    }
}
if (-not $FoundAny) {
    Write-Host "  (None in standard Program Files paths; StreamVault's built-in libmpv player handles all formats directly.)" -ForegroundColor Gray
}

Write-Host ""
if ($AllOk) {
    Write-Host "RESULT: READY — Double-click StreamVault.exe to launch!" -ForegroundColor Green
} else {
    Write-Host "RESULT: ACTION NEEDED — Review any [FAIL]/[WARN] items above before launching." -ForegroundColor Yellow
}
Write-Host ""
