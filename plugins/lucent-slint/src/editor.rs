//! Lucent — lx-slint-editor. Analyzer layout + Vizia feature parity:
//! SMOOTH/SNAP/relay curves + toggle bar, resonance markers, masking bars,
//! goniometer / LED meters / correlation, vault setup overlay.
//!
//! Sync pattern matches Meridian/Equilibrium: `SyncCache` in a Mutex,
//! 33 ms `due()` gating, dirty-checked setters.
//!
//! Display chain (dev parity 2026-07):
//! - Spectrum range SPAN-like −78…−18 dB, log-frequency axis (20 Hz…20 kHz)
//! - SMOOTH toggle → SharedState.spectrum_smooth (1/3-oct display smooth)
//! - SNAP → session max-hold resonance/masking markdown (no FFT phases)

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lx_analysis::{SPECTRUM_BINS, SharedState};
use lx_slint_editor::{LxSlintEditor, PluginContext};
use slint::{ModelRc, SharedString, VecModel};
use truce_core::cast::{discrete_index, discrete_norm};
use truce_core::editor::{Editor, PluginContextReadF32};

use crate::relay_state::RelayState;
use crate::{
    LucentParams, LucentParamsParamId as P, editor_ensure_consumer, read_masking, read_relays,
    read_resonance,
};

slint::include_modules!();

// Lucent compact shell (no footer / no OUT GAIN): 990 × 500
const WINDOW_W: u32 = 990;
const WINDOW_H: u32 = 500;
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Spectrum viewBox — scales to the pit; only aspect affects stroke width.
const PATH_W: f32 = 620.0;
const PATH_H: f32 = 300.0;

/// Match vizia `TICK_INTERVAL` — telemetry / host→UI poll ~30 Hz.
const TICK_INTERVAL: Duration = Duration::from_millis(33);

/// SPAN-matched display range (dev lucent SpectrumConfig).
const SPEC_MIN_DB: f32 = -78.0;
const SPEC_MAX_DB: f32 = -18.0;

const DISPLAY_HOLD_MS: u128 = 500;

/// Relay curve palette (dev editor.rs, cycles by active-relay index).
const RELAY_COLORS: [(u8, u8, u8); 6] = [
    (255, 153, 51),  // rgb(1.0, 0.6, 0.2) orange
    (204, 76, 76),   // rgb(0.8, 0.3, 0.3) red
    (76, 204, 127),  // rgb(0.3, 0.8, 0.5) green
    (102, 153, 255), // rgb(0.4, 0.6, 1.0) blue
    (229, 178, 76),  // rgb(0.9, 0.7, 0.3) yellow
    (178, 102, 204), // rgb(0.7, 0.4, 0.8) purple
];

fn db_to_y(db: f32) -> f32 {
    let range = SPEC_MAX_DB - SPEC_MIN_DB;
    let t = ((SPEC_MAX_DB - db.clamp(SPEC_MIN_DB, SPEC_MAX_DB)) / range).clamp(0.0, 1.0);
    t * PATH_H
}

/// Log-frequency x fraction (20 Hz … 20 kHz) — dev lx-ui SpectrumView axis.
fn log_x(freq: f32) -> f32 {
    ((freq.max(20.0).ln() - 20.0f32.ln()) / (20000.0f32.ln() - 20.0f32.ln())).clamp(0.0, 1.0)
}

/// Header status per analyze mode. Process semantics: 0 = own-only,
/// 1 = hybrid (own + relays), 2 = relay-only. The segmented button's choices
/// `["Own", "Both", "Relay"]` map index → mode directly.
pub(crate) fn mode_status(mode: usize) -> &'static str {
    ["Own bus", "Own + Relays", "Relays"]
        .get(mode)
        .copied()
        .unwrap_or("?")
}

/// 1/3-octave fractional-band smoothing (dev `lx-ui` canvas, default SMOOTH on look).
fn smooth_spectrum_third_octave(spectrum: &[f32], sample_rate: f32) -> Vec<f32> {
    if spectrum.is_empty() {
        return Vec::new();
    }
    let fft_size = SPECTRUM_BINS * 2;
    let log_min = 20.0_f32.ln();
    let log_max = 20000.0_f32.ln();
    let bin_hz = sample_rate / fft_size as f32;
    const DENOM_LOW: f32 = 3.0;
    const DENOM_HIGH: f32 = 20.0;
    const F_LOW: f32 = 500.0;
    const F_HIGH: f32 = 16000.0;
    let taper_lo = F_LOW.ln();
    let taper_hi = F_HIGH.ln();
    const STEPS: usize = 240;
    let len = spectrum.len();

    let power: Vec<f32> = spectrum
        .iter()
        .map(|&db| 10.0_f32.powf(db * 0.1))
        .collect();

    let mut out = Vec::with_capacity(STEPS + 1);
    for i in 0..=STEPS {
        let frac = i as f32 / STEPS as f32;
        let ln_fc = log_min + (log_max - log_min) * frac;
        let fc = ln_fc.exp();
        let t = ((ln_fc - taper_lo) / (taper_hi - taper_lo)).clamp(0.0, 1.0);
        let denom = DENOM_LOW + (DENOM_HIGH - DENOM_LOW) * t;
        let half = 2.0_f32.powf(1.0 / (2.0 * denom));
        const MIN_BIN: f32 = 1.0;
        let lo = (fc / half / bin_hz).clamp(MIN_BIN, (len - 1) as f32);
        let hi = (fc * half / bin_hz).clamp(MIN_BIN, (len - 1) as f32);
        let avg_power = if hi - lo >= 1.0 {
            let i0 = lo.floor() as usize;
            let i1 = hi.floor() as usize;
            let mut sum = 0.0f32;
            if i0 == i1 {
                sum = power[i0] * (hi - lo);
            } else {
                sum += power[i0] * ((i0 + 1) as f32 - lo);
                for p in &power[i0 + 1..i1] {
                    sum += *p;
                }
                sum += power[i1] * (hi - i1 as f32);
            }
            sum / (hi - lo)
        } else {
            let pos = (fc / bin_hz).clamp(MIN_BIN, (len - 1) as f32);
            let i0 = pos.floor() as usize;
            let i1 = (i0 + 1).min(len - 1);
            let t_bin = pos - i0 as f32;
            power[i0] * (1.0 - t_bin) + power[i1] * t_bin
        };
        out.push((10.0 * avg_power.max(1e-12).log10()).clamp(-90.0, 12.0));
    }
    if out.len() >= 3 {
        let raw = out.clone();
        for i in 1..out.len().saturating_sub(1) {
            out[i] = raw[i - 1] * 0.25 + raw[i] * 0.5 + raw[i + 1] * 0.25;
        }
    }
    out
}

fn build_curve(out: &mut String, pts: &[(f32, f32)], fill: bool) {
    if pts.len() < 2 {
        return;
    }
    let base = PATH_H + 2.0;
    if fill {
        out.push_str(&format!("M -2.0 {base:.1} L {:.1} {:.1}", pts[0].0, pts[0].1));
    } else {
        out.push_str(&format!("M {:.1} {:.1}", pts[0].0, pts[0].1));
    }
    for &(x, y) in &pts[1..] {
        out.push_str(&format!(" L {x:.1} {y:.1}"));
    }
    if fill {
        let lx = pts.last().unwrap().0;
        out.push_str(&format!(
            " L {lx:.1} {base:.1} L {:.1} {base:.1} Z",
            PATH_W + 2.0
        ));
    }
}

/// Spectrum path into `out` (reused buffer): SMOOTH on = 1/3-oct smoothed
/// (241 log-spaced points); off = raw bins on a log-frequency axis, driven by
/// `sample_rate` (no hardcoded 44.1 k). `fill` closes along the baseline
/// (own curve); relay curves are open stroke-only.
fn spectrum_path(bins: &[f32], sample_rate: f32, smooth: bool, fill: bool, out: &mut String) {
    out.clear();
    if bins.len() < 2 {
        return;
    }
    let n = bins.len();
    let fft_size = (n * 2) as f32;
    if smooth {
        let sm = smooth_spectrum_third_octave(bins, sample_rate);
        let denom = sm.len().saturating_sub(1).max(1) as f32;
        let pts: Vec<(f32, f32)> = sm
            .iter()
            .enumerate()
            .map(|(i, &db)| (i as f32 / denom * PATH_W, db_to_y(db)))
            .collect();
        build_curve(out, &pts, fill);
    } else {
        let mut pts: Vec<(f32, f32)> = Vec::with_capacity(n);
        for (i, &db) in bins.iter().enumerate() {
            let freq = i as f32 * sample_rate / fft_size;
            if freq < 20.0 {
                continue;
            }
            if freq > 20000.0 {
                break;
            }
            pts.push((log_x(freq) * PATH_W, db_to_y(db)));
        }
        build_curve(out, &pts, fill);
    }
}

/// Masking overlay bars from the full persistence-gated masking map
/// (dev SpectrumView `masking` overlay), log-x like the spectrum.
fn mask_bars_path(map: &[f32], sample_rate: f32, out: &mut String) {
    out.clear();
    let n = map.len();
    if n < 2 {
        return;
    }
    let fft_size = (n * 2) as f32;
    for (j, &db) in map.iter().enumerate() {
        if db <= -89.0 {
            continue;
        }
        let f0 = j as f32 * sample_rate / fft_size;
        if f0 < 20.0 {
            continue;
        }
        if f0 > 20000.0 {
            break;
        }
        let f1 = (j + 1) as f32 * sample_rate / fft_size;
        let x = log_x(f0) * PATH_W;
        let w = ((log_x(f1) - log_x(f0)) * PATH_W).max(1.0);
        let y0 = db_to_y(db);
        out.push_str(&format!(
            "M {x:.1} {y0:.1} L {xr:.1} {y0:.1} L {xr:.1} {H:.0} L {x:.1} {H:.0} Z ",
            xr = x + w,
            H = PATH_H
        ));
    }
}

/// Resonance marker: vertical line full height + diamond at top (dev canvas).
/// Alpha is applied per-marker in Slint: (score/20).clamp(0.2, 0.9).
fn res_marker_cmds(bin: usize, sample_rate: f32) -> String {
    let fft_size = (SPECTRUM_BINS * 2) as f32;
    let freq = bin as f32 * sample_rate / fft_size;
    let x = log_x(freq) * PATH_W;
    format!(
        "M {x:.1} 0 L {x:.1} {H:.0} M {x:.1} 0 L {xr:.1} 4 L {x:.1} 8 L {xl:.1} 4 Z",
        H = PATH_H,
        xr = x + 4.0,
        xl = x - 4.0
    )
}

/// Goniometer path (M/S rotation) — Meridian port, visual auto-gain happens
/// in `process()` (`scope_vis_envelope`).
fn gonio_path(shared: &SharedState, w: f32, h: f32, out: &mut String) {
    out.clear();
    let (samples, write_pos) = {
        let pos = shared.scope_write_pos.load(Ordering::Relaxed);
        let samples = match shared.scope_samples.try_lock() {
            Ok(v) => v.clone(),
            Err(_) => return,
        };
        (samples, pos)
    };
    if samples.is_empty() {
        return;
    }

    let points_to_take = 512usize.min(lx_analysis::SCOPE_BUFFER_LEN);
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
            out.push_str(&format!("M {x:.1} {y:.1}"));
        } else {
            out.push_str(&format!(" L {x:.1} {y:.1}"));
        }
    }
}

/// Map peak dB → 0..1 over −60..+6 dB (LxLedPeakMeter range).
fn peak_norm(db: f32) -> f32 {
    ((db + 60.0) / 66.0).clamp(0.0, 1.0)
}

fn fmt_db(v: f32) -> String {
    if v <= -60.0 {
        "-inf".to_string()
    } else {
        format!("{v:.1}")
    }
}

fn max_hold_score(map: &mut HashMap<usize, f32>, bin: usize, score: f32) {
    map.entry(bin)
        .and_modify(|s| {
            if score > *s {
                *s = score;
            }
        })
        .or_insert(score);
}

fn max_hold_named(
    map: &mut HashMap<usize, (f32, Vec<String>)>,
    bin: usize,
    score: f32,
    names: &[String],
) {
    map.entry(bin)
        .and_modify(|(s, n)| {
            if score > *s {
                *s = score;
                *n = names.to_vec();
            }
        })
        .or_insert((score, names.to_vec()));
}

// ─── Analyzer text panels (dev editor.rs ports) ─────────────────────────────

/// Top-3 own + top-3 group resonance lines.
fn format_resonance_text(
    acc_own: &HashMap<usize, f32>,
    acc_relay: &HashMap<usize, (f32, Vec<String>)>,
    sample_rate: f32,
) -> String {
    if acc_own.is_empty() && acc_relay.is_empty() {
        return "No resonances detected".to_string();
    }
    let fft_size = (SPECTRUM_BINS * 2) as f32;

    let mut own: Vec<(usize, f32)> = acc_own.iter().map(|(&bin, &score)| (bin, score)).collect();
    own.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut relay: Vec<(usize, f32, Vec<String>)> = acc_relay
        .iter()
        .map(|(&bin, (score, names))| (bin, *score, names.clone()))
        .collect();
    relay.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut lines = Vec::new();
    for (bin, score) in own.iter().take(3) {
        let freq = *bin as f32 * sample_rate / fft_size;
        lines.push(format!("Own: {freq:.0} Hz {score:.1}"));
    }
    for (bin, score, contributors) in relay.iter().take(3) {
        let freq = *bin as f32 * sample_rate / fft_size;
        if contributors.is_empty() {
            lines.push(format!("Group: {freq:.0} Hz {score:.1}"));
        } else {
            lines.push(format!(
                "Group: {freq:.0} Hz {score:.1} ({})",
                contributors.join(", ")
            ));
        }
    }
    lines.join("\n")
}

/// Top-3 masking collision lines.
fn format_masking_text(
    mode: usize,
    acc: &HashMap<usize, (f32, Vec<String>)>,
    relays_empty: bool,
    sample_rate: f32,
) -> String {
    if mode == 0 {
        return "Standalone — no masking".to_string();
    }
    if acc.is_empty() || relays_empty {
        return "No masking detected".to_string();
    }
    let fft_size = (SPECTRUM_BINS * 2) as f32;
    let mut peaks: Vec<(usize, f32, Vec<String>)> = acc
        .iter()
        .map(|(&bin, (db, names))| (bin, *db, names.clone()))
        .collect();
    peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    peaks.truncate(3);
    peaks
        .iter()
        .map(|(bin, db, contributors)| {
            let freq = *bin as f32 * sample_rate / fft_size;
            if contributors.is_empty() {
                format!("{freq:.0} Hz  {db:.1} dB")
            } else {
                format!("{freq:.0} Hz  {db:.1} dB ({})", contributors.join("-"))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ─── SNAP (session max-hold markdown, dev parity) ───────────────────────────

fn snap_filename(vault_path: &str) -> String {
    let dir = std::path::Path::new(vault_path);
    let mut max_n = 0u32;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let s = e.file_name().to_string_lossy().into_owned();
            if let Some(inner) = s
                .strip_prefix("SNAPSHOT-")
                .and_then(|r| r.strip_suffix(".md"))
                && let Ok(n) = inner.parse::<u32>()
            {
                max_n = max_n.max(n);
            }
        }
    }
    format!("SNAPSHOT-{:03}.md", max_n + 1)
}

fn snap_markdown(
    instance_name: &str,
    res_own: &[(usize, f32)],
    res_relay: &[(usize, f32, Vec<String>)],
    masking: &[(usize, f32, Vec<String>)],
    sr: f32,
    sensitivity_pct: f32,
    mode: usize,
) -> String {
    let fft_sz = 2048.0;
    let bin_hz = sr / fft_sz;
    let mode_name = match mode {
        0 => "standalone",
        2 => "relay",
        _ => "hybrid",
    };
    let name = if instance_name.trim().is_empty() {
        "Lucent"
    } else {
        instance_name.trim()
    };
    let name_yaml = if name.chars().any(|c| matches!(c, ':' | '#' | '"' | '\'' | '\n'))
        || name.contains(": ")
    {
        format!("\"{}\"", name.replace('"', "\\\""))
    } else {
        name.to_string()
    };

    let res_rows = {
        let mut rows = Vec::new();
        for &(bin, score) in res_own {
            let hz = bin as f32 * bin_hz;
            rows.push(format!("| Own ({name}) | {hz:.0} | {score:.2} | |"));
        }
        for (bin, score, names) in res_relay {
            let hz = *bin as f32 * bin_hz;
            rows.push(format!(
                "| Group | {hz:.0} | {score:.2} | {} |",
                names.join(", ")
            ));
        }
        if rows.is_empty() {
            "_No resonances detected at current sensitivity._".to_string()
        } else {
            format!(
                "| Source | Hz | Score | Contributors |\n|--------|-----|-------|--------------|\n{}",
                rows.join("\n")
            )
        }
    };
    let mask_rows = if mode == 0 {
        "_Standalone — no masking._".to_string()
    } else if masking.is_empty() {
        "_No masking areas detected at current sensitivity._".to_string()
    } else {
        let rows = masking
            .iter()
            .map(|(bin, db, names)| {
                let hz = *bin as f32 * bin_hz;
                format!("| {hz:.0} | {db:.1} | {} |", names.join(" / "))
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("| Hz | Amount (dB) | Maskers |\n|----|-------------|---------|\n{rows}")
    };

    format!(
        "---\n\
         plugin: lucent\n\
         name: {name_yaml}\n\
         type: snapshot\n\
         sample_rate: {sr:.0}\n\
         analyze_mode: {mode_name}\n\
         sensitivity_pct: {sensitivity_pct:.0}\n\
         ---\n\n\
         # Lucent Snapshot — {name}\n\n\
         > Resonance score = detector strength (Sensitivity-gated). \
         Masking dB = collision level after ERB smooth + persistence gate. \
         Lists are **session max-hold** over analysis until SNAP (not one frame, not UI top-N). \
         `name` = instance label from the Name field (multi-Lucent).\n\n\
         ## Resonance\n{res_rows}\n\n\
         ## Masking\n{mask_rows}\n"
    )
}

// ─── Sync cache (Meridian pattern: 33 ms due() + dirty mirrors) ─────────────

struct SyncCache {
    last_tick: Instant,
    /// First open must fill UI even if Instant says "not due".
    primed: bool,
    // Dirty mirrors — only call Slint setters when value changes.
    mode: f32,
    sens: f32,
    sens_text_q: f32,
    res_active: Option<bool>,
    mask_active: Option<bool>,
    bypass: Option<bool>,
    smooth: Option<bool>,
    meter_l: f32,
    meter_r: f32,
    hold_l: f32,
    hold_r: f32,
    corr: f32,
    balance: f32,
    hold_l_q: f32,
    hold_r_q: f32,
    // Relay list (EMA + toggles) + last-pushed models.
    relay_state: RelayState,
    relay_names: Vec<String>,
    relay_actives: Vec<bool>,
    relay_curves_shown: bool,
    res_markers_shown: bool,
    mask_shown: bool,
    // Display (500 ms window) + session (SNAP) max-hold accumulators.
    display_own: HashMap<usize, f32>,
    display_relay: HashMap<usize, (f32, Vec<String>)>,
    display_mask: HashMap<usize, (f32, Vec<String>)>,
    snap_res_own: HashMap<usize, f32>,
    snap_res_relay: HashMap<usize, (f32, Vec<String>)>,
    snap_mask: HashMap<usize, (f32, Vec<String>)>,
    display_window_start: Instant,
    snap_blink: u32,
    snap_label: String,
    vault_path: Option<String>,
    // Reused per-tick path buffers.
    path_scratch: String,
    mask_scratch: String,
    gonio_scratch: String,
}

impl SyncCache {
    fn new(vault_path: Option<String>) -> Self {
        Self {
            last_tick: Instant::now()
                .checked_sub(TICK_INTERVAL)
                .unwrap_or_else(Instant::now),
            primed: false,
            mode: f32::NAN,
            sens: f32::NAN,
            sens_text_q: f32::NAN,
            res_active: None,
            mask_active: None,
            bypass: None,
            smooth: None,
            meter_l: f32::NAN,
            meter_r: f32::NAN,
            hold_l: f32::NAN,
            hold_r: f32::NAN,
            corr: f32::NAN,
            balance: f32::NAN,
            hold_l_q: f32::NAN,
            hold_r_q: f32::NAN,
            relay_state: RelayState::default(),
            relay_names: Vec::new(),
            relay_actives: Vec::new(),
            relay_curves_shown: false,
            res_markers_shown: false,
            mask_shown: false,
            display_own: HashMap::new(),
            display_relay: HashMap::new(),
            display_mask: HashMap::new(),
            snap_res_own: HashMap::new(),
            snap_res_relay: HashMap::new(),
            snap_mask: HashMap::new(),
            display_window_start: Instant::now(),
            snap_blink: 0,
            snap_label: String::new(),
            vault_path,
            path_scratch: String::new(),
            mask_scratch: String::new(),
            gonio_scratch: String::new(),
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

pub fn build_editor(params: Arc<LucentParams>) -> Box<dyn Editor> {
    let shared = params.shared.clone();
    let instance_key = Arc::as_ptr(&params) as usize;
    let snap_request = Arc::new(AtomicBool::new(false));

    let init_vp = lx_analysis::load_config("Lucent").vault_path;
    let sync_cache = Arc::new(Mutex::new(SyncCache::new(init_vp.clone())));

    LxSlintEditor::new(
        params.clone(),
        (WINDOW_W, WINDOW_H),
        {
            let params = params.clone();
            let shared = shared.clone();
            let snap_request = snap_request.clone();
            let sync_cache = sync_cache.clone();
            let init_vp = init_vp.clone();
            move |state: PluginContext<LucentParams>| {
                let ui = LucentUi::new().expect("LucentUi::new");

                ui.set_version(SharedString::from(VERSION));
                let name0 = params.name.read().map(|s| s.clone()).unwrap_or_default();
                ui.set_display_name(SharedString::from(name0.as_str()));
                ui.set_spectrum_smooth(shared.spectrum_smooth.load(Ordering::Relaxed));
                if let Some(ref vp) = init_vp {
                    ui.set_vault_path(SharedString::from(vp.as_str()));
                }

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

                let shared_smooth = shared.clone();
                ui.on_spectrum_smooth_changed(move |on: bool| {
                    shared_smooth.spectrum_smooth.store(on, Ordering::Relaxed);
                });

                // RESET: peak holds re-arm in process(); clear the display
                // atomics immediately too (dev parity).
                let shared_reset = shared.clone();
                ui.on_reset_peaks(move || {
                    shared_reset.reset_peak.store(true, Ordering::Release);
                    shared_reset.peak_hold.store(-100.0, Ordering::Release);
                    shared_reset.peak_hold_l.store(-100.0, Ordering::Release);
                    shared_reset.peak_hold_r.store(-100.0, Ordering::Release);
                });

                // Relay toggle bar — flips the slot-stable toggle and pushes
                // the bitmask straight to the audio thread.
                let cache_t = sync_cache.clone();
                let shared_t = shared.clone();
                ui.on_relay_toggled(move |idx: f32| {
                    if let Ok(mut c) = cache_t.lock()
                        && let Some(r) = c.relay_state.relays.get_mut(idx.max(0.0) as usize)
                    {
                        r.active = !r.active;
                        // DRIVEN bit: distinguish "all off" from "no preference".
                        shared_t.relay_active_mask.store(
                            c.relay_state.active_mask() | lx_analysis::RELAY_MASK_DRIVEN,
                            Ordering::Release,
                        );
                    }
                });

                // SNAP: needs a vault path; without one open the setup overlay.
                let cache_s = sync_cache.clone();
                let snap_arm = snap_request.clone();
                let snap_ui = ui.as_weak();
                ui.on_snap_clicked(move || {
                    let no_vault = cache_s
                        .lock()
                        .ok()
                        .map(|c| c.vault_path.as_ref().is_none_or(|v| v.is_empty()))
                        .unwrap_or(true);
                    if no_vault {
                        if let Some(ui) = snap_ui.upgrade() {
                            ui.set_vault_setup_open(true);
                        }
                        return;
                    }
                    snap_arm.store(true, Ordering::Release);
                });

                let cache_v = sync_cache.clone();
                ui.on_vault_path_changed(move |path: SharedString| {
                    let path = path.to_string().trim().to_string();
                    let new_vp = if path.is_empty() { None } else { Some(path) };
                    if let Ok(mut c) = cache_v.lock() {
                        c.vault_path = new_vp.clone();
                    }
                    let mut cfg = lx_analysis::load_config("Lucent");
                    cfg.vault_path = new_vp;
                    let _ = lx_analysis::save_config("Lucent", &cfg);
                });

                // Vault Setup PASTE: write draft path only (vault_setup_path).
                let paste_ui = ui.as_weak();
                ui.on_vault_paste_requested(move || {
                    let Some(ui) = paste_ui.upgrade() else { return };
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

                ui
            }
        },
        {
            let shared_sync = shared.clone();
            let params_sync = params.clone();
            let snap_req = snap_request.clone();
            let sync_cache = sync_cache.clone();
            move |ui: &LucentUi, state: &PluginContext<LucentParams>| {
                let Ok(mut cache) = sync_cache.lock() else {
                    return;
                };
                if !cache.due() {
                    return;
                }

                editor_ensure_consumer(&params_sync, &shared_sync);

                // --- host → UI params (dirty) ---
                let mode = discrete_index(
                    PluginContextReadF32::get_param(state, P::AnalyzeMode) as f64,
                    3,
                );
                if changed_f32(&mut cache.mode, mode as f32) {
                    ui.set_analyze_mode(mode as f32);
                    ui.set_status_line(SharedString::from(mode_status(mode)));
                }
                let res_active = PluginContextReadF32::get_param(state, P::ResonanceActive) > 0.5;
                if changed_bool(&mut cache.res_active, res_active) {
                    ui.set_resonance_active(res_active);
                }
                let mask_active = PluginContextReadF32::get_param(state, P::MaskingActive) > 0.5;
                if changed_bool(&mut cache.mask_active, mask_active) {
                    ui.set_masking_active(mask_active);
                }
                let bypass = PluginContextReadF32::get_param(state, P::BypassActive) > 0.5;
                if changed_bool(&mut cache.bypass, bypass) {
                    ui.set_bypass_active(bypass);
                }
                let sens = PluginContextReadF32::get_param(state, P::Sensitivity);
                if changed_f32(&mut cache.sens, sens) {
                    ui.set_sensitivity(sens);
                }
                let plain = state.params().sensitivity.raw_target() as f32;
                let plain_q = (plain * 10.0).round() / 10.0;
                if changed_f32(&mut cache.sens_text_q, plain_q) {
                    ui.set_sensitivity_text(SharedString::from(format!("{plain:.0}%")));
                }
                let smooth = shared_sync.spectrum_smooth.load(Ordering::Relaxed);
                if changed_bool(&mut cache.smooth, smooth) {
                    ui.set_spectrum_smooth(smooth);
                }

                // --- relay list (EMA + toggles + mask) ---
                if mode != 0 {
                    cache.relay_state.sync(&read_relays(instance_key));
                    // DRIVEN | slot-bits: all-off is not the same as mask==0.
                    shared_sync.relay_active_mask.store(
                        cache.relay_state.active_mask() | lx_analysis::RELAY_MASK_DRIVEN,
                        Ordering::Release,
                    );
                } else {
                    cache.relay_state.clear();
                    // Own mode: no UI preference (process ignores relays anyway).
                    shared_sync
                        .relay_active_mask
                        .store(0, Ordering::Release);
                }

                let names: Vec<String> = cache
                    .relay_state
                    .relays
                    .iter()
                    .map(|r| r.name.clone())
                    .collect();
                if names != cache.relay_names {
                    cache.relay_names = names;
                    let v: Vec<SharedString> = cache
                        .relay_names
                        .iter()
                        .map(|s| SharedString::from(s.as_str()))
                        .collect();
                    ui.set_relay_names(ModelRc::new(VecModel::from(v)));
                }
                let actives: Vec<bool> = cache
                    .relay_state
                    .relays
                    .iter()
                    .map(|r| r.active)
                    .collect();
                if actives != cache.relay_actives {
                    cache.relay_actives = actives.clone();
                    ui.set_relay_actives(ModelRc::new(VecModel::from(actives)));
                }

                // --- spectrum + overlays ---
                let sr = shared_sync.sample_rate.load(Ordering::Relaxed).max(1.0);
                {
                    let bins = shared_sync
                        .spectrum_avg
                        .try_lock()
                        .ok()
                        .map(|g| g.clone())
                        .unwrap_or_else(|| vec![-90.0; SPECTRUM_BINS]);
                    spectrum_path(&bins, sr, smooth, true, &mut cache.path_scratch);
                }
                ui.set_spectrum_cmds(SharedString::from(cache.path_scratch.as_str()));

                // Relay curves: active relays only, palette cycles by active
                // index (dev parity).
                let mut curves: Vec<RelayCurve> = Vec::new();
                if mode != 0 {
                    for (i, r) in cache
                        .relay_state
                        .relays
                        .iter()
                        .filter(|r| r.active)
                        .enumerate()
                    {
                        let mut cmds = String::new();
                        spectrum_path(&r.spectrum, sr, smooth, false, &mut cmds);
                        if cmds.is_empty() {
                            continue;
                        }
                        let (rr, gg, bb) = RELAY_COLORS[i % RELAY_COLORS.len()];
                        curves.push(RelayCurve {
                            cmds: SharedString::from(cmds),
                            col: slint::Color::from_rgb_u8(rr, gg, bb),
                        });
                    }
                }
                let curves_shown = !curves.is_empty();
                if curves_shown || cache.relay_curves_shown {
                    ui.set_relay_curves(ModelRc::new(VecModel::from(curves)));
                    cache.relay_curves_shown = curves_shown;
                }

                // Resonance markers — display-gated by the Resonance param
                // (analysis keeps running either way, dev parity).
                let lists = read_resonance(instance_key);
                let mut markers: Vec<ResMarker> = Vec::new();
                if res_active {
                    for &(bin, score) in &lists.own {
                        markers.push(ResMarker {
                            cmds: SharedString::from(res_marker_cmds(bin, sr)),
                            a: (score / 20.0).clamp(0.2, 0.9),
                        });
                    }
                    for (bin, score, _) in &lists.relay {
                        markers.push(ResMarker {
                            cmds: SharedString::from(res_marker_cmds(*bin, sr)),
                            a: (*score / 20.0).clamp(0.2, 0.9),
                        });
                    }
                }
                let markers_shown = !markers.is_empty();
                if markers_shown || cache.res_markers_shown {
                    ui.set_res_markers(ModelRc::new(VecModel::from(markers)));
                    cache.res_markers_shown = markers_shown;
                }

                // Masking bars — display-gated by the Masking param (dev:
                // `show_masking && (!relays.is_empty() || mode == RELAY)`).
                let mask_on =
                    mask_active && mode != 0 && (!cache.relay_state.relays.is_empty() || mode == 2);
                if mask_on {
                    let map = shared_sync
                        .masking_map
                        .try_lock()
                        .ok()
                        .map(|m| m.clone())
                        .unwrap_or_default();
                    mask_bars_path(&map, sr, &mut cache.mask_scratch);
                    ui.set_mask_cmds(SharedString::from(cache.mask_scratch.as_str()));
                    cache.mask_shown = true;
                } else if cache.mask_shown {
                    ui.set_mask_cmds(SharedString::new());
                    cache.mask_shown = false;
                }

                // --- display (500 ms) + session (SNAP) max-hold ---
                let masking_top = read_masking(instance_key);
                for &(bin, score) in &lists.own {
                    max_hold_score(&mut cache.display_own, bin, score);
                    max_hold_score(&mut cache.snap_res_own, bin, score);
                }
                for (bin, score, names) in &lists.relay {
                    max_hold_named(&mut cache.display_relay, *bin, *score, names);
                    max_hold_named(&mut cache.snap_res_relay, *bin, *score, names);
                }
                for (bin, db, names) in &masking_top {
                    max_hold_named(&mut cache.display_mask, *bin, *db, names);
                    max_hold_named(&mut cache.snap_mask, *bin, *db, names);
                }

                if cache.display_window_start.elapsed().as_millis() >= DISPLAY_HOLD_MS {
                    let rt = if res_active {
                        format_resonance_text(&cache.display_own, &cache.display_relay, sr)
                    } else {
                        "Off".to_string()
                    };
                    ui.set_resonance_text(SharedString::from(rt));
                    let mt = if !mask_active {
                        "Off".to_string()
                    } else {
                        format_masking_text(
                            mode,
                            &cache.display_mask,
                            cache.relay_state.relays.is_empty(),
                            sr,
                        )
                    };
                    ui.set_masking_text(SharedString::from(mt));
                    cache.display_own.clear();
                    cache.display_relay.clear();
                    cache.display_mask.clear();
                    cache.display_window_start = Instant::now();
                }

                // --- SNAP: session max-hold markdown into the vault ---
                if snap_req.swap(false, Ordering::AcqRel) {
                    cache.snap_blink = 72;
                    if let Some(vp) = cache.vault_path.clone().filter(|v| !v.is_empty()) {
                        let _ = std::fs::create_dir_all(&vp);
                        let mut res_own: Vec<(usize, f32)> = cache
                            .snap_res_own
                            .iter()
                            .map(|(&bin, &score)| (bin, score))
                            .collect();
                        res_own
                            .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                        let mut res_relay: Vec<(usize, f32, Vec<String>)> = cache
                            .snap_res_relay
                            .iter()
                            .map(|(&bin, (score, names))| (bin, *score, names.clone()))
                            .collect();
                        res_relay
                            .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                        let mut mask_v: Vec<(usize, f32, Vec<String>)> = cache
                            .snap_mask
                            .iter()
                            .map(|(&bin, (db, names))| (bin, *db, names.clone()))
                            .collect();
                        mask_v
                            .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                        let instance_name = params_sync
                            .name
                            .try_read()
                            .map(|n| n.clone())
                            .unwrap_or_default();
                        let md = snap_markdown(
                            &instance_name,
                            &res_own,
                            &res_relay,
                            &mask_v,
                            sr,
                            plain,
                            mode,
                        );
                        let fname = snap_filename(&vp);
                        let path = std::path::Path::new(&vp).join(&fname);
                        if std::fs::write(&path, &md).is_ok() {
                            ui.set_snap_status(SharedString::from(format!("Wrote {fname}")));
                        }
                        cache.snap_res_own.clear();
                        cache.snap_res_relay.clear();
                        cache.snap_mask.clear();
                    }
                }
                if cache.snap_blink > 0 {
                    cache.snap_blink -= 1;
                    if cache.snap_blink == 0 {
                        ui.set_snap_status(SharedString::new());
                    }
                }
                let label = if cache.snap_blink > 0 {
                    "ANALYZE..."
                } else if cache.vault_path.as_ref().is_none_or(|v| v.is_empty()) {
                    "SET VAULT"
                } else {
                    "SNAP"
                };
                if cache.snap_label != label {
                    cache.snap_label = label.to_string();
                    ui.set_snap_label(SharedString::from(label));
                }

                // --- meters / correlation / balance ---
                let peak = shared_sync.input_peak.load(Ordering::Relaxed);
                let pl = shared_sync.output_peak_l.load(Ordering::Relaxed);
                let pr = shared_sync.output_peak_r.load(Ordering::Relaxed);
                let pl = if pl <= -90.0 { peak } else { pl };
                let pr = if pr <= -90.0 { peak } else { pr };
                let ml = peak_norm(pl);
                let mr = peak_norm(pr);
                if changed_f32(&mut cache.meter_l, ml) {
                    ui.set_meter_l(ml);
                }
                if changed_f32(&mut cache.meter_r, mr) {
                    ui.set_meter_r(mr);
                }
                let hold_l_db = shared_sync.peak_hold_l.load(Ordering::Relaxed);
                let hold_r_db = shared_sync.peak_hold_r.load(Ordering::Relaxed);
                let hl = peak_norm(hold_l_db);
                let hr = peak_norm(hold_r_db);
                if changed_f32(&mut cache.hold_l, hl) {
                    ui.set_hold_l(hl);
                }
                if changed_f32(&mut cache.hold_r, hr) {
                    ui.set_hold_r(hr);
                }
                let hold_l_q = (hold_l_db * 10.0).round() / 10.0;
                let hold_r_q = (hold_r_db * 10.0).round() / 10.0;
                if changed_f32(&mut cache.hold_l_q, hold_l_q) {
                    ui.set_peak_l_text(SharedString::from(fmt_db(hold_l_db)));
                }
                if changed_f32(&mut cache.hold_r_q, hold_r_q) {
                    ui.set_peak_r_text(SharedString::from(fmt_db(hold_r_db)));
                }
                let balance = shared_sync.balance.load(Ordering::Relaxed);
                if changed_f32(&mut cache.balance, balance) {
                    ui.set_balance(balance);
                }
                let corr = shared_sync.phase_correlation.load(Ordering::Relaxed);
                if changed_f32(&mut cache.corr, corr) {
                    ui.set_correlation(corr);
                    ui.set_corr_text(SharedString::from(format!("correlation: {corr:.2}")));
                }

                // --- goniometer ---
                gonio_path(&shared_sync, 139.0, 139.0, &mut cache.gonio_scratch);
                ui.set_gonio_path(SharedString::from(cache.gonio_scratch.as_str()));
            }
        },
    )
    .resizable(true)
    .into()
}

#[cfg(test)]
mod tests {
    use super::mode_status;

    /// Process semantics: 0 = own-only, 1 = hybrid (own + relays),
    /// 2 = relay-only. Slint segmented choices are ["Own", "Both", "Relay"]
    /// (index → mode directly) — this pins the status line to the same map.
    #[test]
    fn mode_labels_match_process_semantics() {
        assert_eq!(mode_status(0), "Own bus");
        assert_eq!(mode_status(1), "Own + Relays");
        assert_eq!(mode_status(2), "Relays");
    }
}
