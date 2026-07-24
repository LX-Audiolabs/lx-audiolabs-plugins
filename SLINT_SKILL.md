# Slint Development Skill — LX Audiolabs

Slint **1.17.1**, multi-backend (femtovg, wgpu, wgpu-vulkan), baseview **0.3** (next).
Target: DAW audio plugins (CLAP/VST3/LV2), embedded in host windows.

> **`.slint` language reference:** See [slint-ui/ai-plugins](https://github.com/slint-ui/ai-plugins/blob/master/skills/slint/SKILL.md) — upstream skill with extensive `.slint` language docs, layout rules, gotchas. This file covers LX-Audiolabs-specific project context, backend choices, and interop.
> **Docs authority:** `https://releases.slint.dev/1.17.1/docs/` — trust docs over this file for exact signatures.
> **Quick check:** `slint-viewer --check <file>.slint` (compile without window).

---

## Project Context

### Stack

| Layer | Crate | Version | Notes |
|-------|-------|---------|-------|
| UI Toolkit | `slint` | `=1.17.1` | `renderer-software` + `compat-1-2` + `std` |
| Windowing | `baseview` | 0.3 (next, git) | Via slint-baseview (our fork) |
| Windowing (legacy) | `baseview-truce` | 0.1.1-truce.13 | Via truce-slint (truce-dev) |
| Editor adapter | `lx-slint-editor` | 0.1.0 | Our `truce::Editor` impl on slint-baseview |
| Editor adapter (legacy) | `truce-slint` | 6.3.0 | truce's SlintEditor, uses baseview-truce 0.1.1 |
| Build helper | `lx-slint-build` | 0.1.0 | `@truce` widgets + JetBrains Mono |
| Shared UI | `lx-ui-slint` | 0.1.0 | Re-exports lx-slint-build widgets for plugins |

### Rendering Backends (slint-baseview)

Our fork of slint-baseview (private, github.com/lxndrbe) supports three backends:

| Feature | Renderer | GPU-API | baseview dep | Notes |
|---------|----------|---------|-------------|-------|
| `backend-femtovg` (default) | FemtoVG | OpenGL | `baseview/opengl` | GPU-accelerated, GL context from baseview |
| `backend-wgpu` | Software | wgpu (DX12) | — | CPU render → wgpu blit, no OpenGL |
| `backend-wgpu-vulkan` | Software | wgpu (Vulkan) | — | Forces Vulkan backend, avoids DX12 crashes |

Skia (`renderer-skia`) is **not available** in slint 1.17.1 — `SkiaWGPURenderer` is gated behind `unstable-wgpu-29` and lacks a public `render()` for manual render loops. Target slint 2.x.

### No MCP server
DAW plugin context can't expose TCP ports. Use `slint-viewer --screenshot` for visual checks.

### Build

```sh
# Build a single plugin (CLAP)
cargo truce install --clap -p lucent-slint

# Check .slint files without building Rust
slint-viewer --check crates/lx-slint-build/ui/widgets.slint

# Preview / screenshot components
slint-viewer crates/lx-slint-build/ui/knob.slint
slint-viewer --screenshot out.png crates/lx-slint-build/ui/meter.slint
```

### Crate Map

```
lx-audiolabs-slint/
├── crates/
│   ├── lx-slint-build/     ← Build helper: @truce widgets + font embedding
│   ├── lx-slint-editor/    ← truce::Editor on slint-baseview (replaces truce-slint)
│   ├── lx-ui-slint/        ← Shared UI library
│   ├── lx-dsp/             ← DSP algorithms
│   ├── lx-analysis/        ← FFT, spectrum analysis
│   ├── lx-vault/           ← Preset management
│   └── lx-shm/             ← Shared memory (Lucent Relay)
├── plugins/
│   ├── lucent-slint/       ← Spectrum analyzer
│   ├── meridian-slint/     ← Channel strip
│   ├── aether-slint/       ← EQ
│   ├── equilibrium-slint/  ← Spectral balancer
│   ├── aurum-slint/        ← Saturation
│   └── lucent-relay-slint/ ← Relay sender
└── Cargo.toml              ← slint-baseview git-dep, baseview next
```

> **Migration status:** `lx-slint-editor` skeleton exists (compiles). Plugins still use `truce-slint`. Migration planned when editor adapter is wired.

### Widgets (`@truce`)

Import: `import { Knob, Meter, ParamSlider, Toggle, Dropdown, XYPad } from "@truce";`

| Widget | Properties |
|--------|-----------|
| `Knob` | `value`, `default-value`, `label`, `changed(float)` |
| `Meter` | `value` (0..1), `peak` (0..1), `label` |
| `ParamSlider` | `value`, `minimum`, `maximum`, `label`, `value-text`, `changed(float)` |
| `Toggle` | `checked`, `label`, `toggled(bool)` |
| `Dropdown` | `current-index`, `model: [string]`, `selected(int, string)` |
| `XYPad` | `x`, `y` (0..1), `changed(float, float)` |

---

## Rust Interop

### truce-slint Pattern (current, legacy)

```rust
use truce_slint::SlintEditor;

SlintEditor::new(params, (600, 400), |state: PluginContext<MyParams>| {
    let ui = MyPluginUi::new().unwrap();
    let s = state.clone();
    ui.on_gain_changed(move |v| s.automate(P::Gain, v as f64));
    Box::new(move |state: &PluginContext<MyParams>| {
        ui.set_gain(state.get_param(P::Gain));
    })
})
```

### lx-slint-editor Pattern (future)

```rust
use lx_slint_editor::LxSlintEditor;

LxSlintEditor::new(params, (600, 400), |state: PluginContext<MyParams>| {
    let ui = MyPluginUi::new().unwrap();
    let s = state.clone();
    ui.on_gain_changed(move |v| s.automate(P::Gain, v as f64));
    Box::new(move |state: &PluginContext<MyParams>| {
        ui.set_gain(state.get_param(P::Gain));
    })
}).resizable(true)
```

### Naming Convention
- Slint kebab-case → Rust snake_case: `row-clicked` → `on_row_clicked()`
- Property `foo-bar` → setter `set_foo_bar()`

### Thread Safety
```rust
let ui_weak = ui.as_weak();
slint::invoke_from_event_loop(move || {
    if let Some(ui) = ui_weak.upgrade() {
        ui.global::<PluginState>().set_gain(new_gain);
    }
}).ok();
```

### Models (Lists)
```rust
use slint::{ModelRc, VecModel};
let model: ModelRc<i32> = ModelRc::new(VecModel::from(vec![1, 2, 3]));
// For live updates:
let model = std::rc::Rc::new(VecModel::default());
model.push(42);
```

---

## baseview Versions — Quick Reference

| Crate | baseview | rwh | keyb-types | Notes |
|-------|----------|-----|------------|-------|
| `truce-slint` (truce-dev) | 0.1.1 (baseview-truce) | 0.5 | 0.7 | truce fork, AAX fix |
| `slint-baseview` (our fork) | 0.3 (next, git) | 0.6 | 0.8 | multi-backend |
| baseview upstream | 0.3 (pre-release) | 0.6 | 0.8 | Host callbacks, reentrancy |
| `lx-slint-editor` | 0.3 (next, git) | 0.6 | — | Our adapter |

**rwh bridge:** `lx-slint-editor/src/parent.rs` converts truce's `RawWindowHandle` to rwh 0.6 `HasWindowHandle` via raw platform pointers.

---

## Layout & Sizing — Quick Reference

| Element type | Default behavior |
|-------------|-----------------|
| `Rectangle`, `TouchArea`, layouts | **Fill parent** |
| `Text`, `Image` | **Preferred size** |
| Custom component | Inherits root |

- `padding`/`spacing` only on layout elements, NOT on Rectangle/Text
- `x`/`y` ignored inside layouts; use `alignment` or spacers
- Z-order: later siblings on top, no `z-index`
- String interpolation: `"\{root.value}"` — backslash-brace, NOT `${}`

## Common Gotchas

- `/` always returns float, truncate explicitly: `(x / 2).floor()`
- No `em` unit, use `rem`; no `hsl()`, use `hsv()` or `oklch()`
- `animate` goes ON the property INSIDE the element
- Use `ListView` for long lists (virtualizes); `for` in `ScrollView` is slow
