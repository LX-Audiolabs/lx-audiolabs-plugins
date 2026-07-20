//! Lucent Slint UI — analyzer layout, not a Vizia port.
//! truce-slint software renderer.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use slint::SharedString;
use truce::prelude::*;
use truce_core::cast::{discrete_index, discrete_norm};
use truce_core::editor::{Editor, PluginContextReadF32};
use truce_slint::{PluginContext, SlintEditor, SyncFn};

use crate::{
    editor_ensure_consumer, read_masking, read_resonance, LucentParams, LucentParamsParamId as P,
};
use lx_analysis::{relay_hub, SPECTRUM_BINS};

slint::include_modules!();

const WINDOW_W: u32 = 990;
const WINDOW_H: u32 = 550;
const VERSION: &str = env!("CARGO_PKG_VERSION");
const PATH_W: f32 = 900.0;
const PATH_H: f32 = 400.0;

fn db_to_y(db: f32) -> f32 {
    let t = ((-db.clamp(-90.0, 0.0)) / 90.0).clamp(0.0, 1.0);
    t * PATH_H
}

fn spectrum_fill_path(bins: &[f32]) -> String {
    let n = bins.len().max(1);
    let mut s = String::with_capacity(n * 14 + 40);
    s.push_str(&format!("M 0 {PATH_H:.0}"));
    for (i, &db) in bins.iter().enumerate() {
        let x = (i as f32 / (n.saturating_sub(1).max(1) as f32)) * PATH_W;
        let y = db_to_y(db);
        s.push_str(&format!(" L {x:.1} {y:.1}"));
    }
    s.push_str(&format!(" L {PATH_W:.0} {PATH_H:.0} Z"));
    s
}

fn mask_bars_path(mask: &[(usize, f32, Vec<String>)]) -> String {
    if mask.is_empty() {
        return String::new();
    }
    let mut s = String::new();
    let n = SPECTRUM_BINS.max(1) as f32;
    for (bin, db, _) in mask.iter().take(24) {
        let x = (*bin as f32 / n) * PATH_W;
        let y0 = db_to_y(*db);
        let y1 = PATH_H;
        let w = (PATH_W / n).max(1.5);
        // thin rectangle as path
        s.push_str(&format!(
            "M {x:.1} {y0:.1} L {xr:.1} {y0:.1} L {xr:.1} {y1:.0} L {x:.1} {y1:.0} Z ",
            xr = x + w
        ));
    }
    s
}

/// Map peak dB → 0..1 over −60..+6 dB (LxLedPeakMeter / LxStereoMeter range).
fn peak_norm(db: f32) -> f32 {
    ((db + 60.0) / 66.0).clamp(0.0, 1.0)
}

pub fn build_editor(params: Arc<LucentParams>) -> Box<dyn Editor> {
    let shared = params.shared.clone();
    let instance_key = Arc::as_ptr(&params) as usize;

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

                let mode = discrete_index(
                    PluginContextReadF32::get_param(state, P::AnalyzeMode) as f64,
                    3,
                );
                ui.set_analyze_mode(mode as f32);
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

                // Spectrum
                let bins = shared_sync
                    .spectrum_avg
                    .lock()
                    .ok()
                    .map(|g| g.clone())
                    .unwrap_or_else(|| vec![-90.0; SPECTRUM_BINS]);
                ui.set_spectrum_cmds(SharedString::from(spectrum_fill_path(&bins)));

                let mask = read_masking(instance_key);
                ui.set_mask_cmds(SharedString::from(mask_bars_path(&mask)));

                let res = read_resonance(instance_key);
                let res_line = if res.own.is_empty() && res.relay.is_empty() {
                    "No peaks".into()
                } else {
                    format!(
                        "own {} · group {}",
                        res.own.len().min(99),
                        res.relay.len().min(99)
                    )
                };
                ui.set_resonance_line(SharedString::from(res_line));

                let mask_line = if mask.is_empty() {
                    "No collisions".into()
                } else {
                    let top = mask
                        .first()
                        .map(|(b, db, names)| {
                            format!("bin {b}  {db:.0} dB  {}", names.join("+"))
                        })
                        .unwrap_or_default();
                    format!("{} hits · {top}", mask.len())
                };
                ui.set_masking_line(SharedString::from(mask_line));

                let peak = shared_sync.input_peak.load(Ordering::Relaxed);
                let pl = shared_sync.output_peak_l.load(Ordering::Relaxed);
                let pr = shared_sync.output_peak_r.load(Ordering::Relaxed);
                // Lucent may only fill input_peak — fall back
                let pl = if pl <= -90.0 { peak } else { pl };
                let pr = if pr <= -90.0 { peak } else { pr };
                ui.set_peak_l(peak_norm(pl));
                ui.set_peak_r(peak_norm(pr));
                ui.set_peak_text(SharedString::from(if peak <= -90.0 {
                    "—".into()
                } else {
                    format!("{peak:.1} dB")
                }));

                let now = lx_analysis::shm::now_ms();
                let n_relays = relay_hub()
                    .map(|h| h.read_consumers(now).len())
                    .unwrap_or(0);
                ui.set_relay_count(n_relays as i32);

                let mode_s = ["Own bus", "Relays", "Own + Relays"]
                    .get(mode)
                    .copied()
                    .unwrap_or("?");
                ui.set_status_line(SharedString::from(mode_s));
            })
        },
    )
    .resizable(false)
    .into_editor()
}
