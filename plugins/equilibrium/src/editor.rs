//! Equilibrium Slint editor — aura-editor, Vizia feature parity (vault/SNAP/telemetry).

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use aura::prelude::*;
use aura_editor::typed::*;
use aura_editor::ui_zoom::{apply_ui_zoom, UiZoom};
use lx_editor_utils::{bind_bools, bind_floats, dirty::*, meter::*, slint_helpers::*, sync_bools_dirty, sync_floats_dirty, tick::*, viz::*};
use slint::SharedString;

use crate::presets::{
    apply_stereo_from_preset, load_presets, param_norm, pink_noise_preset, preset_names,
    preset_save_dir, profile_for_save, snap_filename, snap_markdown, PresetEntry,
};
use crate::EquilibriumParams;
use crate::EquilibriumParamsParamId as P;

slint::include_modules!();

const WINDOW_W: u32 = 990;
const WINDOW_H: u32 = 670;
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn format_pan(plain: f32) -> String {
    if plain.abs() < 0.01 {
        "C".into()
    } else if plain < 0.0 {
        format!("L {:.0}%", -plain * 100.0)
    } else {
        format!("R {:.0}%", plain * 100.0)
    }
}

/// Mean-normalized bar height 0..1 (Vizia canvas window −20…+22 around avg).
fn band_bar_norm(levels: &[f32; 5], i: usize) -> f32 {
    let avg: f32 = levels.iter().map(|v| v.max(-50.0)).sum::<f32>() / 5.0;
    band_bar_norm_avg(levels, avg, i, -50.0)
}

fn band_bar_norm_avg(levels: &[f32; 5], avg: f32, i: usize, floor: f32) -> f32 {
    if avg <= -70.0 {
        return 0.0;
    }
    let rel = (levels[i].max(floor) - avg).clamp(-20.0, 22.0);
    ((rel + 20.0) / 42.0).clamp(0.0, 1.0)
}

fn target_bar_norm(targets: &[f32; 5], i: usize) -> f32 {
    let avg: f32 = targets.iter().map(|v| v.max(-30.0)).sum::<f32>() / 5.0;
    band_bar_norm_avg(targets, avg, i, -30.0)
}

/// Tolerance half-height in the same −20…+22 (42 dB) display window.
fn tol_bar_norm(tol_db: f32) -> f32 {
    (tol_db / 42.0).clamp(0.0, 1.0)
}

struct SyncCache {
    tick: TickCache,
    floats: [f32; 18],
    bools: [Option<bool>; 11],
    texts: [String; 18],
    meter_l: f32,
    meter_r: f32,
    peak_hold_l: f32,
    peak_hold_r: f32,
    corr: f32,
    balance: f32,
    hold_l_q: f32,
    hold_r_q: f32,
    bands: [f32; 5],
    tgts: [f32; 5],
    tgt_tols: [f32; 5],
    lis: [f32; 5],
    lis_tols: [f32; 5],
    lis_lo: [f32; 5],
    lis_hi: [f32; 5],
    listening: Option<bool>,
    listen_ready: Option<bool>,
    auto_loud: Option<bool>,
    snap_was_active: bool,
    snap_blink: u32,
    snap_label: String,
    /// Persistent goniometer display window — see `gonio_path`.
    gonio_window: Vec<[f32; 2]>,
}

impl SyncCache {
    fn new() -> Self {
        Self {
            tick: TickCache::new(),
            floats: [f32::NAN; 18],
            bools: [None; 11],
            texts: std::array::from_fn(|_| String::new()),
            meter_l: f32::NAN,
            meter_r: f32::NAN,
            peak_hold_l: f32::NAN,
            peak_hold_r: f32::NAN,
            corr: f32::NAN,
            balance: f32::NAN,
            hold_l_q: f32::NAN,
            hold_r_q: f32::NAN,
            bands: [f32::NAN; 5],
            tgts: [f32::NAN; 5],
            tgt_tols: [f32::NAN; 5],
            lis: [f32::NAN; 5],
            lis_tols: [f32::NAN; 5],
            lis_lo: [f32::NAN; 5],
            lis_hi: [f32::NAN; 5],
            listening: None,
            listen_ready: None,
            auto_loud: None,
            snap_was_active: false,
            snap_blink: 0,
            snap_label: String::new(),
            gonio_window: Vec::new(),
        }
    }

    fn due(&mut self) -> bool {
        self.tick.due()
    }
}

struct VaultUiState {
    vault_path: Option<String>,
    last_preset: Option<String>,
    presets: Vec<PresetEntry>,
}

pub fn build_editor(params: Arc<EquilibriumParams>) -> Box<dyn Editor> {
    let shared = params.shared.clone();
    let sync_cache = Arc::new(Mutex::new(SyncCache::new()));

    let init_cfg = lx_vault::load_config("Equilibrium");
    let init_vp = init_cfg.vault_path.clone();
    let init_last = init_cfg.last_preset.clone();
    let init_presets = load_presets(init_vp.as_deref());
    // Seed targets once: restore last preset if known, else Pink Noise default.
    {
        let pink = pink_noise_preset();
        let seed = init_last
            .as_ref()
            .and_then(|n| {
                init_presets
                    .iter()
                    .find(|(name, _, _)| name == n)
                    .map(|(_, _, p)| p.clone())
            })
            .unwrap_or(pink);
        for b in 0..5 {
            shared.target_levels[b].store(seed.bands[b], Ordering::Release);
            shared.target_tolerances[b].store(seed.tolerances[b], Ordering::Release);
        }
        let idx = init_last
            .as_ref()
            .and_then(|n| init_presets.iter().position(|(name, _, _)| name == n))
            .unwrap_or(0);
        shared.selected_preset_index.store(idx, Ordering::Release);
    }

    let vault_state = Arc::new(Mutex::new(VaultUiState {
        vault_path: init_vp.clone(),
        last_preset: init_last,
        presets: init_presets,
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
            let sync_cache = Arc::clone(&sync_cache);
            move |state: LxPluginContext<EquilibriumParams>| {
                // New Slint component each open; wipe dirty mirrors so labels
                // and layout are re-pushed (stale cache → empty text / collapse).
                if let Ok(mut c) = sync_cache.lock() {
                    *c = SyncCache::new();
                }
                let ui = EquilibriumUi::new().expect("EquilibriumUi::new");
                ui.set_version(SharedString::from(VERSION));

                ui.set_ui_zoom_percent(zoom_build.percent() as i32);
                {
                    let z = zoom_build.clone();
                    let s = state.clone();
                    ui.on_ui_zoom_changed(move |p| {
                        apply_ui_zoom(&z, |w, h| s.request_resize(w, h), p as u32);
                    });
                }

                // Live vault_state — path/name survive UI close.
                let (live_vp, live_last, names) = vault_state
                    .lock()
                    .ok()
                    .map(|g| {
                        (
                            g.vault_path.clone(),
                            g.last_preset.clone(),
                            preset_names(&g.presets),
                        )
                    })
                    .unwrap_or_else(|| (None, None, vec!["Pink Noise".into()]));
                ui.set_vault_path(SharedString::from(live_vp.as_deref().unwrap_or("")));
                if live_vp.as_ref().is_none_or(|v| v.is_empty()) {
                    ui.set_snap_label(SharedString::from("SET VAULT"));
                }
                ui.set_preset_names(names_model(&names));
                if let Some(ref name) = live_last {
                    if !name.is_empty() {
                        ui.set_preset_name(SharedString::from(name.as_str()));
                    }
                }

                bind_floats!(ui, state,
                    P::LowGain => low_gain,
                    P::BassGain => bass_gain,
                    P::MidGain => mid_gain,
                    P::HighMidGain => high_mid_gain,
                    P::HighGain => high_gain,
                    P::LowWidth => low_width,
                    P::BassWidth => bass_width,
                    P::MidWidth => mid_width,
                    P::HighMidWidth => high_mid_width,
                    P::HighWidth => high_width,
                    P::LowPan => low_pan,
                    P::BassPan => bass_pan,
                    P::MidPan => mid_pan,
                    P::HighMidPan => high_mid_pan,
                    P::HighPan => high_pan,
                    P::OutputGain => output_gain,
                    P::MonoFloor => mono_floor,
                    P::PreMasterTargetDb => pre_master_target_db,
                );

                bind_bools!(ui, state,
                    P::MonoActive => mono_active,
                    P::DeltaActive => delta_active,
                    P::ListenActive => listen_active,
                    P::AutoGainActive => auto_gain_active,
                    P::BypassActive => bypass_active,
                    P::PreMasterActive => pre_master_active,
                    P::SoloLow => solo_low,
                    P::SoloBass => solo_bass,
                    P::SoloMid => solo_mid,
                    P::SoloHighMid => solo_high_mid,
                    P::SoloHigh => solo_high,
                );

                // SNAP
                {
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
                                ui.set_vault_setup_path(SharedString::new());
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
                    });
                }

                // SAVE — target profile bands, not gain knobs
                {
                    let params_save = params.clone();
                    let shared_save = shared.clone();
                    let vs_save = vault_state.clone();
                    let ui_weak = ui.as_weak();
                    ui.on_save_clicked(move || {
                        let Some(ui) = ui_weak.upgrade() else { return };
                        let name_input = ui.get_preset_name().to_string();
                        let mut vs = match vs_save.lock() {
                            Ok(g) => g,
                            Err(_) => return,
                        };
                        let name = if name_input.trim().is_empty() {
                            format!("User Preset {}", vs.presets.len())
                        } else {
                            name_input.trim().to_string()
                        };
                        let mut bands = [0.0f32; 5];
                        let mut tols = [0.0f32; 5];
                        for b in 0..5 {
                            bands[b] = shared_save.target_levels[b].load(Ordering::Acquire);
                            tols[b] = shared_save.target_tolerances[b].load(Ordering::Acquire);
                        }
                        let prof = profile_for_save(&name, bands, tols, &params_save);
                        let dir = preset_save_dir(&vs.vault_path);
                        let _ = std::fs::create_dir_all(&dir);
                        let safe = name.replace(
                            |c: char| !c.is_alphanumeric() && c != ' ' && c != '-' && c != '_',
                            "",
                        );
                        let fp = dir.join(format!("{safe}.md"));
                        let md = crate::presets::export_preset_to_markdown(&prof);
                        if std::fs::write(&fp, md).is_ok() {
                            vs.presets = load_presets(vs.vault_path.as_deref());
                            vs.last_preset = Some(name.clone());
                            let _ = lx_vault::set_last_preset("Equilibrium", &name);
                            ui.set_preset_names(names_model(&preset_names(&vs.presets)));
                            ui.set_preset_name(SharedString::from(name.as_str()));
                            tracing::info!("SAVE Equilibrium preset {}", fp.display());
                        }
                    });
                }

                // vault path
                {
                    let vs_path = vault_state.clone();
                    let ui_path = ui.as_weak();
                    ui.on_vault_path_changed(move |path: SharedString| {
                        let path = path.to_string().trim().to_string();
                        let new_vp = if path.is_empty() { None } else { Some(path) };
                        if let Ok(mut vs) = vs_path.lock() {
                            vs.vault_path = new_vp.clone();
                            let _ = lx_vault::set_vault_path("Equilibrium", new_vp.clone());
                            vs.presets = load_presets(new_vp.as_deref());
                            if let Some(ui) = ui_path.upgrade() {
                                ui.set_preset_names(names_model(&preset_names(&vs.presets)));
                                if new_vp.as_ref().is_none_or(|v| v.is_empty()) {
                                    ui.set_snap_label(SharedString::from("SET VAULT"));
                                } else {
                                    ui.set_snap_label(SharedString::from("SNAP"));
                                }
                            }
                        }
                    });
                }

                // PASTE → draft path
                {
                    let paste_ui = ui.as_weak();
                    ui.on_vault_paste_requested(move || {
                        let Some(ui) = paste_ui.upgrade() else { return };
                        match aura_editor::platform::clipboard_get_retry() {
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
                }

                // preset select
                {
                    let sel_state = state.clone();
                    let sel_shared = shared.clone();
                    let sel_vs = vault_state.clone();
                    let sel_ui = ui.as_weak();
                    ui.on_preset_selected(move |name: SharedString| {
                        let name = name.to_string();
                        let profile = {
                            let vs = sel_vs.lock().ok();
                            vs.and_then(|g| {
                                g.presets
                                    .iter()
                                    .find(|(n, _, _)| n == &name)
                                    .map(|(_, _, p)| p.clone())
                            })
                        };
                        if let Some(prof) = profile {
                            for b in 0..5 {
                                sel_shared.target_levels[b]
                                    .store(prof.bands[b], Ordering::Release);
                                sel_shared.target_tolerances[b]
                                    .store(prof.tolerances[b], Ordering::Release);
                            }
                            apply_stereo_from_preset(&sel_state, &prof);
                            if let Ok(mut vs) = sel_vs.lock() {
                                vs.last_preset = Some(prof.name.clone());
                            }
                            let _ = lx_vault::set_last_preset("Equilibrium", &prof.name);
                            if let Some(ui) = sel_ui.upgrade() {
                                ui.set_preset_name(SharedString::from(prof.name.as_str()));
                            }
                        }
                    });
                }

                // RESET
                {
                    let reset_state = state.clone();
                    let reset_params = params.clone();
                    let reset_shared = shared.clone();
                    let reset_vs = vault_state.clone();
                    ui.on_reset_clicked(move || {
                        for (id, plain) in [
                            (P::LowGain, 0.0),
                            (P::BassGain, 0.0),
                            (P::MidGain, 0.0),
                            (P::HighMidGain, 0.0),
                            (P::HighGain, 0.0),
                            (P::LowWidth, 100.0),
                            (P::BassWidth, 100.0),
                            (P::MidWidth, 100.0),
                            (P::HighMidWidth, 100.0),
                            (P::HighWidth, 100.0),
                            (P::LowPan, 0.0),
                            (P::BassPan, 0.0),
                            (P::MidPan, 0.0),
                            (P::HighMidPan, 0.0),
                            (P::HighPan, 0.0),
                            (P::OutputGain, 0.0),
                            (P::MonoFloor, 0.0),
                            (P::PreMasterTargetDb, -3.0),
                        ] {
                            reset_state.automate(id, param_norm(id, plain));
                        }
                        for id in [
                            P::MonoActive,
                            P::DeltaActive,
                            P::BypassActive,
                            P::PreMasterActive,
                            P::ListenActive,
                            P::AutoGainActive,
                            P::SoloLow,
                            P::SoloBass,
                            P::SoloMid,
                            P::SoloHighMid,
                            P::SoloHigh,
                        ] {
                            reset_state.automate(id, 0.0);
                        }
                        let pink = pink_noise_preset();
                        for b in 0..5 {
                            reset_shared.target_levels[b].store(pink.bands[b], Ordering::Release);
                            reset_shared.target_tolerances[b]
                                .store(pink.tolerances[b], Ordering::Release);
                        }
                        reset_shared.selected_preset_index.store(0, Ordering::Release);
                        reset_shared.auto_loud.gain_offset.store(0.0, Ordering::Release);
                        reset_shared.snap.reset_analysis.store(true, Ordering::Release);
                        let _ = (&reset_params, &reset_vs);
                        tracing::info!("RESET clicked");
                    });
                }

                // peaks
                {
                    let s = shared.clone();
                    ui.on_reset_peaks(move || {
                        s.peaks.reset_peak.store(true, Ordering::Release);
                    });
                }

                // auto loud (blocked while PRE-MASTER owns loudness — Vizia parity)
                {
                    let s = shared.clone();
                    let st = state.clone();
                    ui.on_auto_loud_clicked(move || {
                        if PluginContextReadF32::get_param(&st, P::PreMasterActive) > 0.5 {
                            return;
                        }
                        if s.auto_loud.measuring.load(Ordering::Acquire) {
                            return;
                        }
                        s.auto_loud.trigger.store(true, Ordering::Release);
                    });
                }

                // apply / reset analysis
                {
                    let s = shared.clone();
                    ui.on_apply_analysis_clicked(move || {
                        if s.listen_samples.load(Ordering::Acquire) <= 100.0 {
                            return;
                        }
                        for b in 0..5 {
                            let lv = s.listen_levels[b].load(Ordering::Acquire);
                            let tol = s.listen_tolerances[b].load(Ordering::Acquire);
                            s.target_levels[b].store(lv, Ordering::Release);
                            s.target_tolerances[b].store(tol, Ordering::Release);
                        }
                    });
                }
                {
                    let s = shared.clone();
                    ui.on_reset_analysis_clicked(move || {
                        s.snap.reset_analysis.store(true, Ordering::Release);
                        s.listen_samples.store(0.0, Ordering::Release);
                        for b in 0..5 {
                            s.listen_levels[b].store(-90.0, Ordering::Release);
                            s.listen_tolerances[b].store(0.0, Ordering::Release);
                        }
                    });
                }

                ui
            }
        },
        {
            let shared = shared.clone();
            let vault_state = vault_state.clone();
            move |ui: &EquilibriumUi, state: &LxPluginContext<EquilibriumParams>| {
                let Ok(mut cache) = sync_cache.lock() else {
                    return;
                };
                if !cache.due() {
                    return;
                }

                sync_floats_dirty!(ui, state, cache,
                    0, P::LowGain => low_gain,
                    1, P::BassGain => bass_gain,
                    2, P::MidGain => mid_gain,
                    3, P::HighMidGain => high_mid_gain,
                    4, P::HighGain => high_gain,
                    5, P::LowWidth => low_width,
                    6, P::BassWidth => bass_width,
                    7, P::MidWidth => mid_width,
                    8, P::HighMidWidth => high_mid_width,
                    9, P::HighWidth => high_width,
                    10, P::LowPan => low_pan,
                    11, P::BassPan => bass_pan,
                    12, P::MidPan => mid_pan,
                    13, P::HighMidPan => high_mid_pan,
                    14, P::HighPan => high_pan,
                    15, P::OutputGain => output_gain,
                    16, P::MonoFloor => mono_floor,
                    17, P::PreMasterTargetDb => pre_master_target_db,
                );

                sync_bools_dirty!(ui, state, cache,
                    0, P::MonoActive => mono_active,
                    1, P::DeltaActive => delta_active,
                    2, P::ListenActive => listen_active,
                    3, P::AutoGainActive => auto_gain_active,
                    4, P::BypassActive => bypass_active,
                    5, P::PreMasterActive => pre_master_active,
                    6, P::SoloLow => solo_low,
                    7, P::SoloBass => solo_bass,
                    8, P::SoloMid => solo_mid,
                    9, P::SoloHighMid => solo_high_mid,
                    10, P::SoloHigh => solo_high,
                );

                let p = &state.params;
                let set_txt = |ui: &EquilibriumUi, cache: &mut SyncCache, i: usize, s: String| {
                    if changed_str(&mut cache.texts[i], &s) {
                        let ss = SharedString::from(s.as_str());
                        match i {
                            0 => ui.set_low_gain_text(ss),
                            1 => ui.set_bass_gain_text(ss),
                            2 => ui.set_mid_gain_text(ss),
                            3 => ui.set_high_mid_gain_text(ss),
                            4 => ui.set_high_gain_text(ss),
                            5 => ui.set_low_width_text(ss),
                            6 => ui.set_bass_width_text(ss),
                            7 => ui.set_mid_width_text(ss),
                            8 => ui.set_high_mid_width_text(ss),
                            9 => ui.set_high_width_text(ss),
                            10 => ui.set_low_pan_text(ss),
                            11 => ui.set_bass_pan_text(ss),
                            12 => ui.set_mid_pan_text(ss),
                            13 => ui.set_high_mid_pan_text(ss),
                            14 => ui.set_high_pan_text(ss),
                            15 => ui.set_output_gain_text(ss),
                            16 => ui.set_mono_floor_text(ss),
                            17 => ui.set_pre_master_target_db_text(ss),
                            _ => {}
                        }
                    }
                };
                set_txt(ui, &mut cache, 0, format!("{:.1} dB", p.low_gain.raw_target()));
                set_txt(ui, &mut cache, 1, format!("{:.1} dB", p.bass_gain.raw_target()));
                set_txt(ui, &mut cache, 2, format!("{:.1} dB", p.mid_gain.raw_target()));
                set_txt(ui, &mut cache, 3, format!("{:.1} dB", p.high_mid_gain.raw_target()));
                set_txt(ui, &mut cache, 4, format!("{:.1} dB", p.high_gain.raw_target()));
                set_txt(ui, &mut cache, 5, format!("{:.0}%", p.low_width.raw_target()));
                set_txt(ui, &mut cache, 6, format!("{:.0}%", p.bass_width.raw_target()));
                set_txt(ui, &mut cache, 7, format!("{:.0}%", p.mid_width.raw_target()));
                set_txt(ui, &mut cache, 8, format!("{:.0}%", p.high_mid_width.raw_target()));
                set_txt(ui, &mut cache, 9, format!("{:.0}%", p.high_width.raw_target()));
                set_txt(ui, &mut cache, 10, format_pan(p.low_pan.raw_target() as f32));
                set_txt(ui, &mut cache, 11, format_pan(p.bass_pan.raw_target() as f32));
                set_txt(ui, &mut cache, 12, format_pan(p.mid_pan.raw_target() as f32));
                set_txt(ui, &mut cache, 13, format_pan(p.high_mid_pan.raw_target() as f32));
                set_txt(ui, &mut cache, 14, format_pan(p.high_pan.raw_target() as f32));
                set_txt(ui, &mut cache, 15, format!("{:.1} dB", p.output_gain.raw_target()));
                let mf = p.mono_floor.raw_target() as f32;
                set_txt(
                    ui,
                    &mut cache,
                    16,
                    if mf < 0.5 {
                        "off".into()
                    } else {
                        format!("{mf:.0} Hz")
                    },
                );
                set_txt(
                    ui,
                    &mut cache,
                    17,
                    format!("{:.0}", p.pre_master_target_db.raw_target()),
                );

                // --- telemetry (band bars + Vizia tolerance corridors) ---
                let mut band_levels = [0.0f32; 5];
                let mut target_levels = [0.0f32; 5];
                let mut target_tols = [0.0f32; 5];
                let mut listen_levels = [0.0f32; 5];
                let mut listen_tols = [0.0f32; 5];
                let mut listen_mins = [0.0f32; 5];
                let mut listen_maxs = [0.0f32; 5];
                for b in 0..5 {
                    band_levels[b] = shared.band_levels[b].load(Ordering::Acquire);
                    target_levels[b] = shared.target_levels[b].load(Ordering::Acquire);
                    target_tols[b] = shared.target_tolerances[b].load(Ordering::Acquire);
                    listen_levels[b] = shared.listen_levels[b].load(Ordering::Acquire);
                    listen_tols[b] = shared.listen_tolerances[b].load(Ordering::Acquire);
                    listen_mins[b] = shared.listen_level_min[b].load(Ordering::Acquire);
                    listen_maxs[b] = shared.listen_level_max[b].load(Ordering::Acquire);
                }
                let listen_samples = shared.listen_samples.load(Ordering::Acquire);
                let listening = listen_samples > 0.0;
                let listen_ready = listen_samples > 100.0;
                if changed_bool(&mut cache.listening, listening) {
                    ui.set_listening(listening);
                }
                if changed_bool(&mut cache.listen_ready, listen_ready) {
                    ui.set_listen_ready(listen_ready);
                }

                let listen_avg: f32 =
                    listen_levels.iter().map(|v| v.max(-50.0)).sum::<f32>() / 5.0;

                let bn = [
                    band_bar_norm(&band_levels, 0),
                    band_bar_norm(&band_levels, 1),
                    band_bar_norm(&band_levels, 2),
                    band_bar_norm(&band_levels, 3),
                    band_bar_norm(&band_levels, 4),
                ];
                let tn = [
                    target_bar_norm(&target_levels, 0),
                    target_bar_norm(&target_levels, 1),
                    target_bar_norm(&target_levels, 2),
                    target_bar_norm(&target_levels, 3),
                    target_bar_norm(&target_levels, 4),
                ];
                let ttn = [
                    tol_bar_norm(target_tols[0]),
                    tol_bar_norm(target_tols[1]),
                    tol_bar_norm(target_tols[2]),
                    tol_bar_norm(target_tols[3]),
                    tol_bar_norm(target_tols[4]),
                ];
                let ln = [
                    band_bar_norm_avg(&listen_levels, listen_avg, 0, -50.0),
                    band_bar_norm_avg(&listen_levels, listen_avg, 1, -50.0),
                    band_bar_norm_avg(&listen_levels, listen_avg, 2, -50.0),
                    band_bar_norm_avg(&listen_levels, listen_avg, 3, -50.0),
                    band_bar_norm_avg(&listen_levels, listen_avg, 4, -50.0),
                ];
                let ltn = [
                    tol_bar_norm(listen_tols[0]),
                    tol_bar_norm(listen_tols[1]),
                    tol_bar_norm(listen_tols[2]),
                    tol_bar_norm(listen_tols[3]),
                    tol_bar_norm(listen_tols[4]),
                ];
                let llo = [
                    band_bar_norm_avg(&listen_mins, listen_avg, 0, -50.0),
                    band_bar_norm_avg(&listen_mins, listen_avg, 1, -50.0),
                    band_bar_norm_avg(&listen_mins, listen_avg, 2, -50.0),
                    band_bar_norm_avg(&listen_mins, listen_avg, 3, -50.0),
                    band_bar_norm_avg(&listen_mins, listen_avg, 4, -50.0),
                ];
                let lhi = [
                    band_bar_norm_avg(&listen_maxs, listen_avg, 0, -50.0),
                    band_bar_norm_avg(&listen_maxs, listen_avg, 1, -50.0),
                    band_bar_norm_avg(&listen_maxs, listen_avg, 2, -50.0),
                    band_bar_norm_avg(&listen_maxs, listen_avg, 3, -50.0),
                    band_bar_norm_avg(&listen_maxs, listen_avg, 4, -50.0),
                ];
                for i in 0..5 {
                    if changed_f32(&mut cache.bands[i], bn[i]) {
                        match i {
                            0 => ui.set_band0(bn[i]),
                            1 => ui.set_band1(bn[i]),
                            2 => ui.set_band2(bn[i]),
                            3 => ui.set_band3(bn[i]),
                            4 => ui.set_band4(bn[i]),
                            _ => {}
                        }
                    }
                    if changed_f32(&mut cache.tgts[i], tn[i]) {
                        match i {
                            0 => ui.set_tgt0(tn[i]),
                            1 => ui.set_tgt1(tn[i]),
                            2 => ui.set_tgt2(tn[i]),
                            3 => ui.set_tgt3(tn[i]),
                            4 => ui.set_tgt4(tn[i]),
                            _ => {}
                        }
                    }
                    if changed_f32(&mut cache.tgt_tols[i], ttn[i]) {
                        match i {
                            0 => ui.set_tgt_tol0(ttn[i]),
                            1 => ui.set_tgt_tol1(ttn[i]),
                            2 => ui.set_tgt_tol2(ttn[i]),
                            3 => ui.set_tgt_tol3(ttn[i]),
                            4 => ui.set_tgt_tol4(ttn[i]),
                            _ => {}
                        }
                    }
                    if changed_f32(&mut cache.lis[i], ln[i]) {
                        match i {
                            0 => ui.set_lis0(ln[i]),
                            1 => ui.set_lis1(ln[i]),
                            2 => ui.set_lis2(ln[i]),
                            3 => ui.set_lis3(ln[i]),
                            4 => ui.set_lis4(ln[i]),
                            _ => {}
                        }
                    }
                    if changed_f32(&mut cache.lis_tols[i], ltn[i]) {
                        match i {
                            0 => ui.set_lis_tol0(ltn[i]),
                            1 => ui.set_lis_tol1(ltn[i]),
                            2 => ui.set_lis_tol2(ltn[i]),
                            3 => ui.set_lis_tol3(ltn[i]),
                            4 => ui.set_lis_tol4(ltn[i]),
                            _ => {}
                        }
                    }
                    if changed_f32(&mut cache.lis_lo[i], llo[i]) {
                        match i {
                            0 => ui.set_lis_lo0(llo[i]),
                            1 => ui.set_lis_lo1(llo[i]),
                            2 => ui.set_lis_lo2(llo[i]),
                            3 => ui.set_lis_lo3(llo[i]),
                            4 => ui.set_lis_lo4(llo[i]),
                            _ => {}
                        }
                    }
                    if changed_f32(&mut cache.lis_hi[i], lhi[i]) {
                        match i {
                            0 => ui.set_lis_hi0(lhi[i]),
                            1 => ui.set_lis_hi1(lhi[i]),
                            2 => ui.set_lis_hi2(lhi[i]),
                            3 => ui.set_lis_hi3(lhi[i]),
                            4 => ui.set_lis_hi4(lhi[i]),
                            _ => {}
                        }
                    }
                }

                let peak_l = shared.peaks.output_peak_l.load(Ordering::Acquire);
                let peak_r = shared.peaks.output_peak_r.load(Ordering::Acquire);
                let hold_l = shared.peaks.peak_hold_l.load(Ordering::Acquire);
                let hold_r = shared.peaks.peak_hold_r.load(Ordering::Acquire);
                let corr = shared.peaks.phase_correlation.load(Ordering::Acquire);
                let balance = shared.peaks.balance.load(Ordering::Acquire);

                let ml = db_to_meter(peak_l);
                let mr = db_to_meter(peak_r);
                let phl = db_to_meter(hold_l);
                let phr = db_to_meter(hold_r);
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
                if changed_f32(&mut cache.corr, corr) {
                    ui.set_correlation(corr);
                    ui.set_corr_text(SharedString::from(format!("corr: {corr:.2}")));
                }
                if changed_f32(&mut cache.balance, balance) {
                    ui.set_balance(balance);
                    ui.set_balance_text(SharedString::from(format!("bal: {balance:.2}")));
                }
                let hlq = (hold_l * 10.0).round() / 10.0;
                let hrq = (hold_r * 10.0).round() / 10.0;
                if changed_f32(&mut cache.hold_l_q, hlq) {
                    ui.set_peak_l_text(SharedString::from(fmt_db(hold_l)));
                }
                if changed_f32(&mut cache.hold_r_q, hrq) {
                    ui.set_peak_r_text(SharedString::from(fmt_db(hold_r)));
                }

                // Auto loud: apply gain offset when measure ends
                let measuring = shared.auto_loud.measuring.load(Ordering::Acquire);
                if changed_bool(&mut cache.auto_loud, measuring) {
                    ui.set_auto_loud_measuring(measuring);
                    ui.set_auto_loud_label(SharedString::from(if measuring {
                        "MEASURING..."
                    } else {
                        "AUTO LOUD"
                    }));
                }
                // Falling edge handled by checking offset when not measuring
                if !measuring {
                    let offset = shared.auto_loud.gain_offset.load(Ordering::Acquire);
                    if offset.abs() > 0.05 {
                        shared.auto_loud.gain_offset.store(0.0, Ordering::Release);
                        let cur = p.output_gain.raw_target() as f32;
                        let new_db = (cur + offset).clamp(-12.0, 12.0);
                        let norm = ((new_db + 12.0) / 24.0) as f64;
                        state.automate(P::OutputGain, norm.clamp(0.0, 1.0));
                    }
                }

                // SNAP label + write file on complete
                let snap_now = shared.snap.active.load(Ordering::Acquire);
                let vault_path = vault_state
                    .try_lock()
                    .ok()
                    .and_then(|g| g.vault_path.clone());
                if snap_now {
                    cache.snap_blink = cache.snap_blink.wrapping_add(1);
                    let label = if (cache.snap_blink / 8).is_multiple_of(2) {
                        "ANALYZE..."
                    } else {
                        "ANALYZE· · ·"
                    };
                    if changed_str(&mut cache.snap_label, label) {
                        ui.set_snap_label(SharedString::from(label));
                    }
                } else if cache.snap_was_active {
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
                        let sr = shared.sample_rate.load(Ordering::Acquire).max(1.0);
                        let md = snap_markdown(
                            &stereo,
                            &mono,
                            &delta,
                            band_levels,
                            corr,
                            peak_l,
                            peak_r,
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
                    cache.snap_label.clear();
                    cache.snap_label.push_str(label);
                    ui.set_snap_label(SharedString::from(label));
                }
                cache.snap_was_active = snap_now;

                let mut gonio_cmds = String::new();
                gonio_path(
                    shared.scope.drain().into_iter(),
                    &mut cache.gonio_window,
                    139.0,
                    139.0,
                    &mut gonio_cmds,
                );
                ui.set_gonio_path(SharedString::from(gonio_cmds));
            }
        },
    )
    .resizable(true)
    .into()
}
