//! Lucent Slint UI — analyzer layout, not a Vizia port.
//! truce-slint software renderer.
//!
//! Display chain (dev parity 2026-07):
//! - Spectrum range SPAN-like −78…−18 dB
//! - SMOOTH toggle → SharedState.spectrum_smooth (1/3-oct display smooth)
//! - SNAP → session max-hold resonance/masking markdown (no FFT phases)

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use slint::SharedString;
use truce::prelude::*;
use truce_core::cast::{discrete_index, discrete_norm};
use truce_core::editor::{Editor, PluginContextReadF32};
use truce_slint::{PluginContext, SlintEditor, SyncFn};

use crate::{
    editor_ensure_consumer, read_masking, read_resonance, LucentParams, LucentParamsParamId as P,
};
use lx_analysis::{get_plugin_dir, relay_hub, SPECTRUM_BINS};

slint::include_modules!();

const WINDOW_W: u32 = 990;
const WINDOW_H: u32 = 550;
const VERSION: &str = env!("CARGO_PKG_VERSION");
const PATH_W: f32 = 900.0;
const PATH_H: f32 = 400.0;

/// SPAN-matched display range (dev lucent SpectrumConfig).
const SPEC_MIN_DB: f32 = -78.0;
const SPEC_MAX_DB: f32 = -18.0;

const DISPLAY_HOLD_MS: u128 = 500;

fn db_to_y(db: f32) -> f32 {
    let range = SPEC_MAX_DB - SPEC_MIN_DB;
    let t = ((SPEC_MAX_DB - db.clamp(SPEC_MIN_DB, SPEC_MAX_DB)) / range).clamp(0.0, 1.0);
    t * PATH_H
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

/// Raw path through log-spaced bins (SMOOTH off / SPAN peak height).
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

/// Per-editor-session accumulators (UI thread only).
struct EditorAccum {
    snap_res_own: HashMap<usize, f32>,
    snap_res_relay: HashMap<usize, (f32, Vec<String>)>,
    snap_mask: HashMap<usize, (f32, Vec<String>)>,
    display_own: HashMap<usize, f32>,
    display_relay: HashMap<usize, (f32, Vec<String>)>,
    display_mask: HashMap<usize, (f32, Vec<String>)>,
    display_window_start: Instant,
    snap_blink: u32,
}

impl Default for EditorAccum {
    fn default() -> Self {
        Self {
            snap_res_own: HashMap::new(),
            snap_res_relay: HashMap::new(),
            snap_mask: HashMap::new(),
            display_own: HashMap::new(),
            display_relay: HashMap::new(),
            display_mask: HashMap::new(),
            display_window_start: Instant::now(),
            snap_blink: 0,
        }
    }
}

pub fn build_editor(params: Arc<LucentParams>) -> Box<dyn Editor> {
    let shared = params.shared.clone();
    let instance_key = Arc::as_ptr(&params) as usize;
    let snap_request = Arc::new(AtomicBool::new(false));
    let accum = Arc::new(Mutex::new(EditorAccum::default()));

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
            ui.set_spectrum_smooth(shared.spectrum_smooth.load(Ordering::Relaxed));
            ui.set_db_top(SharedString::from(format!("{SPEC_MAX_DB:.0} dB")));
            ui.set_db_mid(SharedString::from(format!(
                "{:.0}",
                (SPEC_MAX_DB + SPEC_MIN_DB) * 0.5
            )));
            ui.set_db_bot(SharedString::from(format!("{SPEC_MIN_DB:.0}")));

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
                shared_smooth
                    .spectrum_smooth
                    .store(on, Ordering::Relaxed);
            });

            let snap_arm = snap_request.clone();
            ui.on_snap_clicked(move || {
                snap_arm.store(true, Ordering::Release);
            });

            let shared_sync = shared.clone();
            let params_sync = params.clone();
            let snap_req = snap_request.clone();
            let accum_sync = accum.clone();
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

                let smooth = shared_sync.spectrum_smooth.load(Ordering::Relaxed);
                ui.set_spectrum_smooth(smooth);

                // Spectrum path
                let bins = shared_sync
                    .spectrum_avg
                    .try_lock()
                    .ok()
                    .map(|g| g.clone())
                    .unwrap_or_else(|| vec![-90.0; SPECTRUM_BINS]);
                let sample_rate = shared_sync.sample_rate.load(Ordering::Relaxed).max(1.0);
                let path = if smooth {
                    let sm = smooth_spectrum_third_octave(&bins, sample_rate);
                    spectrum_fill_path(&sm)
                } else {
                    spectrum_fill_path(&bins)
                };
                ui.set_spectrum_cmds(SharedString::from(path));

                let mask = read_masking(instance_key);
                ui.set_mask_cmds(SharedString::from(mask_bars_path(&mask)));

                let res = read_resonance(instance_key);

                // Session + display max-hold (dev parity)
                if let Ok(mut acc) = accum_sync.lock() {
                    for &(bin, score) in &res.own {
                        max_hold_score(&mut acc.display_own, bin, score);
                        max_hold_score(&mut acc.snap_res_own, bin, score);
                    }
                    for (bin, score, names) in &res.relay {
                        max_hold_named(&mut acc.display_relay, *bin, *score, names);
                        max_hold_named(&mut acc.snap_res_relay, *bin, *score, names);
                    }
                    for (bin, db, names) in &mask {
                        max_hold_named(&mut acc.display_mask, *bin, *db, names);
                        max_hold_named(&mut acc.snap_mask, *bin, *db, names);
                    }

                    let refresh = acc.display_window_start.elapsed().as_millis() >= DISPLAY_HOLD_MS;
                    if refresh {
                        let res_line = if acc.display_own.is_empty() && acc.display_relay.is_empty()
                        {
                            "No peaks".into()
                        } else {
                            format!(
                                "own {} · group {}",
                                acc.display_own.len().min(99),
                                acc.display_relay.len().min(99)
                            )
                        };
                        ui.set_resonance_line(SharedString::from(res_line));

                        let mask_line = if acc.display_mask.is_empty() {
                            "No collisions".into()
                        } else {
                            let top = acc
                                .display_mask
                                .iter()
                                .max_by(|a, b| {
                                    a.1 .0
                                        .partial_cmp(&b.1 .0)
                                        .unwrap_or(std::cmp::Ordering::Equal)
                                })
                                .map(|(b, (db, names))| {
                                    format!("bin {b}  {db:.0} dB  {}", names.join("+"))
                                })
                                .unwrap_or_default();
                            format!("{} hits · {top}", acc.display_mask.len())
                        };
                        ui.set_masking_line(SharedString::from(mask_line));

                        acc.display_own.clear();
                        acc.display_relay.clear();
                        acc.display_mask.clear();
                        acc.display_window_start = Instant::now();
                    }

                    // SNAP write
                    if snap_req.swap(false, Ordering::AcqRel) {
                        acc.snap_blink = 72;
                        let vault = get_plugin_dir("lucent");
                        let _ = std::fs::create_dir_all(&vault);
                        let mut res_own: Vec<(usize, f32)> = acc
                            .snap_res_own
                            .iter()
                            .map(|(&bin, &score)| (bin, score))
                            .collect();
                        res_own.sort_by(|a, b| {
                            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        let mut res_relay: Vec<(usize, f32, Vec<String>)> = acc
                            .snap_res_relay
                            .iter()
                            .map(|(&bin, (score, names))| (bin, *score, names.clone()))
                            .collect();
                        res_relay.sort_by(|a, b| {
                            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        let mut mask_v: Vec<(usize, f32, Vec<String>)> = acc
                            .snap_mask
                            .iter()
                            .map(|(&bin, (db, names))| (bin, *db, names.clone()))
                            .collect();
                        mask_v.sort_by(|a, b| {
                            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                        });
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
                            sample_rate,
                            plain,
                            mode,
                        );
                        let fname = snap_filename(&vault.to_string_lossy());
                        let path = vault.join(&fname);
                        let _ = std::fs::write(&path, &md);
                        acc.snap_res_own.clear();
                        acc.snap_res_relay.clear();
                        acc.snap_mask.clear();
                        ui.set_snap_status(SharedString::from(format!("Wrote {fname}")));
                    }
                    if acc.snap_blink > 0 {
                        acc.snap_blink -= 1;
                        if acc.snap_blink == 0 {
                            ui.set_snap_status(SharedString::from(""));
                        }
                    }
                }

                let peak = shared_sync.input_peak.load(Ordering::Relaxed);
                let pl = shared_sync.output_peak_l.load(Ordering::Relaxed);
                let pr = shared_sync.output_peak_r.load(Ordering::Relaxed);
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
