<#
.SYNOPSIS
  Packages a clean, zero-personal-data StreamVault 64-bit Windows Portable Release ZIP + SHA256SUMS.txt
  ready for uploading to GitHub Releases.

.DESCRIPTION
  NEVER zip your personal daily-driver `StreamVault\` folder directly, because its `data\` directory
  contains your browsing history, WebView2 cookies, and `streamvault.db`!
  This script stages a brand-new clean `dist\StreamVault\` directory with empty `data\` folders,
  copies `StreamVault.exe`, `libmpv-2.dll`, `vulkan-1.dll` (if present), licenses, and helper scripts,
  and outputs:
    - dist\StreamVault-v<Version>-win64-portable.zip
    - dist\SHA256SUMS.txt

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\package-portable-release.ps1 `
    -ExePath "..\StreamVault\StreamVault.exe" `
    -MpvDllPath "..\StreamVault\runtime\media-engine\libmpv-2.dll" `
    -VulkanStubPath "..\StreamVault\runtime\media-engine\vulkan-1.dll"
#>

param(
    [string]$Version = "1.0.0",
    [string]$ExePath = "target\x86_64-pc-windows-msvc\release\StreamVault.exe",
    [string]$MpvDllPath = "..\StreamVault\runtime\media-engine\libmpv-2.dll",
    [string]$VulkanStubPath = "..\StreamVault\runtime\media-engine\vulkan-1.dll"
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Definition)
Set-Location $RepoRoot

if (-not (Test-Path $ExePath)) {
    throw "StreamVault.exe not found at '$ExePath'. Build first with: cargo build --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc --release (or pass -ExePath)."
}
if (-not (Test-Path $MpvDllPath)) {
    throw "libmpv-2.dll not found at '$MpvDllPath'. Pass -MpvDllPath pointing to your 64-bit LGPL libmpv-2.dll."
}

$DistDir  = Join-Path $RepoRoot "dist"
$StageDir = Join-Path $DistDir "StreamVault"
$ZipName  = "StreamVault-v$Version-win64-portable.zip"
$ZipPath  = Join-Path $DistDir $ZipName
$SumsPath = Join-Path $DistDir "SHA256SUMS.txt"

if (Test-Path $StageDir) { Remove-Item -Recurse -Force $StageDir }
if (Test-Path $ZipPath)  { Remove-Item -Force $ZipPath }

# 1. Create clean, empty portable directory structure (ZERO personal browsing data!)
$Dirs = @(
    "data\database",
    "data\settings",
    "data\cache\media",
    "data\downloads",
    "data\logs",
    "data\backups",
    "data\webview2\shell",
    "data\webview2\tabs",
    "runtime\media-engine"
)
foreach ($d in $Dirs) {
    New-Item -ItemType Directory -Force -Path (Join-Path $StageDir $d) | Out-Null
}

# 2. Copy executable, media engine, licenses, and portable helper scripts
Copy-Item $ExePath (Join-Path $StageDir "StreamVault.exe") -Force
Copy-Item $MpvDllPath (Join-Path $StageDir "runtime\media-engine\libmpv-2.dll") -Force
if (Test-Path $VulkanStubPath) {
    Copy-Item $VulkanStubPath (Join-Path $StageDir "runtime\media-engine\vulkan-1.dll") -Force
}

Copy-Item "packaging\Check-Dependencies.bat" (Join-Path $StageDir "Check-Dependencies.bat") -Force
Copy-Item "packaging\check-windows-dependencies.ps1" (Join-Path $StageDir "runtime\check-windows-dependencies.ps1") -Force
Copy-Item "packaging\README-PORTABLE.txt" (Join-Path $StageDir "README.txt") -Force
Copy-Item "packaging\LICENSE-libmpv.txt" (Join-Path $StageDir "runtime\media-engine\LICENSE-libmpv.txt") -Force
Copy-Item "LICENSE" (Join-Path $StageDir "LICENSE.txt") -Force
Copy-Item "THIRD_PARTY_NOTICES.md" (Join-Path $StageDir "THIRD_PARTY_NOTICES.md") -Force

# 3. Compress clean portable archive & compute SHA-256 checksums
Write-Host "Compressing $ZipName ..." -ForegroundColor Cyan
Compress-Archive -Path $StageDir -DestinationPath $ZipPath -CompressionLevel Optimal

$ZipHash = (Get-FileHash -Algorithm SHA256 $ZipPath).Hash.ToLower()
$ExeHash = (Get-FileHash -Algorithm SHA256 (Join-Path $StageDir "StreamVault.exe")).Hash.ToLower()

@(
    "$ZipHash  $ZipName",
    "$ExeHash  StreamVault.exe"
) | Set-Content -Encoding UTF8 $SumsPath

Write-Host ""
Write-Host "SUCCESS! Clean GitHub Release artifacts generated:" -ForegroundColor Green
Write-Host "  Archive  : $ZipPath"
Write-Host "  Checksums: $SumsPath"
Write-Host ""
Get-Content $SumsPath
