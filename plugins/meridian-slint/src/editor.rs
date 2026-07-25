use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use truce_core::cast::{discrete_index, discrete_norm};
use truce_core::editor::{Editor, PluginContextReadF32};
use truce_params::FloatParam;
use lx_slint_editor::{LxSlintEditor, PluginContext};

use crate::MeridianParams;
use crate::MeridianParamsParamId as P;
use lx_dsp::{Biquad, TiltEq};

slint::include_modules!();

// Frozen vault size (ui-layout-spec): 990 × 660
const WINDOW_W: u32 = 990;
const WINDOW_H: u32 = 660;

/// Match vizia Meridian `TICK_INTERVAL` — telemetry / host→UI poll ~30 Hz.
const TICK_INTERVAL: Duration = Duration::from_millis(33);

/// Cache key for EQ curve (vizia `EqCurveKey` parity).
#[derive(Clone, Copy, PartialEq)]
struct EqCurveKey {
    hpf_freq: f32,
    lpf_freq: f32,
    cut_slope: i64,
    bass_gain: f32,
    bass_slope: i64,
    lo_mid_gain: f32,
    lo_mid_slope: i64,
    mid_gain: f32,
    mid_slope: i64,
    high_gain: f32,
    high_slope: i64,
    excite_gain: f32,
    excite_slope: i64,
    eq_freq_1: f32,
    eq_freq_2: f32,
    eq_freq_3: f32,
    eq_freq_4: f32,
    eq_freq_5: f32,
    tilt_gain: f32,
    sample_rate: f32,
}

fn eq_curve_key(params: &MeridianParams, sr: f32) -> EqCurveKey {
    EqCurveKey {
        hpf_freq: params.hpf_freq.raw_target() as f32,
        lpf_freq: params.lpf_freq.raw_target() as f32,
        cut_slope: params.cut_slope.value(),
        bass_gain: params.bass_gain.raw_target() as f32,
        bass_slope: params.bass_slope.value(),
        lo_mid_gain: params.lo_mid_gain.raw_target() as f32,
        lo_mid_slope: params.lo_mid_slope.value(),
        mid_gain: params.mid_gain.raw_target() as f32,
        mid_slope: params.mid_slope.value(),
        high_gain: params.high_gain.raw_target() as f32,
        high_slope: params.high_slope.value(),
        excite_gain: params.excite_gain.raw_target() as f32,
        excite_slope: params.excite_slope.value(),
        eq_freq_1: params.eq_freq_1.raw_target() as f32,
        eq_freq_2: params.eq_freq_2.raw_target() as f32,
        eq_freq_3: params.eq_freq_3.raw_target() as f32,
        eq_freq_4: params.eq_freq_4.raw_target() as f32,
        eq_freq_5: params.eq_freq_5.raw_target() as f32,
        tilt_gain: params.tilt_gain.raw_target() as f32,
        sample_rate: sr,
    }
}

/// Per-editor sync bookkeeping (vizia Ticker + TickAccum + dirty sets).
struct SyncCache {
    last_tick: Instant,
    /// First open must fill UI even if Instant says "not due".
    primed: bool,
    eq_key: Option<EqCurveKey>,
    eq_cmds: String,
    gr_history: Vec<f32>,
    gr_peak_hold: f32,
    gr_peak_hold_ticks: u32,
    was_measuring: bool,
    // Dirty mirrors — only call Slint setters when value changes.
    floats: [f32; 30],
    ints: [f32; 6],
    /// `None` = never pushed (first set always applies).
    bools: [Option<bool>; 5],
    meter_l: f32,
    meter_r: f32,
    peak_hold_l: f32,
    peak_hold_r: f32,
    gr: f32,
    gr_db_q: f32,
    corr: f32,
    balance: f32,
    hold_l_db_q: f32,
    hold_r_db_q: f32,
    auto_loud: Option<bool>,
}

impl SyncCache {
    fn new() -> Self {
        Self {
            last_tick: Instant::now()
                .checked_sub(TICK_INTERVAL)
                .unwrap_or_else(Instant::now),
            primed: false,
            eq_key: None,
            eq_cmds: String::new(),
            gr_history: vec![0.0; 90],
            gr_peak_hold: 0.0,
            gr_peak_hold_ticks: 0,
            was_measuring: false,
            floats: [f32::NAN; 30],
            ints: [f32::NAN; 6],
            bools: [None; 5],
            meter_l: f32::NAN,
            meter_r: f32::NAN,
            peak_hold_l: f32::NAN,
            peak_hold_r: f32::NAN,
            gr: f32::NAN,
            gr_db_q: f32::NAN,
            corr: f32::NAN,
            balance: f32::NAN,
            hold_l_db_q: f32::NAN,
            hold_r_db_q: f32::NAN,
            auto_loud: None,
        }
    }

    fn due(&mut self) -> bool {
        let now = Instant::now();
        if !self.primed || now.duration_since(self.last_tick) >= TICK_INTERVAL {
            self.last_tick = now;
            self.primed = true;
            true
        } else {
            false
        }
    }
}

#[inline]
fn changed_f32(prev: &mut f32, v: f32) -> bool {
    if *prev != v {
        *prev = v;
        true
    } else {
        false
    }
}

#[inline]
fn changed_bool(prev: &mut Option<bool>, v: bool) -> bool {
    if *prev != Some(v) {
        *prev = Some(v);
        true
    } else {
        false
    }
}

/// Normalized default for a FloatParam (log/linear/skew — matches host).
fn float_default_norm(p: &FloatParam) -> f32 {
    p.info.range.normalize(p.info.default_plain) as f32
}

// --- binding macros ---------------------------------------------------------

macro_rules! bind_floats {
    ($ui:expr, $state:expr, $($p:expr => $name:ident),* $(,)?) => {
        $(
            lx_slint_editor::paste! {
                let s = $state.clone();
                $ui.[<on_ $name _changed>](move |v| s.automate($p, v as f64));
            }
        )*
    };
}

/// Right-click knob/slider reset targets (`*_default` properties).
macro_rules! set_float_defaults {
    ($ui:expr, $params:expr, $($name:ident),* $(,)?) => {
        $(
            lx_slint_editor::paste! {
                $ui.[<set_ $name _default>](float_default_norm(&$params.$name));
            }
        )*
    };
}

/// Full RESET + per-control right-click: automate each float to its param default.
macro_rules! reset_floats {
    ($state:expr, $params:expr, $($p:expr => $name:ident),* $(,)?) => {
        $(
            $state.automate($p, float_default_norm(&$params.$name) as f64);
        )*
    };
}

macro_rules! bind_ints {
    ($ui:expr, $state:expr, $count:expr, $($p:expr => $name:ident),* $(,)?) => {
        $(
            lx_slint_editor::paste! {
                let s = $state.clone();
                let count = $count as usize;
                $ui.[<on_ $name _changed>](move |v: f32| {
                    s.automate($p, discrete_norm(v.max(0.0) as usize, count));
                });
            }
        )*
    };
}

macro_rules! bind_bools {
    ($ui:expr, $state:expr, $($p:expr => $name:ident),* $(,)?) => {
        $(
            lx_slint_editor::paste! {
                let s = $state.clone();
                $ui.[<on_ $name _changed>](move |v: bool| {
                    s.automate($p, if v { 1.0 } else { 0.0 });
                });
            }
        )*
    };
}

/// Dirty host→UI float push. `$idx` indexes `SyncCache::floats`.
macro_rules! sync_floats_dirty {
    ($ui:expr, $state:expr, $cache:expr, $($idx:expr, $p:expr => $name:ident),* $(,)?) => {
        $(
            lx_slint_editor::paste! {
                let v = PluginContextReadF32::get_param($state, $p);
                if changed_f32(&mut $cache.floats[$idx], v) {
                    $ui.[<set_ $name>](v);
                    $ui.[<set_ $name _text>](slint::SharedString::from($state.format_param($p)));
                }
            }
        )*
    };
}

macro_rules! sync_ints_dirty {
    ($ui:expr, $state:expr, $cache:expr, $count:expr, $($idx:expr, $p:expr => $name:ident),* $(,)?) => {
        $(
            lx_slint_editor::paste! {
                let idx = discrete_index(PluginContextReadF32::get_param($state, $p) as f64, $count) as f32;
                if changed_f32(&mut $cache.ints[$idx], idx) {
                    $ui.[<set_ $name>](idx);
                }
            }
        )*
    };
}

macro_rules! sync_bools_dirty {
    ($ui:expr, $state:expr, $cache:expr, $($idx:expr, $p:expr => $name:ident),* $(,)?) => {
        $(
            lx_slint_editor::paste! {
                let v = PluginContextReadF32::get_param($state, $p) > 0.5;
                if changed_bool(&mut $cache.bools[$idx], v) {
                    $ui.[<set_ $name>](v);
                }
            }
        )*
    };
}

// Vault UI helpers (build-time callbacks only)
struct VaultUiState {
    vault_path: Option<String>,
    names: Vec<String>,
    cache: Vec<(String, std::path::PathBuf, ())>,
    pending: Arc<PendingPresets>,
    scanning_for: Option<String>,
}
struct PendingPresets {
    ready: std::sync::atomic::AtomicBool,
    generation: std::sync::atomic::AtomicU32,
    presets: Mutex<Option<(u32, Vec<(String, std::path::PathBuf, ())>)>>,
}
impl PendingPresets {
    fn new() -> Self {
        Self {
            ready: std::sync::atomic::AtomicBool::new(false),
            generation: std::sync::atomic::AtomicU32::new(0),
            presets: Mutex::new(None),
        }
    }
    fn bump_generation(&self) -> u32 {
        let new = self
            .generation
            .load(std::sync::atomic::Ordering::Relaxed)
            .wrapping_add(1);
        self.generation
            .store(new, std::sync::atomic::Ordering::Release);
        self.ready
            .store(false, std::sync::atomic::Ordering::Release);
        if let Ok(mut guard) = self.presets.lock() {
            *guard = None;
        }
        new
    }
}
fn spawn_vault_scan(_vp: String, pending: Arc<PendingPresets>, generation: u32) {
    std::thread::spawn(move || {
        // Minimal scan: just mark ready
        if let Ok(mut guard) = pending.presets.lock() {
            *guard = Some((generation, Vec::new()));
        }
        pending
            .ready
            .store(true, std::sync::atomic::Ordering::Release);
    });
}

pub fn build_editor(params: Arc<MeridianParams>) -> Box<dyn Editor> {
    let shared = params.shared.clone();

    // Vizia-parity tick state: 33 ms throttle, dirty sets, EQ cache.
    // Mutex: LxSlintEditor SyncFn is Send+Sync.
    let sync_cache = Mutex::new(SyncCache::new());

    LxSlintEditor::new(
        params.clone(),
        (WINDOW_W, WINDOW_H),
        {
            let params = params.clone();
            let shared = shared.clone();
            move |state: PluginContext<MeridianParams>| {
                let ui = MeridianUi::new().unwrap();

                // Right-click reset targets: real defaults via range.normalize
                // (Slint `*_default` props were stubbed at 0.5 → mid-position jumps).
                set_float_defaults!(
                    ui,
                    params,
                    hpf_freq,
                    lpf_freq,
                    bass_gain,
                    lo_mid_gain,
                    mid_gain,
                    high_gain,
                    excite_gain,
                    eq_freq_1,
                    eq_freq_2,
                    eq_freq_3,
                    eq_freq_4,
                    eq_freq_5,
                    tilt_gain,
                    warmth_drive,
                    warmth_mix,
                    excite_amount,
                    excite_blend,
                    excite_freq,
                    comp_threshold,
                    comp_mix,
                    comp_attack,
                    comp_release,
                    comp_character,
                    comp_makeup,
                    inflate_effect,
                    inflate_curve,
                    stereo_width,
                    pan,
                    output_gain,
                );

                // --- UI → host callbacks ---
                bind_floats!(ui, state,
                    P::HpfFreq => hpf_freq,
                    P::LpfFreq => lpf_freq,
                    P::BassGain => bass_gain,
                    P::LoMidGain => lo_mid_gain,
                    P::MidGain => mid_gain,
                    P::HighGain => high_gain,
                    P::ExciteGain => excite_gain,
                    P::EqFreq1 => eq_freq_1,
                    P::EqFreq2 => eq_freq_2,
                    P::EqFreq3 => eq_freq_3,
                    P::EqFreq4 => eq_freq_4,
                    P::EqFreq5 => eq_freq_5,
                    P::TiltGain => tilt_gain,
                    P::WarmthDrive => warmth_drive,
                    P::WarmthMix => warmth_mix,
                    P::ExciteAmount => excite_amount,
                    P::ExciteBlend => excite_blend,
                    P::ExciteFreq => excite_freq,
                    P::CompThreshold => comp_threshold,
                    P::CompMix => comp_mix,
                    P::CompAttack => comp_attack,
                    P::CompRelease => comp_release,
                    P::CompCharacter => comp_character,
                    P::CompMakeup => comp_makeup,
                    P::InflateEffect => inflate_effect,
                    P::InflateCurve => inflate_curve,
                    P::StereoWidth => stereo_width,
                    P::Pan => pan,
                    P::OutputGain => output_gain,
                );

                // CutSlope = discrete(0,1) → 2 choices (12/24 dB).
                // EQ band slopes = discrete(0,2) → 3 choices (A/B/C).
                bind_ints!(ui, state, 2, P::CutSlope => cut_slope);
                bind_ints!(ui, state, 3,
                    P::BassSlope => bass_slope,
                    P::LoMidSlope => lo_mid_slope,
                    P::MidSlope => mid_slope,
                    P::HighSlope => high_slope,
                    P::ExciteSlope => excite_slope,
                );

                bind_bools!(ui, state,
                    P::MonoActive => mono_active,
                    P::DeltaActive => delta_active,
                    P::BypassActive => bypass_active,
                    P::InflateBandSplit => inflate_band_split,
                    P::InflateClip => inflate_clip,
                );

                // --- vault / preset / reset callbacks (UI actions, not parameters) ---
                let snap_shared = shared.clone();
                ui.on_snap_clicked(move || {
                    snap_shared
                        .snap_active
                        .store(true, std::sync::atomic::Ordering::Release);
                    tracing::info!("SNAP triggered");
                });

                let save_params = params.clone();
                ui.on_save_clicked(move || {
                    // Minimal preset save: store current parameter values in a plain text file
                    // under the plugin's local presets directory.
                    let dir = lx_analysis::get_plugin_dir("Meridian").join("presets");
                    let _ = std::fs::create_dir_all(&dir);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let name = format!("meridian_preset_{now}");
                    let fp = dir.join(format!("{name}.txt"));
                    let mut lines = Vec::new();
                    macro_rules! store_float {
                        ($p:ident) => {
                            lines.push(format!("{}={}", stringify!($p), save_params.$p.raw_target()));
                        };
                    }
                    store_float!(hpf_freq);
                    store_float!(lpf_freq);
                    store_float!(bass_gain);
                    store_float!(lo_mid_gain);
                    store_float!(mid_gain);
                    store_float!(high_gain);
                    store_float!(excite_gain);
                    store_float!(eq_freq_1);
                    store_float!(eq_freq_2);
                    store_float!(eq_freq_3);
                    store_float!(eq_freq_4);
                    store_float!(eq_freq_5);
                    store_float!(tilt_gain);
                    store_float!(warmth_drive);
                    store_float!(warmth_mix);
                    store_float!(excite_amount);
                    store_float!(excite_blend);
                    store_float!(excite_freq);
                    store_float!(comp_threshold);
                    store_float!(comp_mix);
                    store_float!(comp_attack);
                    store_float!(comp_release);
                    store_float!(comp_character);
                    store_float!(comp_makeup);
                    store_float!(inflate_effect);
                    store_float!(inflate_curve);
                    store_float!(stereo_width);
                    store_float!(pan);
                    store_float!(output_gain);
                    macro_rules! store_int {
                        ($p:ident) => {
                            lines.push(format!("{}={}", stringify!($p), save_params.$p.value()));
                        };
                    }
                    store_int!(cut_slope);
                    store_int!(bass_slope);
                    store_int!(lo_mid_slope);
                    store_int!(mid_slope);
                    store_int!(high_slope);
                    store_int!(excite_slope);
                    macro_rules! store_bool {
                        ($p:ident) => {
                            lines.push(format!("{}={}", stringify!($p), save_params.$p.value()));
                        };
                    }
                    store_bool!(mono_active);
                    store_bool!(delta_active);
                    store_bool!(bypass_active);
                    store_bool!(inflate_band_split);
                    store_bool!(inflate_clip);
                    let content = lines.join("\n");
                    if std::fs::write(&fp, content).is_ok() {
                        tracing::info!("SAVE preset to {}", fp.display());
                    }
                });

                let vault_state = Arc::new(Mutex::new(VaultUiState {
                    vault_path: None,
                    names: Vec::new(),
                    cache: Vec::new(),
                    pending: Arc::new(PendingPresets::new()),
                    scanning_for: None,
                }));
                let vault_state_clone = vault_state.clone();
                ui.on_vault_path_changed(move |path: slint::SharedString| {
                    let path = path.to_string().trim().to_string();
                    let new_vp = if path.is_empty() { None } else { Some(path) };
                    if let Ok(mut vs) = vault_state_clone.lock() {
                        vs.vault_path = new_vp.clone();
                        let mut cfg = lx_analysis::load_config("Meridian");
                        cfg.vault_path = new_vp.clone();
                        let _ = lx_analysis::save_config("Meridian", &cfg);
                        let scan_gen = vs.pending.bump_generation();
                        if let Some(ref _vp) = new_vp {
                            vs.scanning_for = Some(_vp.clone());
                            spawn_vault_scan(_vp.clone(), vs.pending.clone(), scan_gen);
                        } else {
                            vs.names = Vec::new();
                            vs.cache.clear();
                            vs.scanning_for = None;
                        }
                    }
                });

                let reset_state = state.clone();
                let reset_params = params.clone();
                ui.on_reset_clicked(move || {
                    reset_floats!(
                        reset_state,
                        reset_params,
                        P::HpfFreq => hpf_freq,
                        P::LpfFreq => lpf_freq,
                        P::BassGain => bass_gain,
                        P::LoMidGain => lo_mid_gain,
                        P::MidGain => mid_gain,
                        P::HighGain => high_gain,
                        P::ExciteGain => excite_gain,
                        P::EqFreq1 => eq_freq_1,
                        P::EqFreq2 => eq_freq_2,
                        P::EqFreq3 => eq_freq_3,
                        P::EqFreq4 => eq_freq_4,
                        P::EqFreq5 => eq_freq_5,
                        P::TiltGain => tilt_gain,
                        P::WarmthDrive => warmth_drive,
                        P::WarmthMix => warmth_mix,
                        P::ExciteAmount => excite_amount,
                        P::ExciteBlend => excite_blend,
                        P::ExciteFreq => excite_freq,
                        P::CompThreshold => comp_threshold,
                        P::CompMix => comp_mix,
                        P::CompAttack => comp_attack,
                        P::CompRelease => comp_release,
                        P::CompCharacter => comp_character,
                        P::CompMakeup => comp_makeup,
                        P::InflateEffect => inflate_effect,
                        P::InflateCurve => inflate_curve,
                        P::StereoWidth => stereo_width,
                        P::Pan => pan,
                        P::OutputGain => output_gain,
                    );
                    reset_state.automate(P::CutSlope, discrete_norm(0, 2));
                    reset_state.automate(P::BassSlope, discrete_norm(1, 3));
                    reset_state.automate(P::LoMidSlope, discrete_norm(1, 3));
                    reset_state.automate(P::MidSlope, discrete_norm(1, 3));
                    reset_state.automate(P::HighSlope, discrete_norm(1, 3));
                    reset_state.automate(P::ExciteSlope, discrete_norm(1, 3));
                    reset_state.automate(P::InflateBandSplit, 0.0);
                    reset_state.automate(P::InflateClip, 0.0);
                    reset_state.automate(P::MonoActive, 0.0);
                    reset_state.automate(P::DeltaActive, 0.0);
                    reset_state.automate(P::BypassActive, 0.0);
                    tracing::info!("RESET clicked");
                });

                // AUTO LOUD — arm DSP meters (process applies trigger → measure ~5s)
                let shared_loud = shared.clone();
                ui.on_auto_loud_clicked(move || {
                    if shared_loud.auto_loud_measuring.load(Ordering::Acquire) {
                        return; // already measuring
                    }
                    shared_loud
                        .auto_loud_trigger
                        .store(true, Ordering::Release);
                });

                ui
            }
        },
        {
            let shared_for_sync = shared.clone();
            let params_for_curve = params.clone();
            move |ui: &MeridianUi, state: &PluginContext<MeridianParams>| {
                let Ok(mut cache) = sync_cache.lock() else {
                    return;
                };
                // Vizia Ticker: heavy host→UI work at ~30 Hz only.
                if !cache.due() {
                    return;
                }

                // --- host → UI normalized values (dirty) ---
                sync_floats_dirty!(ui, state, cache,
                    0, P::HpfFreq => hpf_freq,
                    1, P::LpfFreq => lpf_freq,
                    2, P::BassGain => bass_gain,
                    3, P::LoMidGain => lo_mid_gain,
                    4, P::MidGain => mid_gain,
                    5, P::HighGain => high_gain,
                    6, P::ExciteGain => excite_gain,
                    7, P::EqFreq1 => eq_freq_1,
                    8, P::EqFreq2 => eq_freq_2,
                    9, P::EqFreq3 => eq_freq_3,
                    10, P::EqFreq4 => eq_freq_4,
                    11, P::EqFreq5 => eq_freq_5,
                    12, P::TiltGain => tilt_gain,
                    13, P::WarmthDrive => warmth_drive,
                    14, P::WarmthMix => warmth_mix,
                    15, P::ExciteAmount => excite_amount,
                    16, P::ExciteBlend => excite_blend,
                    17, P::ExciteFreq => excite_freq,
                    18, P::CompThreshold => comp_threshold,
                    19, P::CompMix => comp_mix,
                    20, P::CompAttack => comp_attack,
                    21, P::CompRelease => comp_release,
                    22, P::CompCharacter => comp_character,
                    23, P::CompMakeup => comp_makeup,
                    24, P::InflateEffect => inflate_effect,
                    25, P::InflateCurve => inflate_curve,
                    26, P::StereoWidth => stereo_width,
                    27, P::Pan => pan,
                    28, P::OutputGain => output_gain,
                );

                sync_ints_dirty!(ui, state, cache, 2, 0, P::CutSlope => cut_slope);
                sync_ints_dirty!(ui, state, cache, 3,
                    1, P::BassSlope => bass_slope,
                    2, P::LoMidSlope => lo_mid_slope,
                    3, P::MidSlope => mid_slope,
                    4, P::HighSlope => high_slope,
                    5, P::ExciteSlope => excite_slope,
                );

                sync_bools_dirty!(ui, state, cache,
                    0, P::MonoActive => mono_active,
                    1, P::DeltaActive => delta_active,
                    2, P::BypassActive => bypass_active,
                    3, P::InflateBandSplit => inflate_band_split,
                    4, P::InflateClip => inflate_clip,
                );

                let shared = &shared_for_sync;
                let sr = shared.sample_rate.load(Ordering::Relaxed).max(1.0);

                // --- EQ curve (cached like vizia EqCurveKey) ---
                let key = eq_curve_key(&params_for_curve, sr);
                if cache.eq_key != Some(key) {
                    cache.eq_key = Some(key);
                    cache.eq_cmds = eq_curve_path(&params_for_curve, sr);
                    ui.set_curve_cmds(slint::SharedString::from(cache.eq_cmds.as_str()));
                }

                // --- meters ---
                let peak_l_db = shared.output_peak_l.load(Ordering::Relaxed);
                let peak_r_db = shared.output_peak_r.load(Ordering::Relaxed);
                let hold_l_db = shared.peak_hold_l.load(Ordering::Relaxed);
                let hold_r_db = shared.peak_hold_r.load(Ordering::Relaxed);
                let gr_db = shared.gain_reduction.load(Ordering::Relaxed).max(0.0);
                let corr = shared.phase_correlation.load(Ordering::Relaxed);
                let balance = shared.balance.load(Ordering::Relaxed);

                let ml = db_to_meter(peak_l_db);
                let mr = db_to_meter(peak_r_db);
                let phl = db_to_meter(hold_l_db);
                let phr = db_to_meter(hold_r_db);
                let grn = db_to_gr(gr_db);
                if changed_f32(&mut cache.meter_l, ml) {
                    ui.set_meter_l(ml);
                }
                if changed_f32(&mut cache.meter_r, mr) {
                    ui.set_meter_r(mr);
                }
                if changed_f32(&mut cache.peak_hold_l, phl) {
                    ui.set_peak_hold_l(phl);
                }
                if changed_f32(&mut cache.peak_hold_r, phr) {
                    ui.set_peak_hold_r(phr);
                }
                if changed_f32(&mut cache.gr, grn) {
                    ui.set_gr(grn);
                }
                // Quantize text updates to 0.1 dB / 0.01 corr so we skip most string allocs.
                let gr_q = (gr_db * 10.0).round() / 10.0;
                if changed_f32(&mut cache.gr_db_q, gr_q) {
                    ui.set_gr_text(slint::SharedString::from(format!("GR: {gr_q:.1}")));
                }
                if changed_f32(&mut cache.corr, corr) {
                    ui.set_correlation(corr);
                    ui.set_corr_text(slint::SharedString::from(format!("corr: {corr:.2}")));
                }
                if changed_f32(&mut cache.balance, balance) {
                    ui.set_balance(balance);
                    ui.set_balance_text(slint::SharedString::from(format!("bal: {balance:.2}")));
                }
                let hold_l_q = (hold_l_db * 10.0).round() / 10.0;
                let hold_r_q = (hold_r_db * 10.0).round() / 10.0;
                if changed_f32(&mut cache.hold_l_db_q, hold_l_q) {
                    ui.set_peak_l_text(slint::SharedString::from(fmt_db(hold_l_db)));
                }
                if changed_f32(&mut cache.hold_r_db_q, hold_r_q) {
                    ui.set_peak_r_text(slint::SharedString::from(fmt_db(hold_r_db)));
                }

                // GR envelope + peak-hold (footer mini-display) — only on tick
                cache.gr_history.push(gr_db);
                if cache.gr_history.len() > 90 {
                    cache.gr_history.remove(0);
                }
                if gr_db > cache.gr_peak_hold {
                    cache.gr_peak_hold = gr_db;
                    cache.gr_peak_hold_ticks = 90;
                } else if cache.gr_peak_hold_ticks > 0 {
                    cache.gr_peak_hold_ticks -= 1;
                } else {
                    cache.gr_peak_hold = (cache.gr_peak_hold - 0.15).max(gr_db).max(0.0);
                }
                ui.set_comp_peak_text(slint::SharedString::from(format!(
                    "PK: {:.1}",
                    cache.gr_peak_hold
                )));
                ui.set_gr_envelope_cmds(slint::SharedString::from(gr_envelope_path(
                    &cache.gr_history,
                    gr_db,
                    110.0,
                    48.0,
                )));

                // --- Auto Loud status + apply LUFS offset when measure ends ---
                let measuring = shared.auto_loud_measuring.load(Ordering::Acquire);
                if changed_bool(&mut cache.auto_loud, measuring) {
                    ui.set_auto_loud_measuring(measuring);
                }
                if cache.was_measuring && !measuring {
                    let offset = shared.auto_loud_gain_offset.load(Ordering::Acquire);
                    shared.auto_loud_gain_offset.store(0.0, Ordering::Release);
                    if offset.abs() > 0.01 {
                        let cur_db = state.params().output_gain.raw_target() as f32;
                        let new_db = (cur_db + offset).clamp(-12.0, 12.0);
                        let norm = ((new_db + 12.0) / 24.0) as f64;
                        state.automate(P::OutputGain, norm.clamp(0.0, 1.0));
                    }
                }
                cache.was_measuring = measuring;

                // --- spectrum & goniometer at tick rate only (~30 Hz, vizia parity) ---
                ui.set_spectrum_path(slint::SharedString::from(spectrum_path(
                    shared, 620.0, 140.0,
                )));
                ui.set_gonio_path(slint::SharedString::from(gonio_path(
                    shared, 139.0, 139.0,
                )));
            }
        },
    )
    .resizable(true)
    .into()
}

fn fmt_db(v: f32) -> String {
    if v <= -60.0 {
        "-inf".to_string()
    } else {
        format!("{v:.1}")
    }
}

// --- meter helpers --------------------------------------------------------

/// Map peak dB to 0..1 over the frozen stereo-meter range (−60..+6 dB).
fn db_to_meter(db: f32) -> f32 {
    ((db + 60.0) / 66.0).clamp(0.0, 1.0)
}

fn db_to_gr(db: f32) -> f32 {
    (1.0 - (db / 30.0)).clamp(0.0, 1.0)
}

// --- spectrum path --------------------------------------------------------
// Y range matches vizia SpectrumConfig default: −70 … −18 dB.
// X is log-frequency (20 Hz … 20 kHz), same as SpectrumView.

fn spectrum_path(shared: &lx_analysis::SharedState, w: f32, h: f32) -> String {
    use lx_analysis::SPECTRUM_BINS;
    const MIN_DB: f32 = -70.0;
    const MAX_DB: f32 = -18.0;

    let bins = shared
        .spectrum_avg
        .try_lock()
        .map(|b| b.clone())
        .or_else(|_| shared.spectrum_bins.try_lock().map(|b| b.clone()))
        .unwrap_or_default();
    let n = bins.len().min(SPECTRUM_BINS);
    if n < 2 {
        return String::new();
    }

    let sr = shared.sample_rate.load(Ordering::Relaxed).max(1.0);
    let fft_size = (n * 2) as f32;
    let log_f = |f: f32| -> f32 {
        ((f.max(20.0).ln() - 20.0f32.ln()) / (20000.0f32.ln() - 20.0f32.ln())).clamp(0.0, 1.0)
    };
    let db_to_y = |db: f32| -> f32 {
        let norm = ((db - MIN_DB) / (MAX_DB - MIN_DB)).clamp(0.0, 1.0);
        h - norm * h
    };

    // Collect log-x samples (skip sub-20 Hz bins).
    let mut pts: Vec<(f32, f32)> = Vec::with_capacity(n);
    for i in 0..n {
        let freq = i as f32 * sr / fft_size;
        if freq < 20.0 {
            continue;
        }
        if freq > 20000.0 {
            break;
        }
        let x = log_f(freq) * w;
        let y = db_to_y(bins[i].clamp(MIN_DB, MAX_DB));
        pts.push((x, y));
    }
    if pts.len() < 2 {
        return String::new();
    }

    let mut s = String::with_capacity(pts.len() * 22);
    // Filled area under curve (floor at bottom of −70 dB range)
    s.push_str(&format!("M {:.1} {:.1}", pts[0].0, h));
    for &(x, y) in &pts {
        s.push_str(&format!(" L {x:.1} {y:.1}"));
    }
    s.push_str(&format!(
        " L {:.1} {:.1} Z",
        pts.last().map(|p| p.0).unwrap_or(w),
        h
    ));
    s
}

/// GR envelope path for footer mini-display (viewBox w×h, max GR = 12 dB).
fn gr_envelope_path(history: &[f32], current: f32, w: f32, h: f32) -> String {
    const MAX_GR: f32 = 12.0;
    const MARGIN: f32 = 2.0;
    let n = history.len() + 1;
    if n < 2 {
        return String::new();
    }
    let x_step = (w - MARGIN * 2.0) / (n - 1) as f32;
    let val_to_y = |val: f32| -> f32 {
        h - MARGIN - (val / MAX_GR).clamp(0.0, 1.0) * (h - MARGIN * 2.0)
    };

    let mut s = String::with_capacity(n * 20);
    // Fill under curve (top-left → points → bottom-right → close)
    s.push_str(&format!("M {:.1} {:.1}", MARGIN, MARGIN));
    for (i, &val) in history.iter().enumerate() {
        let x = MARGIN + i as f32 * x_step;
        s.push_str(&format!(" L {x:.1} {:.1}", val_to_y(val)));
    }
    let last_x = MARGIN + history.len() as f32 * x_step;
    s.push_str(&format!(" L {last_x:.1} {:.1}", val_to_y(current)));
    s.push_str(&format!(" L {last_x:.1} {:.1}", h - MARGIN));
    s.push_str(&format!(" L {:.1} {:.1} Z", MARGIN, h - MARGIN));
    s
}

// --- goniometer path (M/S rotation — vault frozen spec) -------------------

fn gonio_path(shared: &lx_analysis::SharedState, w: f32, h: f32) -> String {
    use lx_analysis::SCOPE_BUFFER_LEN;
    let (samples, write_pos) = {
        let pos = shared.scope_write_pos.load(Ordering::Relaxed);
        let samples = match shared.scope_samples.try_lock() {
            Ok(v) => v.clone(),
            Err(_) => return String::new(),
        };
        (samples, pos)
    };
    if samples.is_empty() {
        return String::new();
    }

    let points_to_take = 512usize.min(SCOPE_BUFFER_LEN);
    let mut s = String::with_capacity(points_to_take * 24);
    let cx = w * 0.5;
    let cy = h * 0.5;
    let scale = cx.min(cy) * 0.9;
    let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
    let n = samples.len();

    for i in 0..points_to_take {
        let age = points_to_take - 1 - i;
        let idx = (write_pos + n - age - 1) % n;
        let [l, r] = samples[idx];
        // M vertical, S horizontal (industry standard 45° rotation)
        let m = (l + r) * inv_sqrt2;
        let side = (l - r) * inv_sqrt2;
        let x = cx - side * scale;
        let y = cy - m * scale;
        if i == 0 {
            s.push_str(&format!("M {x:.1} {y:.1}"));
        } else {
            s.push_str(&format!(" L {x:.1} {y:.1}"));
        }
    }
    s
}

// --- EQ curve path for the amber LxEqCurve ----------------------------------

fn eq_curve_path(params: &MeridianParams, sr: f32) -> String {
    // HPF (6 dB/oct or cascaded 12 dB/oct)
    let mut hpf1 = Biquad::new();
    let mut hpf2 = Biquad::new();
    let hpf_f = params.hpf_freq.raw_target() as f32;
    let cut_slope = params.cut_slope.value();
    const Q1: f32 = 0.541_196_1;
    const Q2: f32 = 1.306_563;
    if cut_slope >= 1 {
        hpf1.set_butterworth_hp_q(hpf_f, Q1, sr);
        hpf2.set_butterworth_hp_q(hpf_f, Q2, sr);
    } else {
        hpf1.set_butterworth_hp(hpf_f, sr);
        hpf2.set_identity();
    }

    // LPF (6 dB/oct or cascaded 12 dB/oct)
    let mut lpf1 = Biquad::new();
    let mut lpf2 = Biquad::new();
    let lpf_f = params.lpf_freq.raw_target() as f32;
    if cut_slope >= 1 {
        lpf1.set_butterworth_lp_q(lpf_f, Q1, sr);
        lpf2.set_butterworth_lp_q(lpf_f, Q2, sr);
    } else {
        lpf1.set_butterworth_lp(lpf_f, sr);
        lpf2.set_identity();
    }

    // Slope/Q mappings must match Meridian DSP reset().
    let slope_val = |slope_idx: i64| -> f32 {
        match slope_idx {
            0 => 0.5,
            1 => 1.0,
            _ => 2.0,
        }
    };
    let q_val = |slope_idx: i64| -> f32 {
        match slope_idx {
            0 => 0.4,
            1 => 0.7,
            _ => 1.5,
        }
    };

    // 5-band EQ
    let mut bass = Biquad::new();
    let mut lo_mid = Biquad::new();
    let mut mid = Biquad::new();
    let mut high = Biquad::new();
    let mut excite = Biquad::new();

    bass.set_low_shelf(
        params.eq_freq_1.raw_target() as f32,
        params.bass_gain.raw_target() as f32,
        slope_val(params.bass_slope.value()),
        sr,
    );
    lo_mid.set_peaking_eq(
        params.eq_freq_2.raw_target() as f32,
        params.lo_mid_gain.raw_target() as f32,
        q_val(params.lo_mid_slope.value()),
        sr,
    );
    mid.set_peaking_eq(
        params.eq_freq_3.raw_target() as f32,
        params.mid_gain.raw_target() as f32,
        q_val(params.mid_slope.value()),
        sr,
    );
    high.set_peaking_eq(
        params.eq_freq_4.raw_target() as f32,
        params.high_gain.raw_target() as f32,
        q_val(params.high_slope.value()),
        sr,
    );
    excite.set_high_shelf(
        params.eq_freq_5.raw_target() as f32,
        params.excite_gain.raw_target() as f32,
        slope_val(params.excite_slope.value()),
        sr,
    );

    // Tilt EQ
    let mut tilt = TiltEq::new();
    tilt.set(1000.0, params.tilt_gain.raw_target() as f32, sr);

    // Match vizia compute_eq_curve: ±24 dB overlay scale (not spectrum −70…−18).
    const N: usize = 256;
    const W: f32 = 620.0;
    const H: f32 = 140.0; // matches FFT card path-h
    const DB_MIN: f32 = -24.0;
    const DB_MAX: f32 = 24.0;
    let db_range = DB_MAX - DB_MIN;
    let db_to_y = |db: f32| -> f32 {
        let norm = ((db - DB_MIN) / db_range).clamp(0.0, 1.0);
        H - norm * H
    };

    let mut cmds = String::with_capacity(N * 16);
    for i in 0..N {
        let t = i as f32 / (N - 1) as f32;
        // Log frequency axis: 20 Hz … 20 kHz (same as vizia)
        let freq = 20.0f32 * 1000.0f32.powf(t);
        let mut db = 0.0f32;
        db += hpf1.magnitude_db(freq, sr);
        db += hpf2.magnitude_db(freq, sr);
        db += lpf1.magnitude_db(freq, sr);
        db += lpf2.magnitude_db(freq, sr);
        db += bass.magnitude_db(freq, sr);
        db += lo_mid.magnitude_db(freq, sr);
        db += mid.magnitude_db(freq, sr);
        db += high.magnitude_db(freq, sr);
        db += excite.magnitude_db(freq, sr);
        db += tilt.magnitude_db(freq, sr);
        let db = db.clamp(DB_MIN, DB_MAX);
        let x = t * W;
        let y = db_to_y(db);
        if i == 0 {
            cmds.push_str(&format!("M {x:.2} {y:.2}"));
        } else {
            cmds.push_str(&format!(" L {x:.2} {y:.2}"));
        }
    }
    cmds
}
