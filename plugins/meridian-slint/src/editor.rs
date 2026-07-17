use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use truce::prelude::*;
use truce_core::cast::{discrete_index, discrete_norm};
use truce_core::editor::{Editor, PluginContextReadF32};
use truce_slint::{PluginContext, SlintEditor, SyncFn};

use crate::MeridianParams;
use crate::MeridianParamsParamId as P;
use shared_dsp::{Biquad, TiltEq};

slint::include_modules!();

slint::include_modules!();

// Frozen vault size (ui-layout-spec): 990 × 660
const WINDOW_W: u32 = 990;
const WINDOW_H: u32 = 660;

// --- binding macros ---------------------------------------------------------

macro_rules! bind_floats {
    ($ui:expr, $state:expr, $($p:expr => $name:ident),* $(,)?) => {
        $(
            truce_slint::paste! {
                let s = $state.clone();
                $ui.[<on_ $name _changed>](move |v| s.automate($p, v as f64));
            }
        )*
    };
}

macro_rules! bind_ints {
    ($ui:expr, $state:expr, $count:expr, $($p:expr => $name:ident),* $(,)?) => {
        $(
            truce_slint::paste! {
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
            truce_slint::paste! {
                let s = $state.clone();
                $ui.[<on_ $name _changed>](move |v: bool| {
                    s.automate($p, if v { 1.0 } else { 0.0 });
                });
            }
        )*
    };
}

macro_rules! sync_floats {
    ($ui:expr, $state:expr, $($p:expr => $name:ident),* $(,)?) => {
        $(
            truce_slint::paste! {
                $ui.[<set_ $name>](PluginContextReadF32::get_param($state, $p));
                $ui.[<set_ $name _text>](slint::SharedString::from($state.format_param($p)));
            }
        )*
    };
}

macro_rules! sync_ints {
    ($ui:expr, $state:expr, $count:expr, $($p:expr => $name:ident),* $(,)?) => {
        $(
            truce_slint::paste! {
                let idx = discrete_index(PluginContextReadF32::get_param($state, $p) as f64, $count) as f32;
                $ui.[<set_ $name>](idx);
            }
        )*
    };
}

macro_rules! sync_bools {
    ($ui:expr, $state:expr, $($p:expr => $name:ident),* $(,)?) => {
        $(
            truce_slint::paste! {
                $ui.[<set_ $name>](PluginContextReadF32::get_param($state, $p) > 0.5);
            }
        )*
    };
}

pub fn build_editor(params: Arc<MeridianParams>) -> Box<dyn Editor> {
    let shared = params.shared.clone();
    SlintEditor::new(
        params.clone(),
        (WINDOW_W, WINDOW_H),
        move |state: PluginContext<MeridianParams>| -> SyncFn<MeridianParams> {
            let ui = MeridianUi::new().unwrap();

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

            bind_ints!(ui, state, 3,
                P::CutSlope => cut_slope,
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
            let snap_state = state.clone();
            ui.on_snap_clicked(move || {
                snap_state
                    .shared
                    .snap_active
                    .store(true, std::sync::atomic::Ordering::Release);
                tracing::info!("SNAP triggered");
            });

            let save_state = state.clone();
            let save_params = params.clone();
            let save_shared = shared.clone();
            ui.on_save_clicked(move || {
                // Minimal preset save: store current parameter values in a JSON file
                // under the plugin's local presets directory.
                let dir = shared_analysis::get_plugin_dir("Meridian").join("presets");
                let _ = std::fs::create_dir_all(&dir);
                let name = format!("meridian_preset_{}", chrono::Utc::now().format("%Y%m%d_%H%M%S"));
                let fp = dir.join(format!("{name}.json"));
                // Build a simple JSON with all float/int/bool param values
                let mut map = serde_json::Map::new();
                // Float params
                macro_rules! store_float {
                    ($p:ident) => {
                        map.insert(
                            stringify!($p).to_string(),
                            serde_json::Value::Number(
                                serde_json::Number::from_f64(save_params.$p.raw_target()).unwrap_or(0.into()),
                            ),
                        );
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
                // Int params
                macro_rules! store_int {
                    ($p:ident) => {
                        map.insert(
                            stringify!($p).to_string(),
                            serde_json::Value::Number(
                                serde_json::Number::from(save_params.$p.value()).unwrap_or(0.into()),
                            ),
                        );
                    };
                }
                store_int!(cut_slope);
                store_int!(bass_slope);
                store_int!(lo_mid_slope);
                store_int!(mid_slope);
                store_int!(high_slope);
                store_int!(excite_slope);
                // Bool params
                macro_rules! store_bool {
                    ($p:ident) => {
                        map.insert(
                            stringify!($p).to_string(),
                            serde_json::Value::Bool(save_params.$p.value()),
                        );
                    };
                }
                store_bool!(mono_active);
                store_bool!(delta_active);
                store_bool!(bypass_active);
                store_bool!(inflate_band_split);
                store_bool!(inflate_clip);
                let json = serde_json::Value::Object(map);
                if let Ok(content) = serde_json::to_string_pretty(&json) {
                    let _ = std::fs::write(&fp, content);
                    tracing::info!("SAVE preset to {}", fp.display());
                }
            });

            let vault_state = Arc::new(std::sync::Mutex::new(VaultUiState {
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
                    let mut cfg = shared_analysis::load_config("Meridian");
                    cfg.vault_path = new_vp.clone();
                    let _ = shared_analysis::save_config("Meridian", &cfg);
                    let scan_gen = vs.pending.bump_generation();
                    if let Some(ref vp) = new_vp {
                        vs.scanning_for = Some(vp.clone());
                        spawn_vault_scan(vp.clone(), vs.pending.clone(), scan_gen);
                    } else {
                        vs.names = Vec::new();
                        vs.cache.clear();
                        vs.scanning_for = None;
                    }
                }
            });

            let reset_state = state.clone();
            ui.on_reset_clicked(move || {
                // Reset all parameters to their defaults (as defined in MeridianParams)
                // Float params
                macro_rules! reset_float {
                    ($p:ident, $default:expr) => {
                        let norm = ((($default as f64) - $p.info.range.min) / ($p.info.range.max - $p.info.range.min)).clamp(0.0, 1.0);
                        reset_state.automate(P::$p, norm);
                    };
                }
                // We need the param info, but we can't access it here easily.
                // Instead, we use the known default values from the struct definition.
                // For simplicity, we set the normalized value to the default normalized value.
                // Since we don't have the param info, we'll use a helper function.
                fn reset_float_param(state: &PluginContext<MeridianParams>, pid: P, default_plain: f32, min: f32, max: f32) {
                    let norm = ((default_plain - min) / (max - min)).clamp(0.0, 1.0) as f64;
                    state.automate(pid, norm);
                }
                fn reset_int_param(state: &PluginContext<MeridianParams>, pid: P, default_val: i64, count: usize) {
                    state.automate(pid, discrete_norm(default_val as usize, count));
                }
                fn reset_bool_param(state: &PluginContext<MeridianParams>, pid: P, default_val: bool) {
                    state.automate(pid, if default_val { 1.0 } else { 0.0 });
                }
                // HPF/LPF
                reset_float_param(&reset_state, P::HpfFreq, 2.0, 2.0, 2000.0);
                reset_float_param(&reset_state, P::LpfFreq, 35000.0, 200.0, 35000.0);
                reset_int_param(&reset_state, P::CutSlope, 0, 2);
                // EQ bands
                reset_float_param(&reset_state, P::BassGain, 0.0, -12.0, 12.0);
                reset_int_param(&reset_state, P::BassSlope, 1, 3);
                reset_float_param(&reset_state, P::LoMidGain, 0.0, -12.0, 12.0);
                reset_int_param(&reset_state, P::LoMidSlope, 1, 3);
                reset_float_param(&reset_state, P::MidGain, 0.0, -12.0, 12.0);
                reset_int_param(&reset_state, P::MidSlope, 1, 3);
                reset_float_param(&reset_state, P::HighGain, 0.0, -12.0, 12.0);
                reset_int_param(&reset_state, P::HighSlope, 1, 3);
                reset_float_param(&reset_state, P::ExciteGain, 0.0, -12.0, 12.0);
                reset_int_param(&reset_state, P::ExciteSlope, 1, 3);
                // EQ frequencies
                reset_float_param(&reset_state, P::EqFreq1, 80.0, 40.0, 200.0);
                reset_float_param(&reset_state, P::EqFreq2, 300.0, 150.0, 800.0);
                reset_float_param(&reset_state, P::EqFreq3, 1000.0, 500.0, 3000.0);
                reset_float_param(&reset_state, P::EqFreq4, 4000.0, 2000.0, 10000.0);
                reset_float_param(&reset_state, P::EqFreq5, 12000.0, 6000.0, 20000.0);
                // Tilt
                reset_float_param(&reset_state, P::TiltGain, 0.0, -1.5, 1.5);
                // Warmth
                reset_float_param(&reset_state, P::WarmthDrive, 0.0, 0.0, 12.0);
                reset_float_param(&reset_state, P::WarmthMix, 0.0, 0.0, 100.0);
                // Exciter
                reset_float_param(&reset_state, P::ExciteAmount, 0.0, 0.0, 30.0);
                reset_float_param(&reset_state, P::ExciteBlend, 0.0, 0.0, 100.0);
                reset_float_param(&reset_state, P::ExciteFreq, 8000.0, 6000.0, 12000.0);
                // Compressor
                reset_float_param(&reset_state, P::CompThreshold, 0.0, -30.0, 0.0);
                reset_float_param(&reset_state, P::CompMix, 0.0, 0.0, 100.0);
                reset_float_param(&reset_state, P::CompAttack, 15.0, 5.0, 50.0);
                reset_float_param(&reset_state, P::CompRelease, 120.0, 50.0, 300.0);
                reset_float_param(&reset_state, P::CompCharacter, 2.0, 1.5, 4.0);
                reset_float_param(&reset_state, P::CompMakeup, 0.0, 0.0, 12.0);
                // Inflate
                reset_float_param(&reset_state, P::InflateEffect, 0.0, 0.0, 100.0);
                reset_float_param(&reset_state, P::InflateCurve, 0.0, -50.0, 50.0);
                reset_bool_param(&reset_state, P::InflateBandSplit, false);
                reset_bool_param(&reset_state, P::InflateClip, false);
                // Stereo/Routing
                reset_float_param(&reset_state, P::StereoWidth, 100.0, 0.0, 200.0);
                reset_float_param(&reset_state, P::Pan, 0.0, -1.0, 1.0);
                reset_float_param(&reset_state, P::OutputGain, 0.0, -12.0, 12.0);
                reset_bool_param(&reset_state, P::MonoActive, false);
                reset_bool_param(&reset_state, P::DeltaActive, false);
                reset_bool_param(&reset_state, P::BypassActive, false);
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

            let shared_for_sync = shared.clone();
            let params_for_curve = params.clone();
            let shared_for_curve = shared.clone();
            let was_measuring = RefCell::new(false);
            // GR envelope mini-display state (matches vizia telemetry)
            let gr_history: RefCell<Vec<f32>> = RefCell::new(vec![0.0; 90]);
            let gr_peak_hold: RefCell<f32> = RefCell::new(0.0);
            let gr_peak_hold_ticks: RefCell<u32> = RefCell::new(0);

            // Vault UI state struct (needed for vault path handling)
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
                presets: std::sync::Mutex<Option<(u32, Vec<(String, std::path::PathBuf, ())>)>>,
            }
            impl PendingPresets {
                fn new() -> Self {
                    Self {
                        ready: std::sync::atomic::AtomicBool::new(false),
                        generation: std::sync::atomic::AtomicU32::new(0),
                        presets: std::sync::Mutex::new(None),
                    }
                }
                fn bump_generation(&self) -> u32 {
                    let new = self.generation.load(std::sync::atomic::Ordering::Relaxed).wrapping_add(1);
                    self.generation.store(new, std::sync::atomic::Ordering::Release);
                    self.ready.store(false, std::sync::atomic::Ordering::Release);
                    if let Ok(mut guard) = self.presets.lock() {
                        *guard = None;
                    }
                    new
                }
            }
            fn spawn_vault_scan(vp: String, pending: Arc<PendingPresets>, generation: u32) {
                std::thread::spawn(move || {
                    // Minimal scan: just mark ready
                    if let Ok(mut guard) = pending.presets.lock() {
                        *guard = Some((generation, Vec::new()));
                    }
                    pending.ready.store(true, std::sync::atomic::Ordering::Release);
                });
            }

            Box::new(move |state: &PluginContext<MeridianParams>| {
                // --- host → UI normalized values ---
                // sync_floats! macro is defined above; we keep it but remove the default_norm lines
                // The macro currently sets default_norm; we need to modify it.
                // For now, we call the macro as is, but we'll fix the macro definition later.
                sync_floats!(ui, state,
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

                sync_ints!(ui, state, 3,
                    P::CutSlope => cut_slope,
                    P::BassSlope => bass_slope,
                    P::LoMidSlope => lo_mid_slope,
                    P::MidSlope => mid_slope,
                    P::HighSlope => high_slope,
                    P::ExciteSlope => excite_slope,
                );

                sync_bools!(ui, state,
                    P::MonoActive => mono_active,
                    P::DeltaActive => delta_active,
                    P::BypassActive => bypass_active,
                    P::InflateBandSplit => inflate_band_split,
                    P::InflateClip => inflate_clip,
                );

                // --- EQ curve ---
                let sr = shared_for_curve.sample_rate.load(Ordering::Relaxed).max(1.0);
                let cmds = eq_curve_path(&params_for_curve, sr);
                ui.set_curve_cmds(slint::SharedString::from(cmds));

                // --- meters ---
                let shared = &shared_for_sync;
                let peak_l_db = shared.output_peak_l.load(Ordering::Relaxed);
                let peak_r_db = shared.output_peak_r.load(Ordering::Relaxed);
                let hold_l_db = shared.peak_hold_l.load(Ordering::Relaxed);
                let hold_r_db = shared.peak_hold_r.load(Ordering::Relaxed);
                let gr_db = shared.gain_reduction.load(Ordering::Relaxed).max(0.0);
                let corr = shared.phase_correlation.load(Ordering::Relaxed);
                let balance = shared.balance.load(Ordering::Relaxed);

                // Stereo meter: map −60..+6 dB → 0..1 (matches frozen LxStereoMeter ticks)
                ui.set_meter_l(db_to_meter(peak_l_db));
                ui.set_meter_r(db_to_meter(peak_r_db));
                ui.set_peak_hold_l(db_to_meter(hold_l_db));
                ui.set_peak_hold_r(db_to_meter(hold_r_db));
                ui.set_gr(db_to_gr(gr_db));
                ui.set_gr_text(slint::SharedString::from(format!("GR: {gr_db:.1}")));
                ui.set_correlation(corr);
                ui.set_balance(balance);
                ui.set_corr_text(slint::SharedString::from(format!("corr: {corr:.2}")));
                ui.set_balance_text(slint::SharedString::from(format!("bal: {balance:.2}")));
                ui.set_peak_l_text(slint::SharedString::from(fmt_db(hold_l_db)));
                ui.set_peak_r_text(slint::SharedString::from(fmt_db(hold_r_db)));

                // GR envelope + peak-hold (original footer mini-display)
                {
                    let mut hist = gr_history.borrow_mut();
                    hist.push(gr_db);
                    if hist.len() > 90 {
                        hist.remove(0);
                    }
                    let mut hold = gr_peak_hold.borrow_mut();
                    let mut ticks = gr_peak_hold_ticks.borrow_mut();
                    if gr_db > *hold {
                        *hold = gr_db;
                        *ticks = 90;
                    } else if *ticks > 0 {
                        *ticks -= 1;
                    } else {
                        *hold = (*hold - 0.15).max(gr_db).max(0.0);
                    }
                    // Original PK label = GR peak-hold, not compressor input peak
                    ui.set_comp_peak_text(slint::SharedString::from(format!("PK: {:.1}", *hold)));
                    ui.set_gr_envelope_cmds(slint::SharedString::from(gr_envelope_path(
                        &hist, gr_db, 110.0, 48.0,
                    )));
                }

                // --- Auto Loud status + apply LUFS offset when measure ends ---
                let measuring = shared.auto_loud_measuring.load(Ordering::Acquire);
                ui.set_auto_loud_measuring(measuring);
                {
                    let mut was = was_measuring.borrow_mut();
                    if *was && !measuring {
                        // Measurement just finished — offset is dB to add to Output Gain.
                        let offset = shared.auto_loud_gain_offset.load(Ordering::Acquire);
                        shared.auto_loud_gain_offset.store(0.0, Ordering::Release);
                        if offset.abs() > 0.01 {
                            let cur_db = state.params().output_gain.raw_target() as f32;
                            let new_db = (cur_db + offset).clamp(-12.0, 12.0);
                            // linear(-12, 12) → normalize
                            let norm = ((new_db + 12.0) / 24.0) as f64;
                            state.automate(P::OutputGain, norm.clamp(0.0, 1.0));
                        }
                    }
                    *was = measuring;
                }

                // --- spectrum & goniometer paths ---
                // Main spectrum ≈ 990 − 180 − 155 − pad ≈ 620 × 170
                ui.set_spectrum_path(slint::SharedString::from(spectrum_path(shared, 620.0, 170.0)));
                // Right-bar gonio is square ~139×139
                ui.set_gonio_path(slint::SharedString::from(gonio_path(shared, 139.0, 139.0)));
            })
        },
    )
    .into_editor()
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

fn spectrum_path(shared: &shared_analysis::SharedState, w: f32, h: f32) -> String {
    use shared_analysis::SPECTRUM_BINS;
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

fn gonio_path(shared: &shared_analysis::SharedState, w: f32, h: f32) -> String {
    use shared_analysis::SCOPE_BUFFER_LEN;
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
    const H: f32 = 170.0;
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
