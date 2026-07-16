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
            ui.on_snap_clicked(move || {
                // TODO: wire SnapFFT trigger
                tracing::info!("SNAP clicked");
            });
            ui.on_save_clicked(move || {
                // TODO: wire preset save
                tracing::info!("SAVE clicked");
            });
            ui.on_vault_path_changed(move |path: slint::SharedString| {
                // TODO: persist vault path
                tracing::info!("VAULT PATH changed: {}", path);
            });
            ui.on_reset_clicked(move || {
                // TODO: wire full reset
                tracing::info!("RESET clicked");
            });

            let shared_for_sync = shared.clone();
            let params_for_curve = params.clone();
            let shared_for_curve = shared.clone();

            Box::new(move |state: &PluginContext<MeridianParams>| {
                // --- host → UI normalized values ---
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
                let gr_db = shared.gain_reduction.load(Ordering::Relaxed);
                let corr = shared.phase_correlation.load(Ordering::Relaxed);
                let balance = shared.balance.load(Ordering::Relaxed);

                // Stereo meter: map −60..+6 dB → 0..1 (matches frozen LxStereoMeter ticks)
                ui.set_meter_l(db_to_meter(peak_l_db));
                ui.set_meter_r(db_to_meter(peak_r_db));
                ui.set_peak_hold_l(db_to_meter(hold_l_db));
                ui.set_peak_hold_r(db_to_meter(hold_r_db));
                ui.set_gr(db_to_gr(gr_db));
                ui.set_correlation(corr);
                ui.set_balance(balance);
                ui.set_corr_text(slint::SharedString::from(format!("corr: {corr:.2}")));
                ui.set_balance_text(slint::SharedString::from(format!("bal: {balance:.2}")));
                ui.set_peak_l_text(slint::SharedString::from(fmt_db(hold_l_db)));
                ui.set_peak_r_text(slint::SharedString::from(fmt_db(hold_r_db)));

                // --- spectrum & goniometer paths ---
                // Main spectrum width ≈ 990 − 180 − 155 − padding ≈ 620
                ui.set_spectrum_path(slint::SharedString::from(spectrum_path(shared, 620.0, 200.0)));
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

fn spectrum_path(shared: &shared_analysis::SharedState, w: f32, h: f32) -> String {
    use shared_analysis::SPECTRUM_BINS;
    let bins = match shared.spectrum_avg.try_lock() {
        Ok(b) => b.clone(),
        Err(_) => return String::new(),
    };
    let n = bins.len().min(SPECTRUM_BINS);
    if n == 0 {
        return String::new();
    }

    let mut s = String::with_capacity(n * 20);
    let floor = h - 2.0;
    s.push_str(&format!("M 0 {floor:.1}"));
    for i in 0..n {
        let t = i as f32 / (n - 1).max(1) as f32;
        let x = t.sqrt() * w;
        let db = bins[i].clamp(-90.0, 0.0);
        let norm = (db + 90.0) / 90.0;
        let y = floor - norm * (h - 4.0);
        s.push_str(&format!(" L {x:.1} {y:.1}"));
    }
    s.push_str(&format!(" L {w:.1} {floor:.1} Z"));
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

    const N: usize = 240;
    const W: f32 = 620.0;
    const H: f32 = 140.0;
    const DB_MIN: f32 = -12.0;
    const DB_MAX: f32 = 12.0;
    let db_range = DB_MAX - DB_MIN;
    let db_to_y = |db: f32| -> f32 {
        let norm = ((db - DB_MIN) / db_range).clamp(0.0, 1.0);
        H - norm * H
    };

    let mut cmds = String::with_capacity(N * 16);
    for i in 0..N {
        let t = i as f32 / (N - 1) as f32;
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
