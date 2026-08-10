# build-local-zip.ps1
# Builds release CLAPs via cargo-aura, packages Plugin-vX.Y.Z-{win|linux}.zip
#
# Default: Aether, Meridian, Equilibrium, Lucent, Lucent Relay x both platforms.
# Mensor still map if passed explicitly - not default (WIP).
# Lucent in the set also builds Lucent-Bundle (Lucent + Lucent Relay together).
#
# Usage:
#   .\build-local-zip.ps1                              # full shipping set, win+linux
#   .\build-local-zip.ps1 -Platform win                # Windows only
#   .\build-local-zip.ps1 -Platform linux              # Linux cross only
#   .\build-local-zip.ps1 -Plugins aether,meridian     # subset
#
# Linux cross (from Windows) requires:
#   winget install zig.zig --source winget
#   cargo install cargo-zigbuild
#   rustup target add x86_64-unknown-linux-gnu
#   cargo-aura (installed from AURA repo)
# Zig linker is wired in .cargo/config.toml + .cargo/zigcc-*.bat
#
# Output:
#   dist/<ClapName>-vX.Y.Z-win.zip
#   dist/<ClapName>-vX.Y.Z-linux.zip
#   dist/Lucent-Bundle-vX.Y.Z-{win|linux}.zip  (when lucent is in the set)

param(
    [string[]]$Plugins = @(
        "aether",
        "meridian",
        "equilibrium",
        "lucent",
        "lucent-relay"
    ),
    [ValidateSet("win", "linux", "both")]
    [string]$Platform = "both",
    [string]$LinuxTarget = "x86_64-unknown-linux-gnu"
)

$ErrorActionPreference = "Stop"
$distDir = "dist"

# CLAP product names (must match aura.toml bundle_id → display name)
$clapNames = @{
    "aether"       = "Aether"
    "mensor"       = "Mensor"
    "equilibrium"  = "Equilibrium"
    "meridian"     = "Meridian"
    "lucent"       = "Lucent"
    "lucent-relay" = "Lucent Relay"
}

# Accept comma-separated single arg: .\build-local-zip.ps1 aether,meridian
if ($Plugins.Count -eq 1 -and $Plugins[0] -match ",") {
    $Plugins = $Plugins[0] -split "," | ForEach-Object { $_.Trim() } | Where-Object { $_ }
}

# Lucent-Bundle needs both CLAPs: if lucent is selected, ensure lucent-relay is built too.
if ($Plugins -contains "lucent" -and $Plugins -notcontains "lucent-relay") {
    $Plugins = @($Plugins) + @("lucent-relay")
    Write-Host "Auto-added lucent-relay (required for Lucent-Bundle)" -ForegroundColor Yellow
}

$platforms = if ($Platform -eq "both") { @("win", "linux") } else { @($Platform) }

function Initialize-LinuxToolchain {
    param([string]$Target)
    if (-not (Get-Command zig -ErrorAction SilentlyContinue)) {
        Write-Warning "zig not on PATH. Install: winget install zig.zig --source winget"
    }
    if (-not (Get-Command cargo-zigbuild -ErrorAction SilentlyContinue)) {
        Write-Warning "cargo-zigbuild not on PATH. Install: cargo install cargo-zigbuild"
    }
    $rustupShow = & rustup target list --installed 2>$null
    if ($rustupShow -notcontains $Target) {
        Write-Host "Adding rustup target $Target ..." -ForegroundColor Yellow
        rustup target add $Target
        if ($LASTEXITCODE -ne 0) { throw "rustup target add failed" }
    }
}

function Get-PluginVersion {
    param([string]$CargoToml)
    $line = Select-String -Path $CargoToml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if (-not $line) { throw "No version in $CargoToml" }
    return $line.Matches[0].Groups[1].Value
}

function Build-And-Package {
    param(
        [string[]]$PluginList,
        [string]$Plat,
        [string]$Target,
        [hashtable]$Names,
        [string]$OutDir
    )

    $suffix = if ($Plat -eq "linux") { "linux" } else { "win" }
    $bundlesDir = if ($Plat -eq "linux") {
        Join-Path "target\bundles" $Target
    } else {
        "target\bundles"
    }

    $list = $PluginList -join ", "
    Write-Host "=== Building: $list  [$Plat] ===" -ForegroundColor Cyan

    if ($Plat -eq "linux") {
        Initialize-LinuxToolchain -Target $Target
        # Skip fontconfig pkg-config sysroot (Windows host has none).
        # Runtime dlopen of libfontconfig on Linux DAW machines.
        # Also set in .cargo/config.toml [env] for plain cargo --target builds.
        $env:RUST_FONTCONFIG_DLOPEN = "on"
    }

    # cargo-aura builds one plugin at a time; copy the cdylib into target\bundles
    # and rename to <DisplayName>.clap for packaging.
    foreach ($p in $PluginList) {
        Write-Host "--- cargo aura build --clap -plug $p [$Plat] ---" -ForegroundColor DarkCyan
        $buildArgs = @("aura", "build", "--clap", "-plug", $p, "--release")
        if ($Plat -eq "linux") {
            $buildArgs += @("--target", $Target)
        }
        & cargo @buildArgs
        if ($LASTEXITCODE -ne 0) { throw "Build failed ($Plat / $p)" }

        $clapName = $Names[$p]
        $crateName = $p -replace '-', '_'
        $src = if ($Plat -eq "linux") {
            Join-Path "target" $Target | Join-Path -ChildPath "release" | Join-Path -ChildPath "lib$crateName.so"
        } else {
            Join-Path "target" "release" | Join-Path -ChildPath "$crateName.dll"
        }
        $dstDir = $bundlesDir
        New-Item -ItemType Directory -Force -Path $dstDir | Out-Null
        $dst = Join-Path $dstDir ($clapName + ".clap")
        if (-not (Test-Path $src)) {
            throw "Built artifact not found: $src"
        }
        Copy-Item -Path $src -Destination $dst -Force
        Write-Host "  -> $dst" -ForegroundColor DarkGray
    }

    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    Write-Host "=== Packaging -> $OutDir (bundles: $bundlesDir) ===" -ForegroundColor Cyan

    foreach ($plugin in $PluginList) {
        $cargoToml = "plugins/$plugin/Cargo.toml"
        if (-not (Test-Path $cargoToml)) {
            Write-Warning "Skipping $plugin - no Cargo.toml"
            continue
        }
        if (-not $Names.ContainsKey($plugin)) {
            Write-Warning "Skipping $plugin - unknown clap name mapping"
            continue
        }

        $ver = Get-PluginVersion -CargoToml $cargoToml
        $clapName = $Names[$plugin]
        $clapPath = Join-Path $bundlesDir ($clapName + ".clap")
        $zipBase = ($clapName -replace '[()\s]+', '-').Trim('-')
        $zipName = "$zipBase-v$ver-$suffix.zip"
        $zipPath = Join-Path $OutDir $zipName

        if (-not (Test-Path $clapPath)) {
            Write-Error "Bundle not found: $clapPath"
            continue
        }

        if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
        Compress-Archive -Path $clapPath -DestinationPath $zipPath -Force
        Write-Host "  $zipName" -ForegroundColor Green
    }

    # Lucent bundle: Lucent + Lucent Relay in one zip (version = Lucent)
    if ($PluginList -contains "lucent") {
        $lucentVer = Get-PluginVersion -CargoToml "plugins/lucent/Cargo.toml"
        $bundleZip = Join-Path $OutDir ("Lucent-Bundle-v$lucentVer-$suffix.zip")
        $lucentClap = Join-Path $bundlesDir "Lucent.clap"
        $relayClap = Join-Path $bundlesDir "Lucent Relay.clap"
        if ((Test-Path $lucentClap) -and (Test-Path $relayClap)) {
            if (Test-Path $bundleZip) { Remove-Item $bundleZip -Force }
            Compress-Archive -Path $lucentClap, $relayClap -DestinationPath $bundleZip -Force
            Write-Host "  Lucent-Bundle-v$lucentVer-$suffix.zip" -ForegroundColor Green
        } else {
            Write-Warning "Lucent bundle skipped - missing Lucent and/or Lucent Relay .clap"
        }
    }
}

foreach ($plat in $platforms) {
    Build-And-Package `
        -PluginList $Plugins `
        -Plat $plat `
        -Target $LinuxTarget `
        -Names $clapNames `
        -OutDir $distDir
}

$platLabel = $platforms -join "+"
Write-Host "=== Done: $distDir  [$platLabel] ===" -ForegroundColor Cyan
