# lx-audiolabs-slint

Slint UI workspace for LX Audiolabs plugins (CLAP / VST3 / LV2 via truce).

## Layout

```
crates/                 # libraries
  lx-slint-build/       # @truce widgets + font (build helper; unused by plugins today)
  lx-slint-editor/      # truce::Editor on slint-baseview
  lx-ui-slint/          # design system (Lx* components)
  lx-dsp/ lx-analysis/ lx-vault/ lx-shm/
plugins/                # products
  aether/ meridian/ equilibrium/
  lucent/ lucent-relay/
  aurum-slint/
```

Runtime GUI: **lx-slint-editor** + **slint-baseview** (default `backend-femtovg` + OpenGL). Optional A/B: `backend-skia`, `backend-wgpu`, `backend-wgpu-vulkan`.

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
| `aurum-slint` | Saturation / clipper / color / limit |

## Build

```powershell
# Workspace check
cargo check --workspace

# Single plugin CLAP install (release)
cargo truce install --clap -p meridian
```
