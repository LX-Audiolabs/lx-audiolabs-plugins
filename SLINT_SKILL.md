# Slint Development Skill — LX Audiolabs

Slint **1.17.1**, software renderer (`renderer-software`), Rust interop via `truce-slint`.
Target: DAW audio plugins (CLAP/VST3/LV2), embedded in host windows via baseview + wgpu present.

> **Docs authority:** `https://releases.slint.dev/1.17.1/docs/` — trust docs over this file for exact signatures.
> Quick check: `slint-viewer --check <file>.slint` (compile without window).

---

## Project Context

### Stack
- **Slint:** `=1.17.1` (workspace pin, `renderer-software` + `compat-1-2` + `std`)
- **truce-slint:** `SlintEditor<P>` — wraps Slint `MinimalSoftwareWindow`, blits CPU framebuffer → wgpu texture → baseview
- **No GPU rendering.** Software only. `SLINT_BACKEND` not used; backend set programmatically by truce-slint.
- **No MCP server.** DAW plugin context can't expose TCP ports. Use `slint-viewer --screenshot` for visual checks.

### Build
```sh
# Build a single plugin (CLAP)
cargo truce install --clap -p lucent-slint

# Check .slint files without building Rust
slint-viewer --check crates/lx-slint-build/ui/widgets.slint

# Preview a component standalone
slint-viewer crates/lx-slint-build/ui/knob.slint

# Screenshot a component (headless, no display needed)
slint-viewer --screenshot out.png crates/lx-slint-build/ui/meter.slint
```

### Crate Map
| Crate | Purpose |
|-------|---------|
| `lx-slint-build` | Build helper: `@truce` widget `.slint` files + font embedding |
| `lx-ui-slint` | Shared UI library (re-exports `lx-slint-build` widgets for plugins) |
| `plugins/*-slint/` | Individual plugin crates |

### Widgets (`@truce`)
Import: `import { Knob, Meter, ParamSlider, Toggle, Dropdown, XYPad } from "@truce";`

| Widget | File | Key properties |
|--------|------|----------------|
| `Knob` | `knob.slint` | `value`, `default-value`, `label`, `changed(float)` |
| `Meter` | `meter.slint` | `value` (0..1), `peak` (0..1), `label` |
| `ParamSlider` | `slider.slint` | `value`, `minimum`, `maximum`, `label`, `value-text`, `changed(float)` |
| `Toggle` | `toggle.slint` | `checked`, `label`, `toggled(bool)` |
| `Dropdown` | `dropdown.slint` | `current-index`, `model: [string]`, `selected(int, string)` |
| `XYPad` | `xy_pad.slint` | `x`, `y` (0..1), `changed(float, float)` |

---

## `.slint` Language — What Agents Get Wrong

### Properties & Bindings
```slint
// REACTIVE binding (auto re-evaluates):  name: expr;
// IMPERATIVE assignment (only inside callbacks/functions):  name = expr;

component Foo {
    in property <float> input;           // host writes, component reads
    out property <float> output;         // component writes, host reads
    in-out property <float> shared;      // both read/write
    private property <bool> internal;    // component-only

    // Reactive binding — recomputes when `input` changes
    scaled: input * 2.0;

    callback changed(float);             // declare
    changed(v) => { root.output = v; }   // handle (named params, NOT positional)

    // Two-way bind: a <=> b
}
```

### String Interpolation
```slint
// CORRECT — backslash-brace:
Text { text: "Value: \{root.value}"; }
// WRONG — literal ${name} shows in UI, no compiler error:
// Text { text: "Value: ${root.value}"; }
```

### Ids & Scoping
```slint
// Id assignment (NOT `id:` property):
foo := Rectangle { /* ... */ }

// Resolution: bare `foo` → nearest in scope. Qualify for explicitness:
root.foo, parent.bar

// `self` = current element, `root` = component root
```

### Control Flow
```slint
if cond : Element { }
for item[idx] in model : Element { }   // idx optional
for i in 5 : Element { }               // iterates 0..4

// Ternary in expressions:
color: root.active ? #ff6600 : #444444;
```

### Callbacks vs Functions vs Pure
```slint
callback foo(int) -> string;            // host handles
pure callback bar(int) -> int;          // callable from bindings (no side effects)
pure function add(a: int, b: int) -> int { return a + b; }

// Call from binding only if `pure`:
result: root.bar(root.value);           // OK — pure callback
// result: root.foo(root.value);        // ERROR — not pure
```

---

## Layout & Sizing — Read Before Fighting

### Fill vs Preferred Size
| Element type | Default behavior |
|-------------|-----------------|
| `Rectangle`, `TouchArea`, `FocusScope`, all layouts (`VerticalLayout`, etc.) | **Fill parent** |
| `Text`, `Image` | **Preferred size** (content-fit) |
| Custom component | Inherits root element's behavior |

```slint
// Stretch non-filling element:
Text { width: 100%; height: 100%; }

// Collapse filling element to content:
component Panel inherits Rectangle {
    height: layout.preferred-height;  // content-sized, not filling
}
```

### Padding & Spacing
```slint
// `padding`/`spacing` ONLY work on layout elements. NOT on Rectangle/Text directly.
// WRONG:
Text { padding: 6px; }  // WARNING: "padding only has effect on layout elements"

// CORRECT — wrap in layout:
HorizontalLayout { padding-left: 6px; Text { "..." } }
```

### Centering & Positioning
- **Outside a layout:** element with implicit size and no `x`/`y` → centered in parent.
- **Inside a layout:** `x`/`y` ignored. Use `alignment` properties or spacers.
- **Spacer:** stretched `Rectangle { }` takes remaining space.
```slint
HorizontalLayout {
    Text { "Left" }
    Rectangle { horizontal-stretch: 1; }  // spacer
    Text { "Right" }
}
```

### Z-order
Later siblings render on top. No `z-index` property.

---

## Common Compile Errors & Gotchas

### Unit & Type Errors
```
"Cannot convert float to length. Use an unit, or multiply by 1px to convert explicitly"
```
→ Slint has typed units. Convert: `value * 1px`, `len / 1px`, `* 1deg`.

```
"Invalid unit 'em'"
```
→ Slint has no `em`. Use `rem` for font-relative spacing.

```
"Unknown unqualified identifier 'hsl'"
```
→ No `hsl()`. Use `hsv()`, `rgb()`, `rgba()`, `oklch()`, hex literals.

### Math
```slint
// `/` always returns float. Assigning to int truncates toward zero.
property <int> half: (root.value / 2).floor();  // explicit rounding

// Builtins: floor(), ceil(), round(), max(), min(), clamp(), abs(), sqrt(), sin(), cos(), …
// Methods: x.floor(), x.round(), x.clamp(lo, hi)
```

### Colors
```slint
// Hex, rgb(), rgba() (alpha 0..1), hsv(), oklch()
color: #ff6600;
color: rgb(255, 102, 0);
color: Palette.foreground.transparentize(0.4);  // derive from palette
```

### Enum Values
```slint
// EnumName.value, lowercase for builtins:
PointerEventKind.down, ColorScheme.dark, Key.Escape, Key.UpArrow
```

### Animations
```slint
// Declared ON the property, INSIDE the element whose property changes:
animate width { duration: 200ms; easing: ease-in-out; }
```

### Gradients
```slint
@linear-gradient(90deg, #ff6600 0%, #ff0066 100%)
@radial-gradient(circle, #fff 0%, #000 100%)
```

### Performance
- Use `ListView` for long lists (virtualizes). `for` inside `ScrollView` instantiates every row.
- `SLINT_DEBUG_PERFORMANCE=refresh_lazy,console` prints frame diagnostics.

---

## Rust Interop (truce-slint specific)

### truce-slint Pattern
```rust
use truce_slint::SlintEditor;

// In plugin activate():
let editor = SlintEditor::new(
    &params,
    (600, 400),  // min size
    |ctx: PluginContext<MyParams>| {
        // ctx gives read access to params, queue, host info
        // Return initialization closure result
    }
);
```

### Global State Pattern
```slint
// globals.slint
export global PluginState {
    in property <float> gain;
    in property <bool> bypass;
    callback param-changed(string, float);
}
```
```rust
// Rust side
ui.global::<PluginState>().set_gain(0.5);
ui.global::<PluginState>().on_param_changed(|name, value| { /* ... */ });
```

### Naming Convention
- Slint kebab-case → Rust snake_case
- `row-clicked` → `on_row_clicked()`
- Property `foo-bar` → setter `set_foo_bar()`

### Thread Safety
```rust
// From audio thread, use invoke_from_event_loop:
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
// [T] in .slint → ModelRc<T>
let model: ModelRc<i32> = ModelRc::new(VecModel::from(vec![1, 2, 3]));
// For live updates, keep Rc<VecModel<T>> and mutate:
let model = std::rc::Rc::new(VecModel::default());
model.push(42);
```

### Type Mapping
| Slint | Rust |
|-------|------|
| `int` | `i32` |
| `float`, `length` | `f32` |
| `string` | `slint::SharedString` |
| `color`, `brush` | `slint::Brush` / `slint::Color` |
| `bool` | `bool` |
| `[T]` | `slint::ModelRc<T>` |

---

## Input Handling

### Mouse/Touch
```slint
TouchArea {
    clicked => { /* modifier-agnostic */ }
    pointer-event(ev) => {
        if ev.kind == PointerEventKind.down && ev.button == PointerEventButton.right {
            // right-click
        }
    }
    moved => { /* drag */ }
    has-hover => { /* cursor over */ }
    mouse-cursor: root.active ? MouseCursor.pointer : MouseCursor.default;
}
```

### Keyboard
```slint
// FocusScope captures keys the focused child REJECTED:
FocusScope {
    key-pressed(ev) => {
        if ev.text == Key.Escape { accept } else { reject }
    }
    // capture-key-pressed runs BEFORE focused child (for global shortcuts):
    capture-key-pressed(ev) => {
        if ev.modifiers.control && ev.text == "a" { /* Ctrl+A */ accept } else { reject }
    }
}
```

### Context Menus
```slint
// Use builtin ContextMenuArea (no import needed):
ContextMenuArea {
    MenuEntry { text: "Copy"; triggered => { /* ... */ } }
    MenuEntry { text: "Paste"; triggered => { /* ... */ } }
}
```

---

## Visual Polish Checklist

Before calling UI work done:

1. **Spacing:** consistent multiples of 4px/8px. Prefer `VerticalBox`/`HorizontalBox` widgets.
2. **Alignment:** `GridLayout` for form layouts, not nested boxes with eyeballed widths.
3. **Colors:** from `Palette` or a single `Theme` global, not scattered hex literals.
4. **Text sizes:** use `rem`, small scale (body/secondary/heading), not `px` per label.
5. **Hover states:** set `mouse-cursor`, animate background/opacity 150-200ms on `has-hover`.
6. **Screenshot check:** render with `slint-viewer --screenshot` and review for: clipped text, edge-touching elements, misaligned baselines, inconsistent gaps.

---

## Debugging

```slint
// Print to stderr at runtime:
debug("value is", root.value);

// Element tree inspection (via slint-viewer):
slint-viewer --check ui/main.slint

// Take screenshot programmatically (Rust):
let buffer = ui.window().take_snapshot()?;
```

---

*Generated from slint-ui/ai-plugins reference. Adapted for LX Audiolabs Slint 1.17.1 / truce-slint / DAW plugin context. 2026-07-20.*
