use std::sync::atomic::Ordering;
use std::sync::Arc;

use truce::prelude::*;
use truce_core::cast::{discrete_index, discrete_norm};
use truce_core::editor::{Editor, PluginContextReadF32};
use truce_slint::{PluginContext, SlintEditor, SyncFn};

use crate::MeridianParams;
use crate::MeridianParamsParamId as P;

slint::include_modules!();

const WINDOW_W: u32 = 1080;
const WINDOW_H: u32 = 720;

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

            let shared_for_sync = shared.clone();

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

                // --- meters ---
                let shared = &shared_for_sync;
                let peak_l_db = shared.output_peak_l.load(Ordering::Relaxed);
                let peak_r_db = shared.output_peak_r.load(Ordering::Relaxed);
                let hold_l_db = shared.peak_hold_l.load(Ordering::Relaxed);
                let hold_r_db = shared.peak_hold_r.load(Ordering::Relaxed);
                let gr_db = shared.gain_reduction.load(Ordering::Relaxed);
                let corr = shared.phase_correlation.load(Ordering::Relaxed);
                let balance = shared.balance.load(Ordering::Relaxed);

                ui.set_meter_l(db_to_meter(peak_l_db));
                ui.set_meter_r(db_to_meter(peak_r_db));
                ui.set_peak_hold_l(db_to_meter(hold_l_db));
                ui.set_peak_hold_r(db_to_meter(hold_r_db));
                ui.set_gr(db_to_gr(gr_db));
                ui.set_corr_text(slint::SharedString::from(format!("corr: {corr:.2}")));
                ui.set_balance_text(slint::SharedString::from(format!("bal: {balance:.2}")));

                // --- spectrum & goniometer paths ---
                ui.set_spectrum_path(slint::SharedString::from(spectrum_path(shared, 620.0, 220.0)));
                ui.set_gonio_path(slint::SharedString::from(gonio_path(shared, 280.0, 220.0)));
            })
        },
    )
    .into_editor()
}

// --- meter helpers --------------------------------------------------------

fn db_to_meter(db: f32) -> f32 {
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
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

// --- goniometer path ------------------------------------------------------

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
    let pad = 8.0;
    let draw_w = w - pad * 2.0;
    let draw_h = h - pad * 2.0;

    for i in 0..points_to_take {
        let idx = (write_pos + SCOPE_BUFFER_LEN - points_to_take + i) % SCOPE_BUFFER_LEN;
        let [l, r] = samples[idx];
        let x = pad + (l + 1.0) * 0.5 * draw_w;
        let y = pad + (1.0 - (r + 1.0) * 0.5) * draw_h;
        if i == 0 {
            s.push_str(&format!("M {x:.1} {y:.1}"));
        } else {
            s.push_str(&format!(" L {x:.1} {y:.1}"));
        }
    }
    s
}
