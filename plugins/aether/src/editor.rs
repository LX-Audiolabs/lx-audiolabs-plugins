use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aura::prelude::*;
use slint::{ModelRc, SharedString, VecModel};
use lx_slint_editor::{
    apply_ui_zoom, discrete_index, discrete_norm, LxPluginContext, LxSlintEditor,
    PluginContextReadF32, UiZoom,
};

use crate::presets::{
    apply_profile, build_profile_md, default_preset_names, find_profile, load_cached_last_profile,
    merge_preset_names, preset_save_dir, profile_from_params, save_last_preset, spawn_vault_scan,
    PendingPresets, PresetEntry,
};
use crate::AetherParams;
use crate::AetherParamsParamId as P;
use crate::{set_band, Biquad, NUM_BANDS};

slint::include_modules!();

// Slightly wider than original 720 Vizia — section titles need room.
const WINDOW_W: u32 = 730;
const WINDOW_H: u32 = 395;
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Match Meridian `TICK_INTERVAL` — host→UI poll ~30 Hz.
const TICK_INTERVAL: Duration = Duration::from_millis(33);

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

// --- dirty helpers ----------------------------------------------------------

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

#[inline]
fn changed_i32(prev: &mut i32, v: i32) -> bool {
    if *prev != v {
        *prev = v;
        true
    } else {
        false
    }
}

#[inline]
fn changed_str(prev: &mut String, v: &str) -> bool {
    if prev.as_str() != v {
        prev.clear();
        prev.push_str(v);
        true
    } else {
        false
    }
}

/// Cache key for EQ curve (skip rebuild when bands unchanged).
#[derive(Clone, Copy, PartialEq)]
struct EqCurveKey {
    bands: [(i32, f32, f32, f32); 5],
    sample_rate: f32,
}

fn eq_curve_key(params: &AetherParams, sr: f32) -> EqCurveKey {
    EqCurveKey {
        bands: [
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
        ],
        sample_rate: sr,
    }
}

/// Per-editor sync bookkeeping (Meridian SyncCache parity).
struct SyncCache {
    last_tick: Instant,
    primed: bool,
    eq_key: Option<EqCurveKey>,
    eq_cmds: String,
    // Dirty mirrors
    types: [i32; 5],
    floats: [f32; 4], // blend, cf_angle, cf_amount, gain
    cf_realism: i32,
    bypass: Option<bool>,
    // Text fields (full string compare — host formats change rarely)
    band_text: [String; 15], // 5 bands × (freq, gain, q)
    knob_text: [String; 4],  // blend, angle, amount, gain
    input_db: String,
    input_peak: String,
    peak_hold: f32,
    peak_hold_ticks: u32,
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
            types: [i32::MIN; 5],
            floats: [f32::NAN; 4],
            cf_realism: i32::MIN,
            bypass: None,
            band_text: std::array::from_fn(|_| String::new()),
            knob_text: std::array::from_fn(|_| String::new()),
            input_db: String::new(),
            input_peak: String::new(),
            peak_hold: -90.0,
            peak_hold_ticks: 0,
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

struct VaultUiState {
    vault_path: Option<String>,
    names: Vec<String>,
    cache: Vec<PresetEntry>,
    pending: Arc<PendingPresets>,
    scanning_for: Option<String>,
}

pub fn build_editor(params: Arc<AetherParams>) -> Box<dyn Editor> {
    let shared = params.shared.clone();
    let sync_cache = Mutex::new(SyncCache::new());

    let init_cfg = lx_analysis::load_config("Aether");
    let init_vp = init_cfg.vault_path.clone();
    let vault_pending = Arc::new(PendingPresets::new());
    {
        // Built-ins immediately; bg-scan vault (or local presets).
        let scan_path = init_vp
            .as_ref()
            .filter(|p| !p.is_empty())
            .cloned()
            .or_else(|| {
                let local = lx_analysis::get_plugin_dir("Aether").join("presets");
                local
                    .is_dir()
                    .then(|| local.to_string_lossy().into_owned())
            });
        if let Some(vp) = scan_path {
            let scan_gen = vault_pending.bump_generation();
            spawn_vault_scan(vp, vault_pending.clone(), scan_gen);
        }
    }
    let vault_state = Arc::new(Mutex::new(VaultUiState {
        vault_path: init_vp.clone(),
        names: default_preset_names(),
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
            let vault_state = vault_state.clone();
            let init_vp = init_vp.clone();
            let init_last = init_cfg.last_preset.clone();
            move |state: LxPluginContext<AetherParams>| {
                let ui = AetherUi::new().expect("AetherUi::new");
                ui.set_version(SharedString::from(VERSION));

                ui.set_ui_zoom_percent(zoom_build.percent() as i32);
                {
                    let z = zoom_build.clone();
                    let s = state.clone();
                    ui.on_ui_zoom_changed(move |p| {
                        apply_ui_zoom(&z, |w, h| s.request_resize(w, h), p as u32);
                    });
                }

                if let Some(ref vp) = init_vp {
                    ui.set_vault_path(SharedString::from(vp.as_str()));
                }

                // Vizia parity: restore last-used preset *values* on open
                // (not just the name label). Cache first for instant startup;
                // fall back to built-in / local file via find_profile.
                let last_name = init_last
                    .clone()
                    .or_else(|| load_cached_last_profile().map(|p| p.name))
                    .unwrap_or_default();
                if !last_name.is_empty() {
                    ui.set_preset_name(SharedString::from(last_name.as_str()));
                    let profile = load_cached_last_profile()
                        .filter(|p| p.name == last_name)
                        .or_else(|| find_profile(&last_name, &init_vp, &[]));
                    if let Some(pf) = profile {
                        apply_profile(&state, &pf);
                    }
                }
                ui.set_preset_names(names_model(&default_preset_names()));

                // ── type cycle ──
                {
                    let s = state.clone();
                    ui.on_eq1_type_changed(move |v| {
                        s.automate(P::Eq1Type, discrete_norm(v.max(0) as usize, 4));
                    });
                }
                {
                    let s = state.clone();
                    ui.on_eq2_type_changed(move |v| {
                        s.automate(P::Eq2Type, discrete_norm(v.max(0) as usize, 4));
                    });
                }
                {
                    let s = state.clone();
                    ui.on_eq3_type_changed(move |v| {
                        s.automate(P::Eq3Type, discrete_norm(v.max(0) as usize, 4));
                    });
                }
                {
                    let s = state.clone();
                    ui.on_eq4_type_changed(move |v| {
                        s.automate(P::Eq4Type, discrete_norm(v.max(0) as usize, 4));
                    });
                }
                {
                    let s = state.clone();
                    ui.on_eq5_type_changed(move |v| {
                        s.automate(P::Eq5Type, discrete_norm(v.max(0) as usize, 4));
                    });
                }

                // ── text field commits ──
                macro_rules! bind_freq {
                    ($cb:ident, $pid:expr) => {{
                        let s = state.clone();
                        ui.$cb(move |txt: SharedString| {
                            if let Some(v) = parse_f32(txt.as_str()) {
                                s.automate($pid, freq_to_norm(v.clamp(FREQ_MIN, FREQ_MAX)));
                            }
                        });
                    }};
                }
                macro_rules! bind_gain {
                    ($cb:ident, $pid:expr) => {{
                        let s = state.clone();
                        ui.$cb(move |txt: SharedString| {
                            if let Some(v) = parse_f32(txt.as_str()) {
                                s.automate($pid, gain_to_norm(v.clamp(-12.0, 12.0)));
                            }
                        });
                    }};
                }
                macro_rules! bind_q {
                    ($cb:ident, $pid:expr) => {{
                        let s = state.clone();
                        ui.$cb(move |txt: SharedString| {
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
                {
                    let s = state.clone();
                    ui.on_blend_changed(move |v| s.automate(P::Blend, v as f64));
                }
                {
                    let s = state.clone();
                    ui.on_cf_angle_changed(move |v| s.automate(P::CfAngle, v as f64));
                }
                {
                    let s = state.clone();
                    ui.on_cf_amount_changed(move |v| s.automate(P::CfAmount, v as f64));
                }
                {
                    let s = state.clone();
                    ui.on_gain_changed(move |v| s.automate(P::Gain, v as f64));
                }
                {
                    let s = state.clone();
                    ui.on_cf_realism_changed(move |v| {
                        s.automate(P::CfRealism, discrete_norm(v.max(0) as usize, 3));
                    });
                }
                {
                    let s = state.clone();
                    ui.on_bypass_changed(move |v| {
                        s.automate(P::Bypass, if v { 1.0 } else { 0.0 });
                    });
                }

                // ── RESET ──
                {
                    let s = state.clone();
                    ui.on_reset_clicked(move || {
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
                        s.automate(P::CfAngle, (60.0 - 30.0) / 45.0);
                        s.automate(P::CfAmount, 0.0);
                        s.automate(P::CfRealism, 0.0);
                        s.automate(P::Gain, 0.5);
                        s.automate(P::Bypass, 0.0);
                        tracing::info!("RESET clicked");
                    });
                }

                // ── SAVE ──
                {
                    let params_save = params.clone();
                    let vs_save = vault_state.clone();
                    let ui_weak = ui.as_weak();
                    ui.on_save_clicked(move || {
                        let Some(ui) = ui_weak.upgrade() else {
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
                                if let Some(pos) = vs.cache.iter().position(|(n, _, _)| n == &name)
                                {
                                    vs.cache[pos] = (name.clone(), fp.clone(), profile.clone());
                                } else {
                                    vs.cache.push((name.clone(), fp.clone(), profile.clone()));
                                }
                                if !vs.names.iter().any(|n| n == &name) {
                                    vs.names.push(name.clone());
                                }
                                ui.set_preset_names(names_model(&vs.names));
                                save_last_preset(&vs.vault_path, &profile);
                                if let Some(ref vault) = vs.vault_path.clone()
                                    && !vault.is_empty()
                                {
                                    let scan_gen = vs.pending.bump_generation();
                                    vs.scanning_for = Some(vault.clone());
                                    spawn_vault_scan(
                                        vault.clone(),
                                        vs.pending.clone(),
                                        scan_gen,
                                    );
                                }
                            }
                            ui.set_preset_name(SharedString::from(name.as_str()));
                            tracing::info!("SAVE preset to {}", fp.display());
                        }
                    });
                }

                // ── vault path ──
                {
                    let vs_path = vault_state.clone();
                    let ui_weak = ui.as_weak();
                    ui.on_vault_path_changed(move |path: SharedString| {
                        let path = path.to_string().trim().to_string();
                        let new_vp = if path.is_empty() { None } else { Some(path) };
                        if let Ok(mut vs) = vs_path.lock() {
                            vs.vault_path = new_vp.clone();
                            let mut cfg = lx_analysis::load_config("Aether");
                            cfg.vault_path = new_vp.clone();
                            let _ = lx_analysis::save_config("Aether", &cfg);
                            if let Some(ref vp) = new_vp {
                                let scan_gen = vs.pending.bump_generation();
                                vs.scanning_for = Some(vp.clone());
                                spawn_vault_scan(vp.clone(), vs.pending.clone(), scan_gen);
                            } else {
                                vs.names = default_preset_names();
                                vs.cache.clear();
                                vs.scanning_for = None;
                                if let Some(ui) = ui_weak.upgrade() {
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
                }

                // Vault Setup PASTE → draft path (vault_setup_path). Ctrl+V also
                // works via slint-baseview clipboard_get_retry when TextInput focused.
                {
                    let paste_ui = ui.as_weak();
                    ui.on_vault_paste_requested(move || {
                        let Some(ui) = paste_ui.upgrade() else {
                            return;
                        };
                        match lx_slint_editor::clipboard_get_retry() {
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

                // ── preset selected ──
                {
                    let s_sel = state.clone();
                    let vs_sel = vault_state.clone();
                    let ui_weak = ui.as_weak();
                    ui.on_preset_selected(move |name: SharedString| {
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
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.set_preset_name(SharedString::from(profile.name.as_str()));
                            }
                        }
                    });
                }

                ui
            }
        },
        {
            let params_for_curve = params.clone();
            let shared_for_sync = shared.clone();
            let vault_state_sync = vault_state.clone();
            move |ui: &AetherUi, state: &LxPluginContext<AetherParams>| {
                let Ok(mut cache) = sync_cache.lock() else {
                    return;
                };
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

                // --- types / realism / knobs / bypass (dirty) ---
                let t1 = discrete_index(
                    PluginContextReadF32::get_param(state, P::Eq1Type) as f64,
                    4,
                ) as i32;
                let t2 = discrete_index(
                    PluginContextReadF32::get_param(state, P::Eq2Type) as f64,
                    4,
                ) as i32;
                let t3 = discrete_index(
                    PluginContextReadF32::get_param(state, P::Eq3Type) as f64,
                    4,
                ) as i32;
                let t4 = discrete_index(
                    PluginContextReadF32::get_param(state, P::Eq4Type) as f64,
                    4,
                ) as i32;
                let t5 = discrete_index(
                    PluginContextReadF32::get_param(state, P::Eq5Type) as f64,
                    4,
                ) as i32;
                if changed_i32(&mut cache.types[0], t1) {
                    ui.set_eq1_type(t1);
                }
                if changed_i32(&mut cache.types[1], t2) {
                    ui.set_eq2_type(t2);
                }
                if changed_i32(&mut cache.types[2], t3) {
                    ui.set_eq3_type(t3);
                }
                if changed_i32(&mut cache.types[3], t4) {
                    ui.set_eq4_type(t4);
                }
                if changed_i32(&mut cache.types[4], t5) {
                    ui.set_eq5_type(t5);
                }

                let realism = discrete_index(
                    PluginContextReadF32::get_param(state, P::CfRealism) as f64,
                    3,
                ) as i32;
                if changed_i32(&mut cache.cf_realism, realism) {
                    ui.set_cf_realism(realism);
                }

                let blend = PluginContextReadF32::get_param(state, P::Blend);
                let angle = PluginContextReadF32::get_param(state, P::CfAngle);
                let amount = PluginContextReadF32::get_param(state, P::CfAmount);
                let gain = PluginContextReadF32::get_param(state, P::Gain);
                if changed_f32(&mut cache.floats[0], blend) {
                    ui.set_blend(blend);
                }
                if changed_f32(&mut cache.floats[1], angle) {
                    ui.set_cf_angle(angle);
                }
                if changed_f32(&mut cache.floats[2], amount) {
                    ui.set_cf_amount(amount);
                }
                if changed_f32(&mut cache.floats[3], gain) {
                    ui.set_gain(gain);
                }

                let bypass = PluginContextReadF32::get_param(state, P::Bypass) > 0.5;
                if changed_bool(&mut cache.bypass, bypass) {
                    ui.set_bypass(bypass);
                }

                // --- plain-value text fields ---
                let plain = |p: P| -> f32 {
                    match p {
                        P::Eq1Freq => state.eq1_freq.raw_target() as f32,
                        P::Eq1Gain => state.eq1_gain.raw_target() as f32,
                        P::Eq1Q => state.eq1_q.raw_target() as f32,
                        P::Eq2Freq => state.eq2_freq.raw_target() as f32,
                        P::Eq2Gain => state.eq2_gain.raw_target() as f32,
                        P::Eq2Q => state.eq2_q.raw_target() as f32,
                        P::Eq3Freq => state.eq3_freq.raw_target() as f32,
                        P::Eq3Gain => state.eq3_gain.raw_target() as f32,
                        P::Eq3Q => state.eq3_q.raw_target() as f32,
                        P::Eq4Freq => state.eq4_freq.raw_target() as f32,
                        P::Eq4Gain => state.eq4_gain.raw_target() as f32,
                        P::Eq4Q => state.eq4_q.raw_target() as f32,
                        P::Eq5Freq => state.eq5_freq.raw_target() as f32,
                        P::Eq5Gain => state.eq5_gain.raw_target() as f32,
                        P::Eq5Q => state.eq5_q.raw_target() as f32,
                        P::Blend => state.blend.raw_target() as f32,
                        P::CfAngle => state.cf_angle.raw_target() as f32,
                        P::CfAmount => state.cf_amount.raw_target() as f32,
                        P::Gain => state.gain.raw_target() as f32,
                        _ => 0.0,
                    }
                };

                let set_band_txt = |ui: &AetherUi, cache: &mut SyncCache, i: usize, s: String| {
                    if changed_str(&mut cache.band_text[i], &s) {
                        let ss = SharedString::from(s.as_str());
                        match i {
                            0 => ui.set_eq1_freq_text(ss),
                            1 => ui.set_eq1_gain_text(ss),
                            2 => ui.set_eq1_q_text(ss),
                            3 => ui.set_eq2_freq_text(ss),
                            4 => ui.set_eq2_gain_text(ss),
                            5 => ui.set_eq2_q_text(ss),
                            6 => ui.set_eq3_freq_text(ss),
                            7 => ui.set_eq3_gain_text(ss),
                            8 => ui.set_eq3_q_text(ss),
                            9 => ui.set_eq4_freq_text(ss),
                            10 => ui.set_eq4_gain_text(ss),
                            11 => ui.set_eq4_q_text(ss),
                            12 => ui.set_eq5_freq_text(ss),
                            13 => ui.set_eq5_gain_text(ss),
                            14 => ui.set_eq5_q_text(ss),
                            _ => {}
                        }
                    }
                };
                set_band_txt(ui, &mut cache, 0, format!("{:.0}", plain(P::Eq1Freq)));
                set_band_txt(ui, &mut cache, 1, format!("{:.1}", plain(P::Eq1Gain)));
                set_band_txt(ui, &mut cache, 2, format!("{:.2}", plain(P::Eq1Q)));
                set_band_txt(ui, &mut cache, 3, format!("{:.0}", plain(P::Eq2Freq)));
                set_band_txt(ui, &mut cache, 4, format!("{:.1}", plain(P::Eq2Gain)));
                set_band_txt(ui, &mut cache, 5, format!("{:.2}", plain(P::Eq2Q)));
                set_band_txt(ui, &mut cache, 6, format!("{:.0}", plain(P::Eq3Freq)));
                set_band_txt(ui, &mut cache, 7, format!("{:.1}", plain(P::Eq3Gain)));
                set_band_txt(ui, &mut cache, 8, format!("{:.2}", plain(P::Eq3Q)));
                set_band_txt(ui, &mut cache, 9, format!("{:.0}", plain(P::Eq4Freq)));
                set_band_txt(ui, &mut cache, 10, format!("{:.1}", plain(P::Eq4Gain)));
                set_band_txt(ui, &mut cache, 11, format!("{:.2}", plain(P::Eq4Q)));
                set_band_txt(ui, &mut cache, 12, format!("{:.0}", plain(P::Eq5Freq)));
                set_band_txt(ui, &mut cache, 13, format!("{:.1}", plain(P::Eq5Gain)));
                set_band_txt(ui, &mut cache, 14, format!("{:.2}", plain(P::Eq5Q)));

                let blend_p = plain(P::Blend);
                let angle_p = plain(P::CfAngle);
                let amount_p = plain(P::CfAmount);
                let gain_p = plain(P::Gain);
                let kt = [
                    format!("{blend_p:.0}%"),
                    format!("{angle_p:.0}°"),
                    format!("{amount_p:.0}%"),
                    format!("{gain_p:.1} dB"),
                ];
                for (i, t) in kt.iter().enumerate() {
                    if changed_str(&mut cache.knob_text[i], t) {
                        let ss = SharedString::from(t.as_str());
                        match i {
                            0 => ui.set_blend_text(ss),
                            1 => ui.set_cf_angle_text(ss),
                            2 => ui.set_cf_amount_text(ss),
                            3 => ui.set_gain_text(ss),
                            _ => {}
                        }
                    }
                }

                // --- input peak meter ---
                let peak_db = shared_for_sync.input_peak.load(Ordering::Relaxed);
                if peak_db > cache.peak_hold {
                    cache.peak_hold = peak_db;
                    cache.peak_hold_ticks = 90;
                } else if cache.peak_hold_ticks > 0 {
                    cache.peak_hold_ticks -= 1;
                } else {
                    cache.peak_hold = (cache.peak_hold - 0.5).max(peak_db);
                }
                let db_txt = if peak_db <= -90.0 {
                    "--".to_string()
                } else {
                    format!("{peak_db:.1} dB")
                };
                if changed_str(&mut cache.input_db, &db_txt) {
                    ui.set_input_db_text(SharedString::from(db_txt.as_str()));
                }
                let pk_txt = if cache.peak_hold <= -90.0 {
                    String::new()
                } else {
                    format!("pk {:.1} dB", cache.peak_hold)
                };
                if changed_str(&mut cache.input_peak, &pk_txt) {
                    ui.set_input_peak_text(SharedString::from(pk_txt.as_str()));
                }

                // --- EQ curve (cached) ---
                let sr = shared_for_sync.sample_rate.load(Ordering::Relaxed).max(1.0);
                let key = eq_curve_key(&params_for_curve, sr);
                if cache.eq_key != Some(key) {
                    cache.eq_key = Some(key);
                    cache.eq_cmds = eq_curve_path(&params_for_curve, sr);
                    ui.set_curve_cmds(SharedString::from(cache.eq_cmds.as_str()));
                }
            }
        },
    )
    .resizable(false)
    .into()
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
