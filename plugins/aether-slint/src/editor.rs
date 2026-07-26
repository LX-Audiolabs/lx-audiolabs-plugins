use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use slint::{ModelRc, SharedString, VecModel};
use truce::prelude::*;
use truce_core::cast::{discrete_index, discrete_norm};
use truce_core::editor::{Editor, PluginContextReadF32};
use truce_slint::{PluginContext, SlintEditor, SyncFn};

use crate::presets::{
    apply_profile, build_profile_md, default_preset_names, find_profile, load_cached_last_profile,
    merge_preset_names, preset_save_dir, profile_from_params, save_last_preset, spawn_vault_scan,
    PendingPresets, PresetEntry,
};
use crate::AetherParams;
use crate::AetherParamsParamId as P;
use crate::{set_band, Biquad, NUM_BANDS};

slint::include_modules!();

// Original Vizia Aether window size.
const WINDOW_W: u32 = 720;
const WINDOW_H: u32 = 395;

const FREQ_MIN: f32 = 20.0;
const FREQ_MAX: f32 = 20000.0;
const Q_MIN: f32 = 0.3;
const Q_MAX: f32 = 8.0;

/// Default band (freq, Q, type) for RESET — matches original Aether.
const BAND_DEF: [(f32, f32, i32); 5] = [
    (105.0, 0.7, 1),
    (300.0, 1.0, 2),
    (1200.0, 1.0, 2),
    (4000.0, 1.0, 2),
    (10000.0, 0.7, 3),
];

fn freq_to_norm(v: f32) -> f64 {
    (((v / FREQ_MIN).log10() / 3.0) as f64).clamp(0.0, 1.0)
}
fn gain_to_norm(v: f32) -> f64 {
    (((v + 12.0) / 24.0) as f64).clamp(0.0, 1.0)
}
fn q_to_norm(v: f32) -> f64 {
    let span = (Q_MAX / Q_MIN).log10();
    (((v / Q_MIN).log10() / span) as f64).clamp(0.0, 1.0)
}

fn parse_f32(s: &str) -> Option<f32> {
    s.trim().replace(',', ".").parse::<f32>().ok()
}

fn names_model(names: &[String]) -> ModelRc<SharedString> {
    let v: Vec<SharedString> = names.iter().map(|s| SharedString::from(s.as_str())).collect();
    ModelRc::new(VecModel::from(v))
}

struct PeakHold {
    hold: f32,
    ticks: u32,
}

struct VaultUiState {
    vault_path: Option<String>,
    names: Vec<String>,
    cache: Vec<PresetEntry>,
    pending: Arc<PendingPresets>,
    scanning_for: Option<String>,
}

/// Owns the Slint UI for one editor open.
///
/// On drop, `live` is set false *before* the component tree is destroyed.
/// That blocks focus-lost TextInput commits from calling host `automate`
/// during REAPER FX remove — a common crash-on-readd cause.
struct EditorSession {
    ui: AetherUi,
    live: Arc<AtomicBool>,
}

impl Drop for EditorSession {
    fn drop(&mut self) {
        self.live.store(false, Ordering::SeqCst);
    }
}

pub fn build_editor(params: Arc<AetherParams>) -> Box<dyn Editor> {
    let shared = params.shared.clone();
    // Fixed-size editor: avoid host resize races on open/close.
    SlintEditor::new(
        params.clone(),
        (WINDOW_W, WINDOW_H),
        move |state: PluginContext<AetherParams>| -> SyncFn<AetherParams> {
            let live = Arc::new(AtomicBool::new(true));
            // Never unwrap here: with a bad panic strategy (or host without
            // catch_unwind) this would kill REAPER on FX add when GUI is created.
            let ui_component = match AetherUi::new() {
                Ok(ui) => ui,
                Err(e) => {
                    tracing::error!("AetherUi::new failed: {e:?}");
                    // Empty sync — editor opens blank rather than aborting host.
                    return Box::new(|_state: &PluginContext<AetherParams>| {});
                }
            };
            let session = EditorSession {
                ui: ui_component,
                live: live.clone(),
            };
            let ui = &session.ui;

            // Labels only — never automate host params on open.
            let cfg = lx_analysis::load_config("Aether");
            let vault_path = cfg.vault_path.clone();
            if let Some(ref vp) = vault_path {
                ui.set_vault_path(SharedString::from(vp.as_str()));
            }
            let last_name = cfg
                .last_preset
                .clone()
                .or_else(|| load_cached_last_profile().map(|p| p.name))
                .unwrap_or_default();
            if !last_name.is_empty() {
                ui.set_preset_name(SharedString::from(last_name.as_str()));
            }

            let pending = Arc::new(PendingPresets::new());
            let vault_state = Arc::new(Mutex::new(VaultUiState {
                vault_path: vault_path.clone(),
                names: default_preset_names(),
                cache: Vec::new(),
                pending: pending.clone(),
                scanning_for: None,
            }));

            // Built-ins immediately; bg-scan vault (or local presets) so the
            // dropdown fills without needing SETUP → SAVE first. Drain is
            // non-blocking (try_lock + generation) on the UI sync tick.
            ui.set_preset_names(names_model(&default_preset_names()));
            {
                let scan_path = vault_path
                    .as_ref()
                    .filter(|p| !p.is_empty())
                    .cloned()
                    .or_else(|| {
                        let local = lx_analysis::get_plugin_dir("Aether").join("presets");
                        local.is_dir().then(|| local.to_string_lossy().into_owned())
                    });
                if let Some(vp) = scan_path {
                    if let Ok(mut vs) = vault_state.lock() {
                        let scan_gen = vs.pending.bump_generation();
                        vs.scanning_for = Some(vp.clone());
                        spawn_vault_scan(vp, vs.pending.clone(), scan_gen);
                    }
                }
            }
            let _ = pending; // held via vault_state

            // ── type cycle ──
            macro_rules! bind_type {
                ($cb:ident, $pid:expr) => {{
                    let s = state.clone();
                    let lv = live.clone();
                    ui.$cb(move |v| {
                        if !lv.load(Ordering::Acquire) {
                            return;
                        }
                        s.automate($pid, discrete_norm(v.max(0) as usize, 4));
                    });
                }};
            }
            bind_type!(on_eq1_type_changed, P::Eq1Type);
            bind_type!(on_eq2_type_changed, P::Eq2Type);
            bind_type!(on_eq3_type_changed, P::Eq3Type);
            bind_type!(on_eq4_type_changed, P::Eq4Type);
            bind_type!(on_eq5_type_changed, P::Eq5Type);

            // ── text field commits ──
            macro_rules! bind_freq {
                ($cb:ident, $pid:expr) => {{
                    let s = state.clone();
                    let lv = live.clone();
                    ui.$cb(move |txt: SharedString| {
                        if !lv.load(Ordering::Acquire) {
                            return;
                        }
                        if let Some(v) = parse_f32(txt.as_str()) {
                            s.automate($pid, freq_to_norm(v.clamp(FREQ_MIN, FREQ_MAX)));
                        }
                    });
                }};
            }
            macro_rules! bind_gain {
                ($cb:ident, $pid:expr) => {{
                    let s = state.clone();
                    let lv = live.clone();
                    ui.$cb(move |txt: SharedString| {
                        if !lv.load(Ordering::Acquire) {
                            return;
                        }
                        if let Some(v) = parse_f32(txt.as_str()) {
                            s.automate($pid, gain_to_norm(v.clamp(-12.0, 12.0)));
                        }
                    });
                }};
            }
            macro_rules! bind_q {
                ($cb:ident, $pid:expr) => {{
                    let s = state.clone();
                    let lv = live.clone();
                    ui.$cb(move |txt: SharedString| {
                        if !lv.load(Ordering::Acquire) {
                            return;
                        }
                        if let Some(v) = parse_f32(txt.as_str()) {
                            s.automate($pid, q_to_norm(v.clamp(Q_MIN, Q_MAX)));
                        }
                    });
                }};
            }
            bind_freq!(on_eq1_freq_committed, P::Eq1Freq);
            bind_gain!(on_eq1_gain_committed, P::Eq1Gain);
            bind_q!(on_eq1_q_committed, P::Eq1Q);
            bind_freq!(on_eq2_freq_committed, P::Eq2Freq);
            bind_gain!(on_eq2_gain_committed, P::Eq2Gain);
            bind_q!(on_eq2_q_committed, P::Eq2Q);
            bind_freq!(on_eq3_freq_committed, P::Eq3Freq);
            bind_gain!(on_eq3_gain_committed, P::Eq3Gain);
            bind_q!(on_eq3_q_committed, P::Eq3Q);
            bind_freq!(on_eq4_freq_committed, P::Eq4Freq);
            bind_gain!(on_eq4_gain_committed, P::Eq4Gain);
            bind_q!(on_eq4_q_committed, P::Eq4Q);
            bind_freq!(on_eq5_freq_committed, P::Eq5Freq);
            bind_gain!(on_eq5_gain_committed, P::Eq5Gain);
            bind_q!(on_eq5_q_committed, P::Eq5Q);

            // ── knobs ──
            macro_rules! bind_float {
                ($cb:ident, $pid:expr) => {{
                    let s = state.clone();
                    let lv = live.clone();
                    ui.$cb(move |v| {
                        if lv.load(Ordering::Acquire) {
                            s.automate($pid, v as f64);
                        }
                    });
                }};
            }
            bind_float!(on_blend_changed, P::Blend);
            bind_float!(on_cf_angle_changed, P::CfAngle);
            bind_float!(on_cf_amount_changed, P::CfAmount);
            bind_float!(on_gain_changed, P::Gain);

            let s = state.clone();
            let lv = live.clone();
            ui.on_cf_realism_changed(move |v| {
                if lv.load(Ordering::Acquire) {
                    s.automate(P::CfRealism, discrete_norm(v.max(0) as usize, 3));
                }
            });
            let s = state.clone();
            let lv = live.clone();
            ui.on_bypass_changed(move |v| {
                if lv.load(Ordering::Acquire) {
                    s.automate(P::Bypass, if v { 1.0 } else { 0.0 });
                }
            });

            // ── RESET ──
            let s = state.clone();
            let lv = live.clone();
            ui.on_reset_clicked(move || {
                if !lv.load(Ordering::Acquire) {
                    return;
                }
                for (i, &(fd, qd, td)) in BAND_DEF.iter().enumerate() {
                    let (fp, gp, qp, tp) = match i {
                        0 => (P::Eq1Freq, P::Eq1Gain, P::Eq1Q, P::Eq1Type),
                        1 => (P::Eq2Freq, P::Eq2Gain, P::Eq2Q, P::Eq2Type),
                        2 => (P::Eq3Freq, P::Eq3Gain, P::Eq3Q, P::Eq3Type),
                        3 => (P::Eq4Freq, P::Eq4Gain, P::Eq4Q, P::Eq4Type),
                        _ => (P::Eq5Freq, P::Eq5Gain, P::Eq5Q, P::Eq5Type),
                    };
                    s.automate(fp, freq_to_norm(fd));
                    s.automate(gp, gain_to_norm(0.0));
                    s.automate(qp, q_to_norm(qd));
                    s.automate(tp, discrete_norm(td.max(0) as usize, 4));
                }
                s.automate(P::Blend, 1.0);
                s.automate(P::CfAngle, ((60.0 - 30.0) / 45.0) as f64);
                s.automate(P::CfAmount, 0.0);
                s.automate(P::CfRealism, 0.0);
                s.automate(P::Gain, 0.5);
                s.automate(P::Bypass, 0.0);
            });

            // ── SAVE ──
            let params_save = params.clone();
            let vs_save = vault_state.clone();
            let ui_weak_save = ui.as_weak();
            let lv = live.clone();
            ui.on_save_clicked(move || {
                if !lv.load(Ordering::Acquire) {
                    return;
                }
                let Some(ui) = ui_weak_save.upgrade() else {
                    return;
                };
                let name = ui.get_preset_name().to_string().trim().to_string();
                if name.is_empty() {
                    return;
                }
                let md = build_profile_md(&params_save);
                let vp = vs_save.lock().ok().and_then(|g| g.vault_path.clone());
                let dir = preset_save_dir(&vp);
                let _ = std::fs::create_dir_all(&dir);
                let fp = dir.join(format!("{name}.md"));
                if std::fs::write(&fp, md).is_ok() {
                    let mut profile = profile_from_params(&params_save, &name);
                    profile.name = name.clone();
                    if let Ok(mut vs) = vs_save.lock() {
                        if let Some(pos) = vs.cache.iter().position(|(n, _, _)| n == &name) {
                            vs.cache[pos] = (name.clone(), fp.clone(), profile.clone());
                        } else {
                            vs.cache.push((name.clone(), fp.clone(), profile.clone()));
                        }
                        if !vs.names.iter().any(|n| n == &name) {
                            vs.names.push(name.clone());
                        }
                        ui.set_preset_names(names_model(&vs.names));
                        save_last_preset(&vs.vault_path, &profile);
                        if let Some(ref vault) = vs.vault_path.clone() {
                            if !vault.is_empty() {
                                let scan_gen = vs.pending.bump_generation();
                                vs.scanning_for = Some(vault.clone());
                                spawn_vault_scan(vault.clone(), vs.pending.clone(), scan_gen);
                            }
                        }
                    }
                    ui.set_preset_name(SharedString::from(name.as_str()));
                }
            });

            // ── vault path ──
            let vs_path = vault_state.clone();
            let ui_weak_path = ui.as_weak();
            let lv = live.clone();
            ui.on_vault_path_changed(move |path: SharedString| {
                if !lv.load(Ordering::Acquire) {
                    return;
                }
                let path = path.to_string().trim().to_string();
                let new_vp = if path.is_empty() { None } else { Some(path) };
                if let Ok(mut vs) = vs_path.lock() {
                    vs.vault_path = new_vp.clone();
                    let mut cfg = lx_analysis::load_config("Aether");
                    cfg.vault_path = new_vp.clone();
                    let _ = lx_analysis::save_config("Aether", &cfg);
                    let scan_gen = vs.pending.bump_generation();
                    if let Some(ref vp) = new_vp {
                        vs.scanning_for = Some(vp.clone());
                        spawn_vault_scan(vp.clone(), vs.pending.clone(), scan_gen);
                    } else {
                        vs.names = default_preset_names();
                        vs.cache.clear();
                        vs.scanning_for = None;
                        if let Some(ui) = ui_weak_path.upgrade() {
                            ui.set_preset_names(names_model(&vs.names));
                        }
                        let local = lx_analysis::get_plugin_dir("Aether").join("presets");
                        if local.is_dir() {
                            let scan_gen = vs.pending.bump_generation();
                            spawn_vault_scan(
                                local.to_string_lossy().into_owned(),
                                vs.pending.clone(),
                                scan_gen,
                            );
                        }
                    }
                }
            });

            // Vault Setup PASTE button (Ctrl+V often stolen by host).
            let paste_ui = ui.as_weak();
            let paste_lv = live.clone();
            ui.on_vault_paste_requested(move || {
                if !paste_lv.load(Ordering::Acquire) {
                    return;
                }
                let Some(ui) = paste_ui.upgrade() else { return };
                match vault_clipboard_get() {
                    Some(s) => {
                        ui.set_vault_path(SharedString::from(s));
                        ui.set_vault_paste_status(SharedString::new());
                    }
                    None => {
                        ui.set_vault_paste_status(SharedString::from(
                            "Clipboard empty or unavailable — copy a path and try PASTE again",
                        ));
                    }
                }
            });

            // ── preset selected ──
            let s_sel = state.clone();
            let vs_sel = vault_state.clone();
            let ui_weak_sel = ui.as_weak();
            let lv = live.clone();
            ui.on_preset_selected(move |name: SharedString| {
                if !lv.load(Ordering::Acquire) {
                    return;
                }
                let name = name.to_string();
                let profile = {
                    let vs = vs_sel.lock().ok();
                    let (vp, cache) = vs
                        .as_ref()
                        .map(|g| (g.vault_path.clone(), g.cache.clone()))
                        .unwrap_or((None, vec![]));
                    find_profile(&name, &vp, &cache)
                };
                if let Some(profile) = profile {
                    apply_profile(&s_sel, &profile);
                    save_last_preset(
                        &vs_sel.lock().ok().and_then(|g| g.vault_path.clone()),
                        &profile,
                    );
                    if let Some(ui) = ui_weak_sel.upgrade() {
                        ui.set_preset_name(SharedString::from(profile.name.as_str()));
                    }
                }
            });

            let params_for_curve = params.clone();
            let shared_for_curve = shared.clone();
            let peak_hold = RefCell::new(PeakHold {
                hold: -90.0,
                ticks: 0,
            });
            let vault_state_sync = vault_state.clone();
            let live_sync = live.clone();

            Box::new(move |state: &PluginContext<AetherParams>| {
                if !live_sync.load(Ordering::Acquire) {
                    return;
                }
                let ui = &session.ui;

                // Drain background vault scan (non-blocking).
                if let Ok(mut vs) = vault_state_sync.try_lock() {
                    if vs.pending.ready.swap(false, Ordering::Acquire) {
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
                }

                ui.set_eq1_type(
                    discrete_index(PluginContextReadF32::get_param(state, P::Eq1Type) as f64, 4)
                        as i32,
                );
                ui.set_eq2_type(
                    discrete_index(PluginContextReadF32::get_param(state, P::Eq2Type) as f64, 4)
                        as i32,
                );
                ui.set_eq3_type(
                    discrete_index(PluginContextReadF32::get_param(state, P::Eq3Type) as f64, 4)
                        as i32,
                );
                ui.set_eq4_type(
                    discrete_index(PluginContextReadF32::get_param(state, P::Eq4Type) as f64, 4)
                        as i32,
                );
                ui.set_eq5_type(
                    discrete_index(PluginContextReadF32::get_param(state, P::Eq5Type) as f64, 4)
                        as i32,
                );
                ui.set_cf_realism(
                    discrete_index(
                        PluginContextReadF32::get_param(state, P::CfRealism) as f64,
                        3,
                    ) as i32,
                );

                ui.set_blend(PluginContextReadF32::get_param(state, P::Blend));
                ui.set_cf_angle(PluginContextReadF32::get_param(state, P::CfAngle));
                ui.set_cf_amount(PluginContextReadF32::get_param(state, P::CfAmount));
                ui.set_gain(PluginContextReadF32::get_param(state, P::Gain));
                ui.set_bypass(PluginContextReadF32::get_param(state, P::Bypass) > 0.5);

                let plain = |p: P| -> f32 {
                    match p {
                        P::Eq1Freq => state.params().eq1_freq.raw_target() as f32,
                        P::Eq1Gain => state.params().eq1_gain.raw_target() as f32,
                        P::Eq1Q => state.params().eq1_q.raw_target() as f32,
                        P::Eq2Freq => state.params().eq2_freq.raw_target() as f32,
                        P::Eq2Gain => state.params().eq2_gain.raw_target() as f32,
                        P::Eq2Q => state.params().eq2_q.raw_target() as f32,
                        P::Eq3Freq => state.params().eq3_freq.raw_target() as f32,
                        P::Eq3Gain => state.params().eq3_gain.raw_target() as f32,
                        P::Eq3Q => state.params().eq3_q.raw_target() as f32,
                        P::Eq4Freq => state.params().eq4_freq.raw_target() as f32,
                        P::Eq4Gain => state.params().eq4_gain.raw_target() as f32,
                        P::Eq4Q => state.params().eq4_q.raw_target() as f32,
                        P::Eq5Freq => state.params().eq5_freq.raw_target() as f32,
                        P::Eq5Gain => state.params().eq5_gain.raw_target() as f32,
                        P::Eq5Q => state.params().eq5_q.raw_target() as f32,
                        P::Blend => state.params().blend.raw_target() as f32,
                        P::CfAngle => state.params().cf_angle.raw_target() as f32,
                        P::CfAmount => state.params().cf_amount.raw_target() as f32,
                        P::Gain => state.params().gain.raw_target() as f32,
                        _ => 0.0,
                    }
                };
                ui.set_eq1_freq_text(SharedString::from(format!("{:.0}", plain(P::Eq1Freq))));
                ui.set_eq1_gain_text(SharedString::from(format!("{:.1}", plain(P::Eq1Gain))));
                ui.set_eq1_q_text(SharedString::from(format!("{:.2}", plain(P::Eq1Q))));
                ui.set_eq2_freq_text(SharedString::from(format!("{:.0}", plain(P::Eq2Freq))));
                ui.set_eq2_gain_text(SharedString::from(format!("{:.1}", plain(P::Eq2Gain))));
                ui.set_eq2_q_text(SharedString::from(format!("{:.2}", plain(P::Eq2Q))));
                ui.set_eq3_freq_text(SharedString::from(format!("{:.0}", plain(P::Eq3Freq))));
                ui.set_eq3_gain_text(SharedString::from(format!("{:.1}", plain(P::Eq3Gain))));
                ui.set_eq3_q_text(SharedString::from(format!("{:.2}", plain(P::Eq3Q))));
                ui.set_eq4_freq_text(SharedString::from(format!("{:.0}", plain(P::Eq4Freq))));
                ui.set_eq4_gain_text(SharedString::from(format!("{:.1}", plain(P::Eq4Gain))));
                ui.set_eq4_q_text(SharedString::from(format!("{:.2}", plain(P::Eq4Q))));
                ui.set_eq5_freq_text(SharedString::from(format!("{:.0}", plain(P::Eq5Freq))));
                ui.set_eq5_gain_text(SharedString::from(format!("{:.1}", plain(P::Eq5Gain))));
                ui.set_eq5_q_text(SharedString::from(format!("{:.2}", plain(P::Eq5Q))));

                let blend = plain(P::Blend);
                ui.set_blend_text(SharedString::from(format!("{blend:.0}%")));
                let angle = plain(P::CfAngle);
                ui.set_cf_angle_text(SharedString::from(format!("{angle:.0}°")));
                let amount = plain(P::CfAmount);
                ui.set_cf_amount_text(SharedString::from(format!("{amount:.0}%")));
                let g = plain(P::Gain);
                ui.set_gain_text(SharedString::from(format!("{g:.1} dB")));

                let peak_db = state.shared.input_peak.load(Ordering::Relaxed);
                {
                    let mut ph = peak_hold.borrow_mut();
                    if peak_db > ph.hold {
                        ph.hold = peak_db;
                        ph.ticks = 90;
                    } else if ph.ticks > 0 {
                        ph.ticks -= 1;
                    } else {
                        ph.hold = (ph.hold - 0.5).max(peak_db);
                    }
                    if peak_db <= -90.0 {
                        ui.set_input_db_text(SharedString::from("--"));
                    } else {
                        ui.set_input_db_text(SharedString::from(format!("{peak_db:.1} dB")));
                    }
                    if ph.hold <= -90.0 {
                        ui.set_input_peak_text(SharedString::from(""));
                    } else {
                        ui.set_input_peak_text(SharedString::from(format!("pk {:.1} dB", ph.hold)));
                    }
                }

                let sr = shared_for_curve.sample_rate.load(Ordering::Relaxed).max(1.0);
                let cmds = eq_curve_path(&params_for_curve, sr);
                ui.set_curve_cmds(SharedString::from(cmds));
            })
        },
    )
    .resizable(false)
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
    const W: f32 = 696.0;
    const H: f32 = 90.0;
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
        let db: f32 = bands
            .iter()
            .map(|b| b.magnitude_db(freq, sr))
            .sum::<f32>()
            .clamp(DB_MIN, DB_MAX);
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

/// OS clipboard for Vault Setup PASTE (Ctrl+V often stolen by host).
fn vault_clipboard_get() -> Option<String> {
    use copypasta::{ClipboardContext, ClipboardProvider};
    for attempt in 0..12 {
        let got = ClipboardContext::new()
            .ok()
            .and_then(|mut ctx| ctx.get_contents().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if got.is_some() {
            return got;
        }
        if attempt + 1 < 12 {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    None
}
