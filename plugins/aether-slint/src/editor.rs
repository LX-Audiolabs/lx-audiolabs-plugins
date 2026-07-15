use std::sync::atomic::Ordering;
use std::sync::Arc;

use truce::prelude::*;
use truce_core::cast::{discrete_index, discrete_norm};
use truce_core::editor::{Editor, PluginContextReadF32};
use truce_slint::{PluginContext, SlintEditor, SyncFn};

use crate::AetherParams;
use crate::AetherParamsParamId as P;
use crate::{set_band, Biquad, NUM_BANDS};

slint::include_modules!();

const WINDOW_W: u32 = 760;
const WINDOW_H: u32 = 580;

pub fn build_editor(params: Arc<AetherParams>) -> Box<dyn Editor> {
    let shared = params.shared.clone();
    SlintEditor::new(
        params.clone(),
        (WINDOW_W, WINDOW_H),
        move |state: PluginContext<AetherParams>| -> SyncFn<AetherParams> {
            let ui = AetherUi::new().unwrap();

            // --- UI → host callbacks for every parameter ---
            let s = state.clone();
            ui.on_eq1_freq_changed(move |v| s.automate(P::Eq1Freq, v as f64));
            let s = state.clone();
            ui.on_eq1_gain_changed(move |v| s.automate(P::Eq1Gain, v as f64));
            let s = state.clone();
            ui.on_eq1_q_changed(move |v| s.automate(P::Eq1Q, v as f64));
            let s = state.clone();
            ui.on_eq1_type_changed(move |v| s.automate(P::Eq1Type, discrete_norm(v.max(0) as usize, 4)));

            let s = state.clone();
            ui.on_eq2_freq_changed(move |v| s.automate(P::Eq2Freq, v as f64));
            let s = state.clone();
            ui.on_eq2_gain_changed(move |v| s.automate(P::Eq2Gain, v as f64));
            let s = state.clone();
            ui.on_eq2_q_changed(move |v| s.automate(P::Eq2Q, v as f64));
            let s = state.clone();
            ui.on_eq2_type_changed(move |v| s.automate(P::Eq2Type, discrete_norm(v.max(0) as usize, 4)));

            let s = state.clone();
            ui.on_eq3_freq_changed(move |v| s.automate(P::Eq3Freq, v as f64));
            let s = state.clone();
            ui.on_eq3_gain_changed(move |v| s.automate(P::Eq3Gain, v as f64));
            let s = state.clone();
            ui.on_eq3_q_changed(move |v| s.automate(P::Eq3Q, v as f64));
            let s = state.clone();
            ui.on_eq3_type_changed(move |v| s.automate(P::Eq3Type, discrete_norm(v.max(0) as usize, 4)));

            let s = state.clone();
            ui.on_eq4_freq_changed(move |v| s.automate(P::Eq4Freq, v as f64));
            let s = state.clone();
            ui.on_eq4_gain_changed(move |v| s.automate(P::Eq4Gain, v as f64));
            let s = state.clone();
            ui.on_eq4_q_changed(move |v| s.automate(P::Eq4Q, v as f64));
            let s = state.clone();
            ui.on_eq4_type_changed(move |v| s.automate(P::Eq4Type, discrete_norm(v.max(0) as usize, 4)));

            let s = state.clone();
            ui.on_eq5_freq_changed(move |v| s.automate(P::Eq5Freq, v as f64));
            let s = state.clone();
            ui.on_eq5_gain_changed(move |v| s.automate(P::Eq5Gain, v as f64));
            let s = state.clone();
            ui.on_eq5_q_changed(move |v| s.automate(P::Eq5Q, v as f64));
            let s = state.clone();
            ui.on_eq5_type_changed(move |v| s.automate(P::Eq5Type, discrete_norm(v.max(0) as usize, 4)));

            let s = state.clone();
            ui.on_blend_changed(move |v| s.automate(P::Blend, v as f64));
            let s = state.clone();
            ui.on_cf_angle_changed(move |v| s.automate(P::CfAngle, v as f64));
            let s = state.clone();
            ui.on_cf_amount_changed(move |v| s.automate(P::CfAmount, v as f64));
            let s = state.clone();
            ui.on_gain_changed(move |v| s.automate(P::Gain, v as f64));
            let s = state.clone();
            ui.on_cf_realism_changed(move |v| s.automate(P::CfRealism, discrete_norm(v.max(0) as usize, 3)));
            let s = state.clone();
            ui.on_bypass_changed(move |v| s.automate(P::Bypass, if v { 1.0 } else { 0.0 }));

            let params_for_curve = params.clone();
            let shared_for_curve = shared.clone();

            Box::new(move |state: &PluginContext<AetherParams>| {
                // Normalised values for sliders/knobs.
                ui.set_eq1_freq(PluginContextReadF32::get_param(state, P::Eq1Freq));
                ui.set_eq1_gain(PluginContextReadF32::get_param(state, P::Eq1Gain));
                ui.set_eq1_q(PluginContextReadF32::get_param(state, P::Eq1Q));
                ui.set_eq2_freq(PluginContextReadF32::get_param(state, P::Eq2Freq));
                ui.set_eq2_gain(PluginContextReadF32::get_param(state, P::Eq2Gain));
                ui.set_eq2_q(PluginContextReadF32::get_param(state, P::Eq2Q));
                ui.set_eq3_freq(PluginContextReadF32::get_param(state, P::Eq3Freq));
                ui.set_eq3_gain(PluginContextReadF32::get_param(state, P::Eq3Gain));
                ui.set_eq3_q(PluginContextReadF32::get_param(state, P::Eq3Q));
                ui.set_eq4_freq(PluginContextReadF32::get_param(state, P::Eq4Freq));
                ui.set_eq4_gain(PluginContextReadF32::get_param(state, P::Eq4Gain));
                ui.set_eq4_q(PluginContextReadF32::get_param(state, P::Eq4Q));
                ui.set_eq5_freq(PluginContextReadF32::get_param(state, P::Eq5Freq));
                ui.set_eq5_gain(PluginContextReadF32::get_param(state, P::Eq5Gain));
                ui.set_eq5_q(PluginContextReadF32::get_param(state, P::Eq5Q));

                ui.set_blend(PluginContextReadF32::get_param(state, P::Blend));
                ui.set_cf_angle(PluginContextReadF32::get_param(state, P::CfAngle));
                ui.set_cf_amount(PluginContextReadF32::get_param(state, P::CfAmount));
                ui.set_gain(PluginContextReadF32::get_param(state, P::Gain));

                ui.set_eq1_type(discrete_index(PluginContextReadF32::get_param(state, P::Eq1Type) as f64, 4) as i32);
                ui.set_eq2_type(discrete_index(PluginContextReadF32::get_param(state, P::Eq2Type) as f64, 4) as i32);
                ui.set_eq3_type(discrete_index(PluginContextReadF32::get_param(state, P::Eq3Type) as f64, 4) as i32);
                ui.set_eq4_type(discrete_index(PluginContextReadF32::get_param(state, P::Eq4Type) as f64, 4) as i32);
                ui.set_eq5_type(discrete_index(PluginContextReadF32::get_param(state, P::Eq5Type) as f64, 4) as i32);
                ui.set_cf_realism(discrete_index(PluginContextReadF32::get_param(state, P::CfRealism) as f64, 3) as i32);

                ui.set_bypass(PluginContextReadF32::get_param(state, P::Bypass) > 0.5);

                // Formatted value texts.
                ui.set_eq1_freq_text(slint::SharedString::from(state.format_param(P::Eq1Freq)));
                ui.set_eq1_gain_text(slint::SharedString::from(state.format_param(P::Eq1Gain)));
                ui.set_eq1_q_text(slint::SharedString::from(state.format_param(P::Eq1Q)));
                ui.set_eq2_freq_text(slint::SharedString::from(state.format_param(P::Eq2Freq)));
                ui.set_eq2_gain_text(slint::SharedString::from(state.format_param(P::Eq2Gain)));
                ui.set_eq2_q_text(slint::SharedString::from(state.format_param(P::Eq2Q)));
                ui.set_eq3_freq_text(slint::SharedString::from(state.format_param(P::Eq3Freq)));
                ui.set_eq3_gain_text(slint::SharedString::from(state.format_param(P::Eq3Gain)));
                ui.set_eq3_q_text(slint::SharedString::from(state.format_param(P::Eq3Q)));
                ui.set_eq4_freq_text(slint::SharedString::from(state.format_param(P::Eq4Freq)));
                ui.set_eq4_gain_text(slint::SharedString::from(state.format_param(P::Eq4Gain)));
                ui.set_eq4_q_text(slint::SharedString::from(state.format_param(P::Eq4Q)));
                ui.set_eq5_freq_text(slint::SharedString::from(state.format_param(P::Eq5Freq)));
                ui.set_eq5_gain_text(slint::SharedString::from(state.format_param(P::Eq5Gain)));
                ui.set_eq5_q_text(slint::SharedString::from(state.format_param(P::Eq5Q)));
                ui.set_blend_text(slint::SharedString::from(state.format_param(P::Blend)));
                ui.set_cf_angle_text(slint::SharedString::from(state.format_param(P::CfAngle)));
                ui.set_cf_amount_text(slint::SharedString::from(state.format_param(P::CfAmount)));
                ui.set_gain_text(slint::SharedString::from(state.format_param(P::Gain)));

                let peak_db = state.shared.input_peak.load(Ordering::Relaxed);
                ui.set_input_db_text(slint::SharedString::from(format!("{peak_db:.1} dB")));

                // Build the EQ curve path from the current parameters.
                let sr = shared_for_curve.sample_rate.load(Ordering::Relaxed).max(1.0);
                let cmds = eq_curve_path(&params_for_curve, sr);
                ui.set_curve_cmds(slint::SharedString::from(cmds));
            })
        },
    )
    .into_editor()
}

fn eq_curve_path(params: &AetherParams, sr: f32) -> String {
    let mut bands: [Biquad; NUM_BANDS] = std::array::from_fn(|_| Biquad::new());
    let band_vals = [
        (
            params.eq1_type.value_i32(),
            params.eq1_freq.raw_target() as f32,
            params.eq1_gain.raw_target() as f32,
            params.eq1_q.raw_target() as f32,
        ),
        (
            params.eq2_type.value_i32(),
            params.eq2_freq.raw_target() as f32,
            params.eq2_gain.raw_target() as f32,
            params.eq2_q.raw_target() as f32,
        ),
        (
            params.eq3_type.value_i32(),
            params.eq3_freq.raw_target() as f32,
            params.eq3_gain.raw_target() as f32,
            params.eq3_q.raw_target() as f32,
        ),
        (
            params.eq4_type.value_i32(),
            params.eq4_freq.raw_target() as f32,
            params.eq4_gain.raw_target() as f32,
            params.eq4_q.raw_target() as f32,
        ),
        (
            params.eq5_type.value_i32(),
            params.eq5_freq.raw_target() as f32,
            params.eq5_gain.raw_target() as f32,
            params.eq5_q.raw_target() as f32,
        ),
    ];
    for (i, &(t, f, g, q)) in band_vals.iter().enumerate() {
        set_band(&mut bands[i], t, f, g, q, sr);
    }

    const N: usize = 240;
    const W: f32 = 720.0;
    const H: f32 = 180.0;
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
        let db: f32 = bands.iter().map(|b| b.magnitude_db(freq, sr)).sum::<f32>().clamp(DB_MIN, DB_MAX);
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
