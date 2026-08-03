# build-local-zip.ps1
# Builds plugins via cargo-truce, packages each as Plugin-vX.Y.Z-{win|linux}.zip
#
# Usage:
#   .\build-local-zip.ps1                              # all plugins, host Windows
#   .\build-local-zip.ps1 aether,meridian
#   .\build-local-zip.ps1 -Platform linux              # cross → x86_64-unknown-linux-gnu
#   .\build-local-zip.ps1 -Platform linux -Plugins equilibrium
#
# Linux cross (from Windows) requires:
#   winget install zig.zig --source winget
#   cargo install cargo-zigbuild
#   rustup target add x86_64-unknown-linux-gnu
#   cargo-truce with --target-aware artifact names (truce ≥ 6.1.9 / truce-dev)
# Zig linker is wired in .cargo/config.toml + .cargo/zigcc-*.bat
#
# Output:
#   dist/<ClapName>-vX.Y.Z-win.zip
#   dist/<ClapName>-vX.Y.Z-linux.zip
#   dist/Lucent-Bundle-vX.Y.Z-{win|linux}.zip  (when lucent is in the set)

param(
    [string[]]$Plugins = @(
        "aether",
        "aurum-slint",
        "equilibrium",
        "lucent",
        "lucent-relay",
        "meridian"
    ),
    [ValidateSet("win", "linux")]
    [string]$Platform = "win",
    [string]$LinuxTarget = "x86_64-unknown-linux-gnu"
)

$ErrorActionPreference = "Stop"
$distDir = "dist"

# CLAP product names from truce.toml (cargo-truce safe_filename / file_stem)
$clapNames = @{
    "aether"       = "Aether"
    "aurum-slint"  = "Aurum (Slint)"
    "equilibrium"  = "Equilibrium"
    "meridian"     = "Meridian"
    "lucent"       = "Lucent"
    "lucent-relay" = "Lucent Relay"
}

# Accept comma-separated single arg: .\build-local-zip.ps1 aether,meridian
if ($Plugins.Count -eq 1 -and $Plugins[0] -match ",") {
    $Plugins = $Plugins[0] -split "," | ForEach-Object { $_.Trim() } | Where-Object { $_ }
}

$suffix = if ($Platform -eq "linux") { "linux" } else { "win" }
$bundlesDir = if ($Platform -eq "linux") {
    "target\bundles\$LinuxTarget"
} else {
    "target\bundles"
}

Write-Host "=== Building plugins: $($Plugins -join ', ')  [$Platform] ===" -ForegroundColor Cyan

if ($Platform -eq "linux") {
    if (-not (Get-Command zig -ErrorAction SilentlyContinue)) {
        Write-Warning "zig not on PATH. Install: winget install zig.zig --source winget"
    }
    if (-not (Get-Command cargo-zigbuild -ErrorAction SilentlyContinue)) {
        Write-Warning "cargo-zigbuild not on PATH. Install: cargo install cargo-zigbuild"
    }
    $rustupShow = & rustup target list --installed 2>$null
    if ($rustupShow -notcontains $LinuxTarget) {
        Write-Host "Adding rustup target $LinuxTarget ..." -ForegroundColor Yellow
        rustup target add $LinuxTarget
        if ($LASTEXITCODE -ne 0) { throw "rustup target add failed" }
    }
}

$pkgArgs = @()
foreach ($p in $Plugins) { $pkgArgs += @("-p", $p) }

$buildArgs = @("truce", "build", "--clap") + $pkgArgs
if ($Platform -eq "linux") {
    $buildArgs += @("--target", $LinuxTarget)
}

& cargo @buildArgs
if ($LASTEXITCODE -ne 0) { throw "Build failed" }

New-Item -ItemType Directory -Force -Path $distDir | Out-Null

Write-Host "=== Packaging ZIPs → $distDir (bundles: $bundlesDir) ===" -ForegroundColor Cyan

foreach ($plugin in $Plugins) {
    $cargoToml = "plugins/$plugin/Cargo.toml"
    if (-not (Test-Path $cargoToml)) {
        Write-Warning "Skipping $plugin — no Cargo.toml"
        continue
    }
    if (-not $clapNames.ContainsKey($plugin)) {
        Write-Warning "Skipping $plugin — unknown clap name mapping"
        continue
    }

    $ver = (Select-String '^version\s*=\s*"' $cargoToml | Select-Object -First 1).Line -replace '.*"(.+)".*', '$1'
    $clapName = $clapNames[$plugin]
    $clapPath = Join-Path $bundlesDir "$clapName.clap"
    # ZIP basename: spaces/parens → hyphens for download-friendly names
    $zipBase = ($clapName -replace '[()\s]+', '-').Trim('-')
    $zipName = "$zipBase-v$ver-$suffix.zip"
    $zipPath = Join-Path $distDir $zipName

    if (-not (Test-Path $clapPath)) {
        Write-Error "Bundle not found: $clapPath"
        continue
    }

    if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
    Compress-Archive -Path $clapPath -DestinationPath $zipPath -Force
    Write-Host "  $zipName" -ForegroundColor Green
}

# Lucent bundle special: Lucent + Lucent Relay together
if ($Plugins -contains "lucent" -and $Plugins -contains "lucent-relay") {
    $lucentVer = (Select-String '^version\s*=\s*"' "plugins/lucent/Cargo.toml" | Select-Object -First 1).Line -replace '.*"(.+)".*', '$1'
    $bundleZip = Join-Path $distDir "Lucent-Bundle-v$lucentVer-$suffix.zip"
    $lucentClap = Join-Path $bundlesDir "Lucent.clap"
    $relayClap = Join-Path $bundlesDir "Lucent Relay.clap"
    if ((Test-Path $lucentClap) -and (Test-Path $relayClap)) {
        if (Test-Path $bundleZip) { Remove-Item $bundleZip -Force }
        Compress-Archive -Path $lucentClap, $relayClap -DestinationPath $bundleZip -Force
        Write-Host "  Lucent-Bundle-v$lucentVer-$suffix.zip" -ForegroundColor Green
    } else {
        Write-Warning "Lucent bundle skipped — missing Lucent and/or Lucent Relay .clap"
    }
} elseif ($Plugins -contains "lucent") {
    # Lucent alone: still try to include relay if already built
    $lucentVer = (Select-String '^version\s*=\s*"' "plugins/lucent/Cargo.toml" | Select-Object -First 1).Line -replace '.*"(.+)".*', '$1'
    $bundleZip = Join-Path $distDir "Lucent-Bundle-v$lucentVer-$suffix.zip"
    $lucentClap = Join-Path $bundlesDir "Lucent.clap"
    $relayClap = Join-Path $bundlesDir "Lucent Relay.clap"
    if ((Test-Path $lucentClap) -and (Test-Path $relayClap)) {
        if (Test-Path $bundleZip) { Remove-Item $bundleZip -Force }
        Compress-Archive -Path $lucentClap, $relayClap -DestinationPath $bundleZip -Force
        Write-Host "  Lucent-Bundle-v$lucentVer-$suffix.zip" -ForegroundColor Green
    }
}

Write-Host "=== Done: $distDir ===" -ForegroundColor Cyan
