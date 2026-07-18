//! Lucent Slint editor — spectrum + analyze controls.
//! truce-slint software renderer.

use std::sync::Arc;

use slint::SharedString;
use truce::prelude::*;
use truce_core::cast::{discrete_index, discrete_norm};
use truce_core::editor::{Editor, PluginContextReadF32};
use truce_slint::{PluginContext, SlintEditor, SyncFn};

use crate::{editor_ensure_consumer, LucentParams, LucentParamsParamId as P};
use lx_analysis::SPECTRUM_BINS;

slint::include_modules!();

const WINDOW_W: u32 = 990;
const WINDOW_H: u32 = 550;
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn spectrum_path(shared: &lx_analysis::SharedState) -> String {
    let bins = SPECTRUM_BINS;
    let w = 900.0f32;
    let h = 360.0f32;
    let avg = shared.spectrum_avg.lock().ok();
    let mut s = String::with_capacity(bins * 12);
    for i in 0..bins {
        let db = avg
            .as_ref()
            .and_then(|v| v.get(i).copied())
            .unwrap_or(-90.0)
            .clamp(-90.0, 0.0);
        let x = (i as f32 / (bins.saturating_sub(1).max(1) as f32)) * w;
        let y = ((-db) / 90.0) * h;
        if i == 0 {
            s.push_str(&format!("M {x:.1} {y:.1}"));
        } else {
            s.push_str(&format!(" L {x:.1} {y:.1}"));
        }
    }
    s
}

pub fn build_editor(params: Arc<LucentParams>) -> Box<dyn Editor> {
    let shared = params.shared.clone();
    SlintEditor::new(
        params.clone(),
        (WINDOW_W, WINDOW_H),
        move |state: PluginContext<LucentParams>| -> SyncFn<LucentParams> {
            let ui = match LucentUi::new() {
                Ok(u) => u,
                Err(e) => {
                    tracing::error!("LucentUi::new failed: {e:?}");
                    return Box::new(|_: &PluginContext<LucentParams>| {});
                }
            };

            ui.set_version(SharedString::from(VERSION));
            let name0 = params.name.read().map(|s| s.clone()).unwrap_or_default();
            ui.set_display_name(SharedString::from(name0.as_str()));

            let p = params.clone();
            ui.on_display_name_changed(move |txt: SharedString| {
                let s = txt.as_str().to_string();
                if let Ok(mut n) = p.name.write() {
                    *n = s.clone();
                }
                if let Ok(mut bg) = p.name_bg.write() {
                    *bg = s;
                }
            });

            let s = state.clone();
            ui.on_analyze_mode_changed(move |v: f32| {
                s.automate(P::AnalyzeMode, discrete_norm(v.max(0.0) as usize, 3));
            });
            let s = state.clone();
            ui.on_resonance_active_changed(move |v: bool| {
                s.automate(P::ResonanceActive, if v { 1.0 } else { 0.0 });
            });
            let s = state.clone();
            ui.on_masking_active_changed(move |v: bool| {
                s.automate(P::MaskingActive, if v { 1.0 } else { 0.0 });
            });
            let s = state.clone();
            ui.on_bypass_active_changed(move |v: bool| {
                s.automate(P::BypassActive, if v { 1.0 } else { 0.0 });
            });
            let s = state.clone();
            ui.on_sensitivity_changed(move |v: f32| {
                s.automate(P::Sensitivity, v as f64);
            });

            let shared_sync = shared.clone();
            let params_sync = params.clone();
            Box::new(move |state: &PluginContext<LucentParams>| {
                editor_ensure_consumer(&params_sync, &shared_sync);

                ui.set_analyze_mode(discrete_index(
                    PluginContextReadF32::get_param(state, P::AnalyzeMode) as f64,
                    3,
                ) as f32);
                ui.set_resonance_active(
                    PluginContextReadF32::get_param(state, P::ResonanceActive) > 0.5,
                );
                ui.set_masking_active(
                    PluginContextReadF32::get_param(state, P::MaskingActive) > 0.5,
                );
                ui.set_bypass_active(PluginContextReadF32::get_param(state, P::BypassActive) > 0.5);
                let sens = PluginContextReadF32::get_param(state, P::Sensitivity);
                ui.set_sensitivity(sens);
                let plain = state.params().sensitivity.raw_target() as f32;
                ui.set_sensitivity_text(SharedString::from(format!("{plain:.0}%")));

                ui.set_spectrum_cmds(SharedString::from(spectrum_path(&shared_sync)));
                let mode = discrete_index(
                    PluginContextReadF32::get_param(state, P::AnalyzeMode) as f64,
                    3,
                );
                let mode_s = ["Own", "Relays", "Both"].get(mode).copied().unwrap_or("?");
                ui.set_status_line(SharedString::from(format!("analyze: {mode_s}")));
            })
        },
    )
    .resizable(false)
    .into_editor()
}
