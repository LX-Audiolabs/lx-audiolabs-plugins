//! Equilibrium Slint editor — 5-band gain/width/pan + monitor.
//! truce-slint software renderer. Spectrum/preset polish later.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use slint::SharedString;
use truce::prelude::*;
use truce_core::editor::{Editor, PluginContextReadF32};
use truce_slint::{PluginContext, SlintEditor, SyncFn};

use crate::{EquilibriumParams, EquilibriumParamsParamId as P};

slint::include_modules!();

const WINDOW_W: u32 = 990;
const WINDOW_H: u32 = 660;
const VERSION: &str = env!("CARGO_PKG_VERSION");

macro_rules! bind_bool {
    ($ui:expr, $state:expr, $p:expr, $on:ident) => {{
        let s = $state.clone();
        $ui.$on(move |v: bool| s.automate($p, if v { 1.0 } else { 0.0 }));
    }};
}
macro_rules! bind_float {
    ($ui:expr, $state:expr, $p:expr, $on:ident) => {{
        let s = $state.clone();
        $ui.$on(move |v: f32| s.automate($p, v as f64));
    }};
}

fn format_pan(plain: f32) -> String {
    if plain.abs() < 0.01 {
        "C".into()
    } else if plain < 0.0 {
        format!("L {:.0}%", -plain * 100.0)
    } else {
        format!("R {:.0}%", plain * 100.0)
    }
}

pub fn build_editor(params: Arc<EquilibriumParams>) -> Box<dyn Editor> {
    let shared = params.shared.clone();
    SlintEditor::new(
        params.clone(),
        (WINDOW_W, WINDOW_H),
        move |state: PluginContext<EquilibriumParams>| -> SyncFn<EquilibriumParams> {
            let ui = match EquilibriumUi::new() {
                Ok(u) => u,
                Err(e) => {
                    tracing::error!("EquilibriumUi::new failed: {e:?}");
                    return Box::new(|_: &PluginContext<EquilibriumParams>| {});
                }
            };
            ui.set_version(SharedString::from(VERSION));

            bind_bool!(ui, state, P::MonoActive, on_mono_active_changed);
            bind_bool!(ui, state, P::DeltaActive, on_delta_active_changed);
            bind_bool!(ui, state, P::ListenActive, on_listen_active_changed);
            bind_bool!(ui, state, P::AutoGainActive, on_auto_gain_active_changed);
            bind_bool!(ui, state, P::BypassActive, on_bypass_active_changed);
            bind_bool!(ui, state, P::PreMasterActive, on_pre_master_active_changed);

            bind_float!(ui, state, P::OutputGain, on_output_gain_changed);
            bind_float!(ui, state, P::MonoFloor, on_mono_floor_changed);
            bind_float!(
                ui,
                state,
                P::PreMasterTargetDb,
                on_pre_master_target_db_changed
            );

            bind_float!(ui, state, P::LowGain, on_low_gain_changed);
            bind_float!(ui, state, P::BassGain, on_bass_gain_changed);
            bind_float!(ui, state, P::MidGain, on_mid_gain_changed);
            bind_float!(ui, state, P::HighMidGain, on_high_mid_gain_changed);
            bind_float!(ui, state, P::HighGain, on_high_gain_changed);

            bind_float!(ui, state, P::LowWidth, on_low_width_changed);
            bind_float!(ui, state, P::BassWidth, on_bass_width_changed);
            bind_float!(ui, state, P::MidWidth, on_mid_width_changed);
            bind_float!(ui, state, P::HighMidWidth, on_high_mid_width_changed);
            bind_float!(ui, state, P::HighWidth, on_high_width_changed);

            bind_float!(ui, state, P::LowPan, on_low_pan_changed);
            bind_float!(ui, state, P::BassPan, on_bass_pan_changed);
            bind_float!(ui, state, P::MidPan, on_mid_pan_changed);
            bind_float!(ui, state, P::HighMidPan, on_high_mid_pan_changed);
            bind_float!(ui, state, P::HighPan, on_high_pan_changed);

            bind_bool!(ui, state, P::SoloLow, on_solo_low_changed);
            bind_bool!(ui, state, P::SoloBass, on_solo_bass_changed);
            bind_bool!(ui, state, P::SoloMid, on_solo_mid_changed);
            bind_bool!(ui, state, P::SoloHighMid, on_solo_high_mid_changed);
            bind_bool!(ui, state, P::SoloHigh, on_solo_high_changed);

            let shared_sync = shared.clone();
            Box::new(move |state: &PluginContext<EquilibriumParams>| {
                let g = |p: P| PluginContextReadF32::get_param(state, p);
                let p = state.params();

                ui.set_mono_active(g(P::MonoActive) > 0.5);
                ui.set_delta_active(g(P::DeltaActive) > 0.5);
                ui.set_listen_active(g(P::ListenActive) > 0.5);
                ui.set_auto_gain_active(g(P::AutoGainActive) > 0.5);
                ui.set_bypass_active(g(P::BypassActive) > 0.5);
                ui.set_pre_master_active(g(P::PreMasterActive) > 0.5);

                ui.set_output_gain(g(P::OutputGain));
                ui.set_mono_floor(g(P::MonoFloor));
                ui.set_pre_master_target_db(g(P::PreMasterTargetDb));

                ui.set_low_gain(g(P::LowGain));
                ui.set_bass_gain(g(P::BassGain));
                ui.set_mid_gain(g(P::MidGain));
                ui.set_high_mid_gain(g(P::HighMidGain));
                ui.set_high_gain(g(P::HighGain));

                ui.set_low_width(g(P::LowWidth));
                ui.set_bass_width(g(P::BassWidth));
                ui.set_mid_width(g(P::MidWidth));
                ui.set_high_mid_width(g(P::HighMidWidth));
                ui.set_high_width(g(P::HighWidth));

                ui.set_low_pan(g(P::LowPan));
                ui.set_bass_pan(g(P::BassPan));
                ui.set_mid_pan(g(P::MidPan));
                ui.set_high_mid_pan(g(P::HighMidPan));
                ui.set_high_pan(g(P::HighPan));

                ui.set_solo_low(g(P::SoloLow) > 0.5);
                ui.set_solo_bass(g(P::SoloBass) > 0.5);
                ui.set_solo_mid(g(P::SoloMid) > 0.5);
                ui.set_solo_high_mid(g(P::SoloHighMid) > 0.5);
                ui.set_solo_high(g(P::SoloHigh) > 0.5);

                ui.set_output_gain_text(SharedString::from(format!(
                    "{:.1} dB",
                    p.output_gain.raw_target()
                )));
                let mf = p.mono_floor.raw_target() as f32;
                ui.set_mono_floor_text(SharedString::from(if mf < 0.5 {
                    "off".into()
                } else {
                    format!("{mf:.0} Hz")
                }));
                ui.set_pre_master_target_db_text(SharedString::from(format!(
                    "{:.0}",
                    p.pre_master_target_db.raw_target()
                )));

                ui.set_low_gain_text(SharedString::from(format!(
                    "{:.1} dB",
                    p.low_gain.raw_target()
                )));
                ui.set_bass_gain_text(SharedString::from(format!(
                    "{:.1} dB",
                    p.bass_gain.raw_target()
                )));
                ui.set_mid_gain_text(SharedString::from(format!(
                    "{:.1} dB",
                    p.mid_gain.raw_target()
                )));
                ui.set_high_mid_gain_text(SharedString::from(format!(
                    "{:.1} dB",
                    p.high_mid_gain.raw_target()
                )));
                ui.set_high_gain_text(SharedString::from(format!(
                    "{:.1} dB",
                    p.high_gain.raw_target()
                )));

                ui.set_low_width_text(SharedString::from(format!(
                    "{:.0}%",
                    p.low_width.raw_target()
                )));
                ui.set_bass_width_text(SharedString::from(format!(
                    "{:.0}%",
                    p.bass_width.raw_target()
                )));
                ui.set_mid_width_text(SharedString::from(format!(
                    "{:.0}%",
                    p.mid_width.raw_target()
                )));
                ui.set_high_mid_width_text(SharedString::from(format!(
                    "{:.0}%",
                    p.high_mid_width.raw_target()
                )));
                ui.set_high_width_text(SharedString::from(format!(
                    "{:.0}%",
                    p.high_width.raw_target()
                )));

                ui.set_low_pan_text(SharedString::from(
                    format_pan(p.low_pan.raw_target() as f32),
                ));
                ui.set_bass_pan_text(SharedString::from(format_pan(
                    p.bass_pan.raw_target() as f32
                )));
                ui.set_mid_pan_text(SharedString::from(
                    format_pan(p.mid_pan.raw_target() as f32),
                ));
                ui.set_high_mid_pan_text(SharedString::from(format_pan(
                    p.high_mid_pan.raw_target() as f32,
                )));
                ui.set_high_pan_text(SharedString::from(format_pan(
                    p.high_pan.raw_target() as f32
                )));

                let peak = shared_sync.input_peak.load(Ordering::Relaxed);
                ui.set_meter_line(SharedString::from(if peak <= -90.0 {
                    "in: —".to_string()
                } else {
                    format!("in: {peak:.1} dB")
                }));
                // Gonio path filled later when scope buffer wiring is ready
                ui.set_gonio_cmds(SharedString::from(""));
            })
        },
    )
    .resizable(false)
    .into_editor()
}
