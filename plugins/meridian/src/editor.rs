use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aura::prelude::*;
use aura::FloatParam;
use slint::{ModelRc, SharedString, VecModel};
use aura_editor::platform::clipboard_get_retry;
use aura_editor::typed::*;
use aura_editor::ui_zoom::{apply_ui_zoom, UiZoom};
use aura_dsp::analysis::SPECTRUM_BINS;
use aura_dsp::analysis::product_shared::MeridianShared;
use aura_dsp::analysis::vault::{load_config, save_config};
use paste::paste;

use crate::MeridianParams;
use crate::MeridianParamsParamId as P;
use crate::presets::{
    apply_profile, export_meridian_markdown, find_profile, merge_preset_names, preset_save_dir,
    profile_from_params, snap_filename, snap_markdown, spawn_vault_scan, PendingPresets,
    PresetEntry,
};
use aura_dsp::fx::{Biquad, TiltEq};

slint::include_modules!();

fn names_model(names: &[String]) -> ModelRc<SharedString> {
    let v: Vec<SharedString> = names.iter().map(|s| SharedString::from(s.as_str())).collect();
    ModelRc::new(VecModel::from(v))
}

// Frozen vault size (ui-layout-spec / Lx.window-*): 990 × 670
const WINDOW_W: u32 = 990;
const WINDOW_H: u32 = 670;

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
    spectrum_fill_q: f32,
    /// Previous tick SNAP active — falling edge writes SNAPSHOT-*.md
    snap_was_active: bool,
    snap_blink: u32,
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
            spectrum_fill_q: f32::NAN,
            snap_was_active: false,
            snap_blink: 0,
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
            paste! {
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
            paste! {
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
            paste! {
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
            paste! {
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
            paste! {
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
            paste! {
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
            paste! {
                let v = PluginContextReadF32::get_param($state, $p) > 0.5;
                if changed_bool(&mut $cache.bools[$idx], v) {
                    $ui.[<set_ $name>](v);
                }
            }
        )*
    };
}

struct VaultUiState {
    vault_path: Option<String>,
    names: Vec<String>,
    cache: Vec<PresetEntry>,
    pending: Arc<PendingPresets>,
    scanning_for: Option<String>,
}

pub fn build_editor(params: Arc<MeridianParams>) -> Box<dyn Editor> {
    let shared = params.shared.clone();

    // Vizia-parity tick state: 33 ms throttle, dirty sets, EQ cache.
    // Mutex: LxSlintEditor SyncFn is Send+Sync.
    let sync_cache = Mutex::new(SyncCache::new());

    // Shared between build callbacks and per-frame sync (scan drain, SNAP write).
    let init_cfg = load_config("Meridian");
    let init_vp = init_cfg.vault_path.clone();
    let vault_pending = Arc::new(PendingPresets::new());
    {
        let scan_gen = vault_pending.bump_generation();
        spawn_vault_scan(
            init_vp.clone().unwrap_or_default(),
            vault_pending.clone(),
            scan_gen,
        );
    }
    let vault_state = Arc::new(Mutex::new(VaultUiState {
        vault_path: init_vp.clone(),
        names: Vec::new(),
        cache: Vec::new(),
        pending: vault_pending,
        scanning_for: init_vp.clone(),
    }));

    let ui_zoom = UiZoom::new(WINDOW_W, WINDOW_H);
    let zoom_build = ui_zoom.clone();
    LxSlintEditor::new_with_zoom(
        params.clone(),
        ui_zoom,
        {
            let params = params.clone();
            let shared = shared.clone();
            let vault_state = vault_state.clone();
            let init_vp = init_vp.clone();
            move |state: LxPluginContext<MeridianParams>| {
                let ui = MeridianUi::new().unwrap();

                ui.set_ui_zoom_percent(zoom_build.percent() as i32);
                {
                    let z = zoom_build.clone();
                    let s = state.clone();
                    ui.on_ui_zoom_changed(move |p| {
                        apply_ui_zoom(&z, |w, h| s.request_resize(w, h), p as u32);
                    });
                }

                // SMOOTH default ON (display-only; not a host param).
                shared
                    .spectrum.smooth
                    .store(true, Ordering::Release);
                ui.set_spectrum_smooth(true);

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

                // --- vault / preset / SNAP (Vizia parity) ---
                if let Some(ref vp) = init_vp {
                    ui.set_vault_path(SharedString::from(vp.as_str()));
                }
                if init_vp.as_ref().is_none_or(|v| v.is_empty()) {
                    ui.set_snap_label(SharedString::from("SET VAULT"));
                }

                let snap_shared = shared.clone();
                let snap_vs = vault_state.clone();
                let snap_ui = ui.as_weak();
                ui.on_snap_clicked(move || {
                    let no_vault = snap_vs
                        .lock()
                        .ok()
                        .and_then(|g| g.vault_path.clone())
                        .is_none_or(|v| v.is_empty());
                    if no_vault {
                        if let Some(ui) = snap_ui.upgrade() {
                            ui.set_vault_setup_open(true);
                            ui.set_snap_label(SharedString::from("SET VAULT"));
                        }
                        return;
                    }
                    snap_shared.snap.active.store(true, Ordering::Release);
                    snap_shared.snap.phase.store(1, Ordering::Release);
                    if let Some(ui) = snap_ui.upgrade() {
                        ui.set_snap_label(SharedString::from("ANALYZE..."));
                    }
                    tracing::info!("SNAP triggered");
                });

                // Display-only 1/3-oct spectrum smoothing toggle (Lucent parity).
                let smooth_shared = shared.clone();
                ui.on_spectrum_smooth_changed(move |on: bool| {
                    smooth_shared
                        .spectrum.smooth
                        .store(on, Ordering::Release);
                });

                // Peak-hold reset (click on readouts / double-click on meter).
                let reset_shared = shared.clone();
                ui.on_reset_peaks(move || {
                    reset_shared
                        .peaks.reset_peak
                        .store(true, Ordering::Release);
                });

                let save_params = params.clone();
                let save_vs = vault_state.clone();
                let save_ui = ui.as_weak();
                ui.on_save_clicked(move || {
                    let Some(ui) = save_ui.upgrade() else { return };
                    let name_input = ui.get_preset_name().to_string();
                    let name = {
                        let mut vs = match save_vs.lock() {
                            Ok(g) => g,
                            Err(_) => return,
                        };
                        let name = if name_input.trim().is_empty() {
                            format!("User Preset {}", vs.cache.len() + 1)
                        } else {
                            name_input.trim().to_string()
                        };
                        let profile = profile_from_params(&save_params, &name);
                        let dir = preset_save_dir(&vs.vault_path);
                        let _ = std::fs::create_dir_all(&dir);
                        let safe_name = name.replace(
                            |c: char| !c.is_alphanumeric() && c != ' ' && c != '-' && c != '_',
                            "",
                        );
                        let fp = dir.join(format!("{safe_name}.md"));
                        let md = export_meridian_markdown(&profile);
                        if std::fs::write(&fp, md).is_ok() {
                            if let Some(pos) = vs.cache.iter().position(|(n, _, _)| n == &name) {
                                vs.cache[pos] = (name.clone(), fp.clone(), profile.clone());
                            } else {
                                vs.cache.push((name.clone(), fp.clone(), profile.clone()));
                            }
                            vs.names = merge_preset_names(&vs.cache);
                            ui.set_preset_names(names_model(&vs.names));
                            ui.set_preset_name(SharedString::from(name.as_str()));
                            tracing::info!("SAVE preset to {}", fp.display());
                            if let Some(ref vault) = vs.vault_path.clone()
                                && !vault.is_empty()
                            {
                                let scan_gen = vs.pending.bump_generation();
                                vs.scanning_for = Some(vault.clone());
                                spawn_vault_scan(vault.clone(), vs.pending.clone(), scan_gen);
                            }
                        }
                        name
                    };
                    let _ = name;
                });

                let vs_path = vault_state.clone();
                let ui_path = ui.as_weak();
                ui.on_vault_path_changed(move |path: SharedString| {
                    let path = path.to_string().trim().to_string();
                    let new_vp = if path.is_empty() { None } else { Some(path) };
                    if let Ok(mut vs) = vs_path.lock() {
                        vs.vault_path = new_vp.clone();
                        let mut cfg = load_config("Meridian");
                        cfg.vault_path = new_vp.clone();
                        let _ = save_config("Meridian", &cfg);
                        let scan_gen = vs.pending.bump_generation();
                        let scan_arg = new_vp.clone().unwrap_or_default();
                        vs.scanning_for = new_vp.clone();
                        spawn_vault_scan(scan_arg, vs.pending.clone(), scan_gen);
                        if let Some(ui) = ui_path.upgrade() {
                            if new_vp.as_ref().is_none_or(|v| v.is_empty()) {
                                ui.set_snap_label(SharedString::from("SET VAULT"));
                            } else {
                                ui.set_snap_label(SharedString::from("SNAP"));
                            }
                        }
                    }
                });

                // Vault Setup PASTE: write draft path only (vault_setup_path).
                // Ctrl+V also works via slint-baseview clipboard_get_retry; this
                // is the DAW-steal fallback button.
                let paste_ui = ui.as_weak();
                ui.on_vault_paste_requested(move || {
                    let Some(ui) = paste_ui.upgrade() else { return };
                    match clipboard_get_retry() {
                        Some(s) => {
                            ui.set_vault_setup_path(SharedString::from(s));
                            ui.set_vault_paste_status(SharedString::new());
                        }
                        None => {
                            ui.set_vault_paste_status(SharedString::from(
                                "Clipboard empty or unavailable — copy a path and try PASTE again",
                            ));
                        }
                    }
                });

                let sel_state = state.clone();
                let sel_params = params.clone();
                let sel_vs = vault_state.clone();
                let sel_ui = ui.as_weak();
                ui.on_preset_selected(move |name: SharedString| {
                    let name = name.to_string();
                    let profile = {
                        let vs = sel_vs.lock().ok();
                        let (vp, cache) = vs
                            .as_ref()
                            .map(|g| (g.vault_path.clone(), g.cache.clone()))
                            .unwrap_or((None, vec![]));
                        find_profile(&name, &vp, &cache)
                    };
                    if let Some(profile) = profile {
                        apply_profile(&sel_state, &sel_params, &profile);
                        if let Some(ui) = sel_ui.upgrade() {
                            ui.set_preset_name(SharedString::from(profile.name.as_str()));
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
                    if shared_loud.auto_loud.measuring.load(Ordering::Acquire) {
                        return; // already measuring
                    }
                    shared_loud
                        .auto_loud.trigger
                        .store(true, Ordering::Release);
                });

                ui
            }
        },
        {
            let shared_for_sync = shared.clone();
            let params_for_curve = params.clone();
            let vault_state_sync = vault_state.clone();
            move |ui: &MeridianUi, state: &LxPluginContext<MeridianParams>| {
                let Ok(mut cache) = sync_cache.lock() else {
                    return;
                };
                // Vizia Ticker: heavy host→UI work at ~30 Hz only.
                if !cache.due() {
                    return;
                }

                // Drain background vault scan (non-blocking).
                if let Ok(mut vs) = vault_state_sync.try_lock()
                    && vs.pending.ready.swap(false, Ordering::Acquire)
                {
                    let current_gen = vs.pending.generation.load(Ordering::Acquire);
                    let scanned = {
                        let guard = vs.pending.presets.try_lock().ok();
                        guard.and_then(|g| match &*g {
                            Some((scan_gen, scanned)) if *scan_gen == current_gen => {
                                Some(scanned.clone())
                            }
                            _ => None,
                        })
                    };
                    if let Some(scanned) = scanned {
                        vs.cache = scanned;
                        vs.names = merge_preset_names(&vs.cache);
                        ui.set_preset_names(names_model(&vs.names));
                    }
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
                let sr = shared.spectrum.sample_rate.load(Ordering::Relaxed).max(1.0);

                // --- EQ curve (cached like vizia EqCurveKey) ---
                let key = eq_curve_key(&params_for_curve, sr);
                if cache.eq_key != Some(key) {
                    cache.eq_key = Some(key);
                    cache.eq_cmds = eq_curve_path(&params_for_curve, sr);
                    ui.set_curve_cmds(slint::SharedString::from(cache.eq_cmds.as_str()));
                }

                // --- meters ---
                let peak_l_db = shared.peaks.output_peak_l.load(Ordering::Relaxed);
                let peak_r_db = shared.peaks.output_peak_r.load(Ordering::Relaxed);
                let hold_l_db = shared.peaks.peak_hold_l.load(Ordering::Relaxed);
                let hold_r_db = shared.peaks.peak_hold_r.load(Ordering::Relaxed);
                let gr_db = shared.gain_reduction.load(Ordering::Relaxed).max(0.0);
                let corr = shared.peaks.phase_correlation.load(Ordering::Relaxed);
                let balance = shared.peaks.balance.load(Ordering::Relaxed);

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
                    ui.set_corr_text(slint::SharedString::from(format!(
                        "correlation: {corr:.2}"
                    )));
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
                let measuring = shared.auto_loud.measuring.load(Ordering::Acquire);
                if changed_bool(&mut cache.auto_loud, measuring) {
                    ui.set_auto_loud_measuring(measuring);
                }
                if cache.was_measuring && !measuring {
                    let offset = shared.auto_loud.gain_offset.load(Ordering::Acquire);
                    shared.auto_loud.gain_offset.store(0.0, Ordering::Release);
                    if offset.abs() > 0.01 {
                        let cur_db = state.output_gain.raw_target() as f32;
                        let new_db = (cur_db + offset).clamp(-12.0, 12.0);
                        let norm = ((new_db + 12.0) / 24.0) as f64;
                        state.automate(P::OutputGain, norm.clamp(0.0, 1.0));
                    }
                }
                cache.was_measuring = measuring;

                // --- SNAP: label blink + write SNAPSHOT-*.md on complete ---
                let snap_now = shared.snap.active.load(Ordering::Acquire);
                let vault_path = vault_state_sync
                    .try_lock()
                    .ok()
                    .and_then(|g| g.vault_path.clone());
                if snap_now {
                    cache.snap_blink = cache.snap_blink.wrapping_add(1);
                    // Blink label while analyzing (vizia: alternate color; we flip text).
                    let label = if (cache.snap_blink / 8).is_multiple_of(2) {
                        "ANALYZE..."
                    } else {
                        "ANALYZE· · ·"
                    };
                    ui.set_snap_label(SharedString::from(label));
                } else if cache.snap_was_active {
                    // Falling edge: write snapshot file (vizia tick parity).
                    if let Some(vp) = vault_path.as_ref().filter(|v| !v.is_empty()) {
                        let stereo = shared
                            .snap.stereo
                            .try_lock()
                            .ok()
                            .map(|v| v.clone())
                            .unwrap_or_else(|| vec![-90.0; 1024]);
                        let mono = shared
                            .snap.mono
                            .try_lock()
                            .ok()
                            .map(|v| v.clone())
                            .unwrap_or_else(|| vec![-90.0; 1024]);
                        let delta = shared
                            .snap.delta
                            .try_lock()
                            .ok()
                            .map(|v| v.clone())
                            .unwrap_or_else(|| vec![-90.0; 1024]);
                        let mut band_levels = [0.0f32; 5];
                        for (dst, src) in band_levels.iter_mut().zip(shared.band_levels.iter()) {
                            *dst = src.load(Ordering::Acquire);
                        }
                        let md = snap_markdown(
                            &stereo,
                            &mono,
                            &delta,
                            band_levels,
                            corr,
                            peak_l_db,
                            peak_r_db,
                            sr,
                        );
                        let fname = snap_filename(vp);
                        let path = std::path::Path::new(vp).join(&fname);
                        if std::fs::write(&path, &md).is_ok() {
                            tracing::info!("SNAP saved {}", path.display());
                        }
                    }
                    cache.snap_blink = 0;
                    let label = if vault_path.as_ref().is_none_or(|v| v.is_empty()) {
                        "SET VAULT"
                    } else {
                        "SNAP"
                    };
                    ui.set_snap_label(SharedString::from(label));
                }
                cache.snap_was_active = snap_now;

                // --- spectrum & goniometer at tick rate only (~30 Hz, vizia parity) ---
                // path-h must match LxSpectrum path-h / FFT card height (165).
                ui.set_spectrum_smooth(shared.spectrum.smooth.load(Ordering::Relaxed));
                let (cmds, fill_top) = spectrum_path(shared, 620.0, 165.0);
                ui.set_spectrum_path(slint::SharedString::from(cmds));
                // Rebuild the fill brush only when the peak moves ≥2 % of height.
                let fq = (fill_top * 50.0).round() / 50.0;
                if changed_f32(&mut cache.spectrum_fill_q, fq) {
                    ui.set_spectrum_fill(spectrum_fill_brush(fq));
                }
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

/// 1/3-octave fractional-band smoothing (Lucent `lx-ui` canvas parity).
/// Returns 241 log-spaced dB points from 20 Hz to 20 kHz.
fn smooth_spectrum_third_octave(spectrum: &[f32], sample_rate: f32) -> Vec<f32> {
    if spectrum.is_empty() {
        return Vec::new();
    }
    let fft_size = SPECTRUM_BINS * 2;
    let log_min = 20.0_f32.ln();
    let log_max = 20000.0_f32.ln();
    let bin_hz = sample_rate / fft_size as f32;
    const DENOM_LOW: f32 = 3.0;
    const DENOM_HIGH: f32 = 20.0;
    const F_LOW: f32 = 500.0;
    const F_HIGH: f32 = 16000.0;
    let taper_lo = F_LOW.ln();
    let taper_hi = F_HIGH.ln();
    const STEPS: usize = 240;
    let len = spectrum.len();

    let power: Vec<f32> = spectrum
        .iter()
        .map(|&db| 10.0_f32.powf(db * 0.1))
        .collect();

    let mut out = Vec::with_capacity(STEPS + 1);
    for i in 0..=STEPS {
        let frac = i as f32 / STEPS as f32;
        let ln_fc = log_min + (log_max - log_min) * frac;
        let fc = ln_fc.exp();
        let t = ((ln_fc - taper_lo) / (taper_hi - taper_lo)).clamp(0.0, 1.0);
        let denom = DENOM_LOW + (DENOM_HIGH - DENOM_LOW) * t;
        let half = 2.0_f32.powf(1.0 / (2.0 * denom));
        const MIN_BIN: f32 = 1.0;
        let lo = (fc / half / bin_hz).clamp(MIN_BIN, (len - 1) as f32);
        let hi = (fc * half / bin_hz).clamp(MIN_BIN, (len - 1) as f32);
        let avg_power = if hi - lo >= 1.0 {
            let i0 = lo.floor() as usize;
            let i1 = hi.floor() as usize;
            let mut sum = 0.0f32;
            if i0 == i1 {
                sum = power[i0] * (hi - lo);
            } else {
                sum += power[i0] * ((i0 + 1) as f32 - lo);
                for p in &power[i0 + 1..i1] {
                    sum += *p;
                }
                sum += power[i1] * (hi - i1 as f32);
            }
            sum / (hi - lo)
        } else {
            let pos = (fc / bin_hz).clamp(MIN_BIN, (len - 1) as f32);
            let i0 = pos.floor() as usize;
            let i1 = (i0 + 1).min(len - 1);
            let t_bin = pos - i0 as f32;
            power[i0] * (1.0 - t_bin) + power[i1] * t_bin
        };
        out.push((10.0 * avg_power.max(1e-12).log10()).clamp(-90.0, 12.0));
    }
    if out.len() >= 3 {
        let raw = out.clone();
        for i in 1..out.len().saturating_sub(1) {
            out[i] = raw[i - 1] * 0.25 + raw[i] * 0.5 + raw[i + 1] * 0.25;
        }
    }
    out
}

fn spectrum_path(shared: &MeridianShared, w: f32, h: f32) -> (String, f32) {
    use aura_dsp::analysis::SPECTRUM_BINS;
    const MIN_DB: f32 = -70.0;
    const MAX_DB: f32 = -18.0;

    let bins = shared
        .spectrum.avg
        .try_lock()
        .map(|b| b.clone())
        .or_else(|_| shared.spectrum.bins.try_lock().map(|b| b.clone()))
        .unwrap_or_default();
    let n = bins.len().min(SPECTRUM_BINS);
    if n < 2 {
        return (String::new(), 1.0);
    }

    let sr = shared.spectrum.sample_rate.load(Ordering::Relaxed).max(1.0);
    let fft_size = (n * 2) as f32;
    let log_f = |f: f32| -> f32 {
        ((f.max(20.0).ln() - 20.0f32.ln()) / (20000.0f32.ln() - 20.0f32.ln())).clamp(0.0, 1.0)
    };
    let db_to_y = |db: f32| -> f32 {
        let norm = ((db - MIN_DB) / (MAX_DB - MIN_DB)).clamp(0.0, 1.0);
        h - norm * h
    };

    // Collect log-x samples. SMOOTH on = 1/3-octave fractional-band average
    // (Lucent parity); off = raw log-spaced bins (skip sub-20 Hz bins).
    let pts: Vec<(f32, f32)> = if shared.spectrum.smooth.load(Ordering::Relaxed) {
        let sm = smooth_spectrum_third_octave(&bins[..n], sr);
        let denom = sm.len().saturating_sub(1).max(1) as f32;
        sm.iter()
            .enumerate()
            .map(|(i, &db)| (i as f32 / denom * w, db_to_y(db.clamp(MIN_DB, MAX_DB))))
            .collect()
    } else {
        let mut pts: Vec<(f32, f32)> = Vec::with_capacity(n);
        for (i, &bin) in bins.iter().enumerate().take(n) {
            let freq = i as f32 * sr / fft_size;
            if freq < 20.0 {
                continue;
            }
            if freq > 20000.0 {
                break;
            }
            let x = log_f(freq) * w;
            let y = db_to_y(bin.clamp(MIN_DB, MAX_DB));
            pts.push((x, y));
        }
        pts
    };
    if pts.len() < 2 {
        return (String::new(), 1.0);
    }

    // Fill gradient anchor: highest curve point (min y) as 0..1 of height.
    // The fill brush fades from here to transparent at the floor — relative
    // to the spectrum, not the widget height.
    let fill_top = pts.iter().map(|p| p.1).fold(f32::INFINITY, f32::min) / h;

    // Stroke+fill path: closed loop from left baseline -> up to first point ->
    // along the curve -> down to right baseline -> back to left baseline.
    // Baseline is h+2 and both corners sit outside the viewBox, so the closing
    // edges (bottom + sides) are clipped — only the curve gets stroked.
    let mut s = String::with_capacity(pts.len() * 22 + 48);
    let base = h + 2.0;
    s.push_str(&format!("M -2.0 {:.1} L {:.1} {:.1}", base, pts[0].0, pts[0].1));
    for &(x, y) in pts.iter().skip(1) {
        s.push_str(&format!(" L {x:.1} {y:.1}"));
    }
    let last_x = pts.last().unwrap().0;
    s.push_str(&format!(" L {:.1} {:.1} L {:.1} {:.1} Z", last_x, base, w + 2.0, base));
    (s, fill_top)
}

/// Fill brush for the spectrum: opaque at the curve's peak, fading to
/// transparent at the pit floor — anchored to the spectrum, not the widget.
fn spectrum_fill_brush(top: f32) -> slint::Brush {
    use slint::private_unstable_api::re_exports::{GradientStop, LinearGradientBrush};
    let c = |a: u8| slint::Color::from_argb_u8(a, 0x19, 0xe6, 0xb3);
    let t = top.clamp(0.0, 0.85);
    let span = 1.0 - t;
    slint::Brush::LinearGradient(LinearGradientBrush::new(
        180.0,
        [
            GradientStop {
                color: slint::Color::from_argb_u8(0xff, 0x7d, 0xf3, 0xdd),
                position: t,
            },
            GradientStop {
                color: c(0xf0),
                position: t + span * 0.55,
            },
            GradientStop {
                color: c(0xb3),
                position: t + span * 0.85,
            },
            GradientStop {
                color: c(0x00),
                position: 1.0,
            },
        ],
    ))
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

fn gonio_path(shared: &MeridianShared, w: f32, h: f32) -> String {
    use aura_dsp::analysis::SCOPE_BUFFER_LEN;
    let (samples, write_pos) = {
        let pos = shared.scope.write_pos.load(Ordering::Relaxed);
        let samples = match shared.scope.samples.try_lock() {
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
    const H: f32 = 165.0; // matches FFT card path-h
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
