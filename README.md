# lx-audiolabs-slint

Slint UI migration workspace for LX Audiolabs plugins.

## Layout

```
crates/                 # libraries
  lx-slint-build/       # @truce widgets + font (build)
  lx-ui-slint/          # design system
  lx-dsp/ lx-analysis/ lx-vault/ lx-shm/
plugins/                # products (truce / lx-slint-editor)
  lucent-relay-slint/ lucent-slint/
  aurum-slint/
  aether/ meridian/ equilibrium/
```
Runtime GUI: **truce-slint** (software + wgpu present). Future GPU path if needed: Slint `renderer-skia`, not a local FemtoVG bridge.

## Shared UI (`crates/lx-ui-slint`)

Brand CI design system for all Slint plugins:

| Module | Contents |
|--------|----------|
| `ui/lx-theme.slint` | `Lx` global — colors, radii, type, spacing |
| `ui/lx-chrome.slint` | Panel, section, header, monitor strip, toggle/danger buttons |
| `ui/lx-controls.slint` | Custom compact `LxKnob`; `@truce` wrappers for `LxSlider`/`LxToggle`/`LxDropdown` + `LxBandColumn` |
| `ui/lx-meters.slint` | Stereo meter (frozen v0.2), GR, correlation, level panel |
| `ui/lx-viz.slint` | Spectrum, EQ curve, goniometer (grids + labels) |
| `ui/lx.slint` | Barrel re-export |

Import from a plugin:

```slint
// path relative to the plugin's ui/main.slint
import {
    Lx, LxHeader, LxKnob, LxSpectrum, LxGoniometer, LxLevelPanel,
} from "../../../crates/lx-ui-slint/ui/lx.slint";
```

## Plugins

- `plugins/aether` — Aether EQ + crossfeed
- `plugins/meridian` — Meridian channel strip
- `plugins/equilibrium` — Equilibrium pre-master spectral balancer

## Build

```powershell
cargo build -p aether
cargo build -p meridian
cargo build -p equilibrium
```
