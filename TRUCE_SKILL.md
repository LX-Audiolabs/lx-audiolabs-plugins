# Truce Development Skill — LX Audiolabs (Slint)

truce **6.3.0**, Rust **Edition 2024**, CLAP/VST3/LV2 output.
Target: DAW audio plugins via `cargo truce install --clap`.

> **Full reference:** truce source in sibling `truce-dev/` workspace. `cargo check -p <plugin>` for quick verify.

---

## Build

```sh
cargo truce install --clap -p lucent-slint    # build + install one plugin
cargo check -p lucent-slint                   # verify without full build
cargo check --workspace                       # verify all plugins
```

Output lands in `%USERPROFILE%\Documents\LX-AudioLabs\CLAPins\`.

---

## Plugin Anatomy

```
plugins/<name>/
├── Cargo.toml       ← crate-type, features (clap/vst3/lv2), deps
├── truce.toml       ← (optional) plugin metadata
└── src/
    ├── lib.rs       ← Params, DspState, PluginLogic, truce::plugin!
    └── editor.rs    ← UI build (Slint)
```

---

## Threading Rules (Critical)

| Method | Thread | Rules |
|--------|--------|-------|
| `init()` | main | Allocate, don't touch audio |
| `reset()` | main | Init DSP state, sample rate, filters |
| `process()` | **audio** | Real-time. **No alloc, no lock, no I/O.** |
| `editor()` | main | Create UI |

- `process()`: Always `let _ftz = FtzDazGuard::new();` first — kills denormals.
- Cross-thread: use atomics (`AtomicBool`, `AtomicF32`), never `Mutex` from audio thread.

---

## Params

```rust
#[derive(Params)]
pub struct MyParams {
    #[param(name = "Gain", default = 0.0, range = "linear(-12, 12)", unit = "dB", smooth = "linear(20)")]
    pub gain: FloatParam,

    #[param(name = "Bypass", default = 0)]
    pub bypass: BoolParam,

    #[param(name = "Mode", default = 0, range = "discrete(0, 2)")]
    pub mode: IntParam,

    #[skip]
    pub shared: Arc<SharedState>,
}
```

- `range`: `"linear(min, max)"` | `"log(min, max)"` | `"discrete(min, max)"`
- `smooth`: `"linear(N)"` for audio-rate interpolation. Omit if not needed.
- `params.gain.value()` = smoothed. `params.gain.raw_target()` = immediate.
- `#[skip]` = not a CLAP parameter (shared state, caches, etc.)

---

## PluginLogic

```rust
impl PluginLogic for MyPlugin {
    type Params = MyParams;
    type DspState = MyDspState;

    fn bus_layouts() -> Vec<BusLayout> { vec![BusLayout::stereo()] }
    fn init(_p: &Params, _cx: &InitContext) -> DspState { DspState::default() }
    fn reset(state: &mut DspState, params: &Params, config: &AudioConfig);
    fn process(state: &mut DspState, params: &Params, buffer: &mut AudioBuffer, _events: &EventList, _ctx: &mut ProcessContext) -> ProcessStatus;
    fn load_state(_s: &mut DspState, _data: &[u8]) -> Result<(), StateLoadError> { Ok(()) }
    fn state_changed(_s: &mut DspState, _p: &Params) {}
    fn editor(params: Arc<Params>) -> Box<dyn Editor>;
}
```

### Bypass Pattern
```rust
if params.bypass.value() {
    for ch in 0..buffer.channels() {
        let (inp, out) = buffer.io(ch);
        out.copy_from_slice(inp);
    }
    return ProcessStatus::Normal;
}
```

---

## Editor (Slint)

### Current: truce-slint
```rust
use truce_slint::SlintEditor;

fn editor(params: Arc<Self::Params>) -> Box<dyn Editor> {
    SlintEditor::new(params, (800, 600), |state: PluginContext<MyParams>| {
        let ui = MyUi::new().unwrap();
        let s = state.clone();
        ui.on_gain_changed(move |v| s.automate(P::Gain, v as f64));
        Box::new(move |state: &PluginContext<MyParams>| {
            ui.set_gain(state.get_param(P::Gain));
        })
    }).into_editor()
}
```

### Future: lx-slint-editor
```rust
use lx_slint_editor::LxSlintEditor;

fn editor(params: Arc<Self::Params>) -> Box<dyn Editor> {
    LxSlintEditor::new(params, (800, 600), |state: PluginContext<MyParams>| {
        // same setup pattern as truce-slint
    }).resizable(true).into_editor()
}
```
See `SLINT_SKILL.md` for full Slint development rules.

---

## State & Migration

### load_state — Always Handle Legacy
```rust
fn load_state(_state: &mut DspState, data: &[u8]) -> Result<(), StateLoadError> {
    if let Some(_params) = state_migration::try_parse_niceplug_state(data) {
        // Handle old nice-plug format session data
        // In modern hosts, this path is legacy — params are already set.
    }
    Ok(())
}
```

### SharedState (Atomics for cross-thread)
```rust
use std::sync::atomic::Ordering;

// Write from audio thread (in process):
params.shared.sample_rate.store(sr, Ordering::Release);
params.shared.input_peak.store(in_db, Ordering::Release);

// Read from main thread (in editor):
let sr = shared.sample_rate.load(Ordering::Acquire);
```

---

## Shared Crates — Quick Reference

### `lx-dsp`
| Export | Purpose |
|--------|---------|
| `Biquad` | 2nd-order IIR: peaking EQ, shelves, HP/LP/BP, Butterworth |
| `Compressor` | Soft-knee compressor with attack/release |
| `LR2Crossover` | 2nd-order Linkwitz-Riley crossover |
| `TiltEq` | Shelving tilt EQ |
| `AutoLoudMeter` | LUFS/I loudness meter |
| `FtzDazGuard` | RAII guard: FTZ+DAZ on new, restore on drop |
| `state_migration` | Legacy nice-plug state parsing |
| `DBTP_CEILING` | Clamp constant for true-peak limiting |

### `lx-analysis`
| Export | Purpose |
|--------|---------|
| `SharedState` | Cross-thread atomics: sample_rate, input_peak, shm_slot, fft data |
| `SnapFFT` / `SnapMode` | FFT snapshot for scope display |
| `spectrum_physical_db()` | Convert FFT bins to dB |
| `relay_hub()` | SHM relay discovery |
| `SPECTRUM_BINS` / `SCOPE_BUFFER_LEN` | Buffer size constants |

### `lx-shm` (standalone)
Shared-memory IPC layer. Used by Lucent ↔ Lucent-Relay for inter-plugin spectrum routing.

---

## Build & Test

```powershell
# Install cargo-truce (one-time)
cargo install cargo-truce

# Build + install
cargo truce install --clap -p <plugin>

# Validate after build
clap-validator "C:\Users\lxndr\Documents\LX-AudioLabs\CLAPins\<plugin>.clap"

# Check without building
cargo check -p <plugin>

# Workspace-wide
cargo check --workspace
cargo clippy --workspace
```

---

## Common Bugs & Gotchas

### 1. Biquad Not Initialized After truce Migration
**Symptom:** Silent audio except bypass.
**Cause:** `Biquad::new()` = all-zero coefficients. After truce version bumps, `reset()` might not call `set_*()` on all filters. Filters process with b0=0 → silence.
**Fix:** Always call `biquad.set_butterworth_hp()` / `set_low_shelf()` / etc. in `reset()`, not just in `update_coeffs()`.
```rust
fn reset(state: &mut DspState, params: &Params, config: &AudioConfig) {
    for b in state.filters.iter_mut() {
        b.reset();  // clears state, does NOT set coefficients
        b.set_peaking_eq(1000.0, 0.0, 0.7, sr);  // MUST set initial coeffs
    }
    state.update_coeffs(params);  // now apply actual param values
}
```

### 2. SHM Singleton — One Per Process
**Symptom:** Second plugin instance overwrites first instance's data.
**Cause:** Using bare `static OnceLock<Vec<...>>` instead of keyed registry.
**Fix:** Key by `Arc::as_ptr(&params)` or other instance-unique id:
```rust
type Registry = Arc<Mutex<HashMap<usize, Data>>>;
// NOT: static FOO: OnceLock<Vec<Data>> = OnceLock::new();  // BUG!
```

### 3. FFT Off-by-One in Display
**Symptom:** DC offset shows as low-frequency peak.
**Cause:** `(k + 1)` instead of `k` used as bin index when drawing spectrum.
**Fix:** Verify bin indexing matches between compute and draw.

### 4. Parameter Smoothing Type
- `smooth = "linear(N)"` — N-sample moving average. Good for gain.
- No smooth — immediate jump. Good for mode switches, type selects.
- Wrong: smoothing a discrete param → host shows intermediate values.

### 5. Relay Display Name Race
**Symptom:** Relay shows "connected" but no spectra.
**Cause:** Display name formatted differently when writing consumer name vs. reading active.
**Fix:** Cache display name once from `(raw_name, claimed_slot)`, reuse same string everywhere.

### 6. Default Must Be Valid
```rust
range = "log(2.0, 2000.0)"   // default MUST be ≥2.0 and ≤2000.0
range = "linear(-12.0, 12.0)" // default MUST be in [-12, 12]
range = "discrete(0, 2)"      // default MUST be 0, 1, or 2
```
Wrong default → host may reject plugin or clamp silently.

---

## Repo-Specific Rules (from GOVERNANCE)

| Action | Rule |
|--------|------|
| DSP algorithm change | **ALWAYS ask user** |
| Parameter add/remove | **ALWAYS ask user** |
| `lx-ui` / `lx-dsp` / `lx-analysis` / `lx-shm` change | **ALWAYS ask user** |
| Vault structure change | **ALWAYS ask user** |
| Refactor, rename, dep update | **DO NOT** |
| Guess, infer, redesign | **DO NOT** |
| New dependency | Avoid unless essential |

---

## Quick Checklist — Adding a New Plugin

1. Copy `Cargo.toml` from sibling plugin, update name/version/description
2. Create `src/lib.rs` with template above
3. Derive `Params` — at minimum: `FloatParam` gain + `BoolParam` bypass + `#[skip] shared: Arc<SharedState>`
4. `DspState` must derive `Default`
5. Implement `PluginLogic` — minimum: `bus_layouts()`, `init()`, `reset()`, `process()`, `load_state()`, `state_changed()`, `editor()`
6. `truce::plugin! { logic: MyPlugin, params: MyPluginParams }` at bottom
7. Add to workspace `Cargo.toml` members
8. `cargo truce install --clap -p <name>` and `clap-validator` to verify
