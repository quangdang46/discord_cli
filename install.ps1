# =============================================================================
# discord - Windows install script
# Download the prebuilt Windows binary from GitHub Releases, or build from source.
#
# Usage:
#   irm "https://raw.githubusercontent.com/quangdang46/discord_cli/main/install.ps1" | iex
#   irm ".../install.ps1" | iex -Args "--easy-mode"
#
# NOTE: keep this file ASCII-only. Windows PowerShell (5.1 / pwsh on Windows)
# decodes no-BOM files as the system ANSI codepage, so non-ASCII characters
# (checkmarks, arrows, em-dashes) can mangle into stray quote characters and
# break parsing when piped through `irm | iex`.
# =============================================================================
param(
    [string]$Dest = "$HOME\.local\bin",
    [string]$Version = "",
    [switch]$System,
    [switch]$EasyMode,
    [switch]$Verify,
    [switch]$FromSource,
    [switch]$Uninstall,
    [switch]$Quiet
)

$ErrorActionPreference = "Stop"
$BinaryName = "discord"
$Owner = "quangdang46"
$Repo = "discord_cli"

function Log-Info($m) { if (-not $Quiet) { Write-Host "[$BinaryName] $m" -ForegroundColor Cyan } }
function Log-Warn($m) { Write-Host "[$BinaryName] WARN: $m" -ForegroundColor Yellow }
function Log-Success($m) { Write-Host "OK $m" -ForegroundColor Green }
function Die($m) { Write-Error $m; exit 1 }

if ($System) { $Dest = "$env:ProgramFiles" }

# --- Uninstall ---
if ($Uninstall) {
    Remove-Item -Force "$Dest\$BinaryName.exe" -ErrorAction SilentlyContinue
    # Remove PATH entries added by installer
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath) {
        $newPath = ($userPath -split ";" | Where-Object { $_ -and $_ -ne $Dest }) -join ";"
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    }
    Write-Host "OK $BinaryName uninstalled"
    exit 0
}

# --- Platform ---
$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { "x86_64" }
    "ARM64" { "aarch64" }
    default { Die "Unsupported arch: $env:PROCESSOR_ARCHITECTURE" }
}
$platform = "windows-$arch"

# --- Resolve version ---
if (-not $Version) {
    try {
        $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/releases/latest" -Headers @{ "Accept" = "application/vnd.github.v3+json" } -TimeoutSec 30
        $Version = $rel.tag_name
    } catch {
        $resp = Invoke-WebRequest -Uri "https://github.com/$Owner/$Repo/releases/latest" -MaximumRedirection 0 -ErrorAction SilentlyContinue
        $Version = ([regex]::Match($resp.Headers.Location, "/tag/(.*)")).Groups[1].Value
    }
}
if ($Version -notmatch '^v\d') { Die "Could not resolve version" }
Log-Info "Latest: $Version"

# --- Download ---
$tmp = Join-Path $env:TEMP ("discord-install-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    # Release assets are named discord-{platform}.zip (no version tag).
    $archive = "$BinaryName-$platform.zip"
    $url = "https://github.com/$Owner/$Repo/releases/download/$Version/$archive"
    $zipPath = Join-Path $tmp $archive

    if ($FromSource) {
        Log-Info "Building from source..."
        $srcDir = Join-Path $tmp "src"
        git clone --depth 1 "https://github.com/$Owner/$Repo.git" $srcDir
        Push-Location $srcDir
        try { cargo build --release -p discord-cli } finally { Pop-Location }
        $exe = Join-Path $srcDir "target\release\$BinaryName.exe"
    } else {
        try {
            Invoke-WebRequest -Uri $url -OutFile $zipPath -TimeoutSec 120
            # Checksum - the release publishes GNU sha256sum format, which may
            # be prefixed with a '*' (binary mode), e.g. "<hash> *discord-...".
            # NOTE: GitHub serves release assets as octet-stream, so
            # Invoke-WebRequest .Content is a byte[] on Windows PowerShell,
            # not a string - decode it explicitly.
            try {
                $raw = [System.Text.Encoding]::ASCII.GetString(
                    (Invoke-WebRequest -Uri "$url.sha256" -TimeoutSec 30).Content)
                $sum = ($raw.Trim() -split "\s+")[0].TrimStart("*").ToLower()
                $actual = (Get-FileHash $zipPath -Algorithm SHA256).Hash.ToLower()
                if ($sum -ne $actual) { Die "Checksum mismatch" }
                Log-Info "Checksum verified"
            } catch {
                Log-Warn "Checksum not verified: $_"
            }
            Expand-Archive -Path $zipPath -DestinationPath $tmp -Force
            $exe = Get-ChildItem -Path $tmp -Recurse -Filter "$BinaryName.exe" | Where-Object { $_.FullName -notmatch "sha256" } | Select-Object -First 1
            $exe = $exe.FullName
        } catch {
            Log-Warn "Binary download failed - building from source..."
            $srcDir = Join-Path $tmp "src"
            git clone --depth 1 "https://github.com/$Owner/$Repo.git" $srcDir
            Push-Location $srcDir
            try { cargo build --release -p discord-cli } finally { Pop-Location }
            $exe = Join-Path $srcDir "target\release\$BinaryName.exe"
        }
    }

    New-Item -ItemType Directory -Path $Dest -Force | Out-Null
    Copy-Item -Force $exe "$Dest\$BinaryName.exe"

    # --- PATH ---
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -and ($userPath -split ";") -notcontains $Dest) {
        if ($EasyMode) {
            [Environment]::SetEnvironmentVariable("Path", "$Dest;$userPath", "User")
            Log-Warn "PATH updated - restart terminal"
        } else {
            Log-Warn "Add to PATH: $Dest"
        }
    }

    if ($Verify) { & "$Dest\$BinaryName.exe" --version }

    Log-Success "$BinaryName installed -> $Dest\$BinaryName.exe"
    Write-Host ""
    Write-Host "  Quick start:"
    Write-Host "    $BinaryName auth --save"
    Write-Host "    $BinaryName guilds --json"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
