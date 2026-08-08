# lx-audiolabs-slint

[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](LICENSE)
[![Slint](https://img.shields.io/badge/Slint-1.17.1-2379F4.svg)](https://slint.dev)
[![agal](https://img.shields.io/badge/powered%20by-agal-00ADD8.svg)](https://github.com/LX-Audiolabs/agal)
[![AI](https://img.shields.io/badge/dev-AI--assisted-6E40C9.svg)](https://github.com/LX-Audiolabs/agal)

Slint UI workspace for LX Audiolabs plugins (CLAP / VST3 / LV2 via truce).

## Layout

```
crates/                 # libraries
  lx-slint-build/       # build helper: @truce widgets + bundled JetBrains Mono (all plugin build.rs)
  lx-slint-editor/      # truce::Editor on slint-baseview
  lx-ui-slint/          # design system (Lx* components)
  lx-dsp/ lx-analysis/ lx-vault/ lx-shm/
plugins/                # products
  aether/ meridian/ equilibrium/
  lucent/ lucent-relay/
  aurum/
```

Runtime GUI: **lx-slint-editor** + **lx-slint-baseview** (default `backend-femtovg` + OpenGL). Optional A/B: `backend-skia`, `backend-wgpu`, `backend-wgpu-vulkan`.

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
| `aurum` | All-in-one mastering (M/S EQ, comp, sat, limiters) |

## Build

```powershell
# Workspace check
cargo check --workspace

# Single plugin CLAP install (release, Windows host)
cargo truce install --clap -p meridian

# Package release ZIPs → dist/  (default: Aether+Meridian+Equilibrium+Lucent+Relay × win+linux)
# also writes Lucent-Bundle-vX.Y.Z-{win|linux}.zip when lucent is in the set
.\build-local-zip.ps1
.\build-local-zip.ps1 -Platform win           # Windows only
.\build-local-zip.ps1 -Platform linux         # Linux cross only
.\build-local-zip.ps1 -Plugins aether,meridian  # subset only
```

### Linux CLAPs (from Windows)

Cross-compile via Zig (already wired in `.cargo/`):

```powershell
winget install zig.zig --source winget
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-gnu

# one plugin
cargo truce build --clap -p aether --target x86_64-unknown-linux-gnu
# → target\bundles\x86_64-unknown-linux-gnu\Aether.clap

# or zip packaging (default already includes linux via -Platform both)
.\build-local-zip.ps1 -Platform linux
# → dist\*-vX.Y.Z-linux.zip
```

Cross-builds: `lx-slint-editor` enables `fontique/fontconfig-dlopen` on Linux
targets, and `.cargo/config.toml` sets `RUST_FONTCONFIG_DLOPEN=on`, so no
Linux fontconfig sysroot / pkg-config is needed. Fontconfig is loaded at
runtime (`libfontconfig.so.1`) on the Linux host.

Native Linux CI: GitHub Actions → **Build Linux CLAPs** (`workflow_dispatch`).

### Dependencies

| Dep | Source |
|-----|--------|
| truce | path `../truce-dev` (local) / clone for CI |
| lx-slint-baseview | public git [`lxndrbe/lx-slint-baseview`](https://github.com/lxndrbe/lx-slint-baseview) |

Local baseview edits: uncomment the `[patch]` block at the bottom of root `Cargo.toml`.
