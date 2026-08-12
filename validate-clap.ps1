# validate-clap.ps1
# Ship-gate: run clap-validator on built/installed CLAPs.
#
# Windows: always -j 1. Parallel out-of-process jobs can flake with
# ACCESS_VIOLATION (0xc0000005) — including Lucent Relay param-fuzz-* —
# even when the same tests pass serially. Not treated as a product bug.
#
# Usage:
#   .\validate-clap.ps1
#   .\validate-clap.ps1 -Plugins "lucent-relay","aether"
#   .\validate-clap.ps1 -Paths "C:\path\Lucent Relay.clap"
#   .\validate-clap.ps1 -Include "param-fuzz" -OnlyFailed
#
# Requires: clap-validator on PATH (cargo install clap-validator).

param(
    [string[]]$Plugins = @(),
    [string[]]$Paths = @(),
    [string]$Include = "",
    [switch]$OnlyFailed
)

$ErrorActionPreference = "Stop"

if (-not (Get-Command clap-validator -ErrorAction SilentlyContinue)) {
    throw "clap-validator not on PATH. Install: cargo install clap-validator"
}

# Display name → clap file stem (matches cargo-aura install names).
$clapFiles = [ordered]@{
    "aether"       = "Aether.clap"
    "meridian"     = "Meridian.clap"
    "equilibrium"  = "Equilibrium.clap"
    "lucent"       = "Lucent.clap"
    "lucent-relay" = "Lucent Relay.clap"
}

$searchDirs = @(
    (Join-Path $env:LOCALAPPDATA "Programs\Common\CLAP"),
    (Join-Path ${env:COMMONPROGRAMFILES} "CLAP"),
    (Join-Path $PSScriptRoot "target\bundled")
) | Where-Object { $_ -and (Test-Path $_) }

function Resolve-ClapPath {
    param([string]$FileName)
    foreach ($dir in $searchDirs) {
        $candidate = Join-Path $dir $FileName
        if (Test-Path $candidate) { return $candidate }
    }
    return $null
}

$resolved = New-Object System.Collections.Generic.List[string]

foreach ($p in $Paths) {
    if (-not (Test-Path $p)) { throw "CLAP not found: $p" }
    $resolved.Add((Resolve-Path $p).Path) | Out-Null
}

if ($Plugins.Count -eq 1 -and $Plugins[0] -match ",") {
    $Plugins = $Plugins[0] -split "," | ForEach-Object { $_.Trim() } | Where-Object { $_ }
}

if ($Plugins.Count -eq 0 -and $Paths.Count -eq 0) {
    $Plugins = @($clapFiles.Keys)
}

foreach ($id in $Plugins) {
    $key = $id.ToLowerInvariant()
    if (-not $clapFiles.Contains($key)) {
        throw "Unknown plugin id '$id'. Known: $($clapFiles.Keys -join ', ')"
    }
    $found = Resolve-ClapPath $clapFiles[$key]
    if (-not $found) {
        Write-Warning "Skip $id — $($clapFiles[$key]) not found in: $($searchDirs -join '; ')"
        continue
    }
    if (-not $resolved.Contains($found)) {
        $resolved.Add($found) | Out-Null
    }
}

if ($resolved.Count -eq 0) {
    throw "No CLAP files to validate. Install first: cargo aura install --clap -plug <name>"
}

# Always serial on Windows. Elsewhere default jobs are fine; still use -j 1
# for a deterministic ship gate across platforms.
$jobs = 1
Write-Host "clap-validator -j $jobs  ($($resolved.Count) plugin(s))" -ForegroundColor Cyan

$failed = 0
foreach ($clap in $resolved) {
    Write-Host ""
    Write-Host "=== $clap ===" -ForegroundColor Yellow
    $args = @("validate", "-j", "$jobs", $clap)
    if ($Include) { $args += @("-t", $Include) }
    if ($OnlyFailed) { $args += "--only-failed" }
    & clap-validator @args
    if ($LASTEXITCODE -ne 0) {
        $failed++
        Write-Host "FAILED: $clap (exit $LASTEXITCODE)" -ForegroundColor Red
    } else {
        Write-Host "OK: $clap" -ForegroundColor Green
    }
}

if ($failed -gt 0) {
    throw "$failed plugin(s) failed clap-validator"
}

Write-Host ""
Write-Host "All $($resolved.Count) plugin(s) passed clap-validator (-j $jobs)." -ForegroundColor Green
