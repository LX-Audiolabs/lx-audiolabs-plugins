# lx-audiolabs-slint

Slint UI migration workspace for LX Audiolabs plugins.

## Shared UI (`shared-ui-slint`)

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
} from "../../../shared-ui-slint/ui/lx.slint";
```

## Plugins

- `plugins/aether-slint` — Aether EQ + crossfeed (Slint)
- `plugins/meridian-slint` — Meridian channel strip (Slint)

## Build

```powershell
cargo build -p aether-slint
cargo build -p meridian-slint
```
