# lx-audiolabs-slint

[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](LICENSE)
[![Slint](https://img.shields.io/badge/Slint-1.17.1-2379F4.svg)](https://slint.dev)
[![agal](https://img.shields.io/badge/powered%20by-agal-00ADD8.svg)](https://github.com/LX-Audiolabs/agal)
[![AI](https://img.shields.io/badge/dev-AI--assisted-6E40C9.svg)](https://github.com/LX-Audiolabs/agal)

Slint UI workspace for LX Audiolabs plugins (CLAP / VST3 / LV2 via AURA).

> **Status:** truce → AURA cutover complete. All six plugins (aether,
> equilibrium, lucent, lucent-relay, mensor, meridian) build and install with
> `cargo aura`. The shared product UI lives in `lx-ui-slint` on top of
> `aura-editor` + `aura-baseview`.

## Layout

```
crates/                 # libraries
  lx-ui-slint/          # design system (Lx* components)
plugins/                # products
  aether/ meridian/ equilibrium/
  lucent/ lucent-relay/
  mensor/
```

Runtime GUI: **aura-editor** + **aura-baseview** (default `backend-femtovg` + OpenGL).

## System requirements (UI)

**INFO — GPU / OpenGL**

Default editor path is **FemtoVG** (custom path shaders). That needs a working
**OpenGL 3.2 Core** context (or newer). OpenGL ES 3.0+ with a solid driver is in
the same class; pure GL 2.x / ancient Mesa / broken plugin-host GL embeds are
**unsupported** — the UI may fail to open or crash the host (seen on old Linux +
REAPER). The older Vizia + Skia builds were softer on weak GL; FemtoVG is not.

A future **wgpu** backend would not revive those machines either (modern GPU API).

Audio processing does not depend on OpenGL — only the editor window does.

## Shared UI (`crates/lx-ui-slint`)

Brand CI design system for all Slint plugins:

| Module | Contents |
|--------|----------|
| `ui/lx-theme.slint` | `Lx` global — colors, radii, type, spacing |
| `ui/lx-chrome.slint` | Panel, section, tabs, toggle/danger buttons |
| `ui/lx-shell.slint` | `LxShellHeader` / body / sidebars / footer |
| `ui/lx-controls.slint` | `LxKnob`, `LxSlider`, band columns, line edit |
| `ui/lx-meters.slint` | LED peak meter, correlation, meter bar |
| `ui/lx-viz.slint` | Spectrum, EQ curve, goniometer |
| `ui/lx.slint` | Barrel re-export |

Import from a plugin:

```slint
// path relative to the plugin's ui/main.slint
import {
    Lx, LxShellHeader, LxKnob, LxSpectrum, LxGoniometer, LxLedPeakMeter,
} from "../../../crates/lx-ui-slint/ui/lx.slint";
```

## Plugins

| Crate | Role |
|-------|------|
| `aether` | EQ + crossfeed |
| `meridian` | Channel strip |
| `equilibrium` | Pre-master spectral balancer |
| `lucent` | Spectrum analyzer (standalone / hybrid / relay) |
| `lucent-relay` | Relay publisher |
| `mensor` | All-in-one mastering (M/S EQ, comp, sat, limiters) |

## Build

```powershell
# Workspace check (debug profile by default)
cargo check --workspace

# Single plugin CLAP install (release, Windows host)
cargo aura install --clap -plug meridian

# Multiple plugins at once
cargo aura install --clap -plug aether meridian equilibrium

# Build without --release uses the dev/debug profile.
# Add --release for optimized builds (used by install and build-local-zip.ps1).
cargo aura build --clap -plug aether

# Package release ZIPs → dist/  (default: Aether+Meridian+Equilibrium+Lucent+Relay × win+linux)
# also writes Lucent-Bundle-vX.Y.Z-{win|linux}.zip when lucent is in the set
.\build-local-zip.ps1
.\build-local-zip.ps1 -Platform win           # Windows only
.\build-local-zip.ps1 -Platform linux         # Linux cross only
.\build-local-zip.ps1 -Plugins aether,meridian  # subset only
```

### CLAP validate (ship gate)

```powershell
# After install (or point -Paths at a .clap). Always uses -j 1.
cargo install clap-validator   # once
cargo aura install --clap -plug lucent-relay
.\validate-clap.ps1 -Plugins lucent-relay
.\validate-clap.ps1            # all installed LX CLAPs found on disk
```

**Windows:** never run `clap-validator` with default/`-j > 1` parallelism as a
ship gate. Parallel out-of-process jobs can flake with `0xc0000005`
(ACCESS_VIOLATION), including Lucent Relay `param-fuzz-basic` /
`param-fuzz-bounds`. Serial (`-j 1`) is green; product process path is fine.
`validate-clap.ps1` forces `-j 1`.

### Linux CLAPs (from Windows)

Cross-compile via Zig (already wired in `.cargo/`):

```powershell
winget install zig.zig --source winget
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-gnu

# one plugin
cargo aura build --clap -plug aether --target x86_64-unknown-linux-gnu
# → target\x86_64-unknown-linux-gnu\release\libaether.so  (rename/copy to Aether.clap)

# or zip packaging (default already includes linux via -Platform both)
.\build-local-zip.ps1 -Platform linux
# → dist\*-vX.Y.Z-linux.zip
```

Cross-builds: `aura-editor` enables `fontique/fontconfig-dlopen` on Linux
targets, and `.cargo/config.toml` sets `RUST_FONTCONFIG_DLOPEN=on`, so no
Linux fontconfig sysroot / pkg-config is needed. Fontconfig is loaded at
runtime (`libfontconfig.so.1`) on the Linux host.

Native Linux CI: GitHub Actions → **Build Linux CLAPs** (`workflow_dispatch`).

### Dependencies

| Dep | Source |
|-----|--------|
| AURA | path `../AURA` (local) / clone for CI |

Local baseview edits: uncomment the `[patch]` block at the bottom of root `Cargo.toml`.
