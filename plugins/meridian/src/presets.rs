//! Meridian vault presets + SNAP markdown — compatible with Vizia Meridian.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use lx_slint_editor::PluginContext;
use truce_params::{FloatParam, IntParam};

use crate::MeridianParams;
use crate::MeridianParamsParamId as P;

pub type PresetEntry = (String, PathBuf, MeridianProfile);

#[derive(Debug, Clone)]
pub struct MeridianProfile {
    pub name: String,
    pub hpf_freq: f32,
    pub lpf_freq: f32,
    pub cut_slope: i32,
    pub bass_gain: f32,
    pub bass_slope: i32,
    pub lo_mid_gain: f32,
    pub lo_mid_slope: i32,
    pub mid_gain: f32,
    pub mid_slope: i32,
    pub high_gain: f32,
    pub high_slope: i32,
    pub excite_gain: f32,
    pub excite_slope: i32,
    pub eq_freq_1: f32,
    pub eq_freq_2: f32,
    pub eq_freq_3: f32,
    pub eq_freq_4: f32,
    pub eq_freq_5: f32,
    pub tilt_gain: f32,
    pub warmth_drive: f32,
    pub warmth_mix: f32,
    pub excite_amount: f32,
    pub excite_blend: f32,
    pub excite_freq: f32,
    pub comp_threshold: f32,
    pub comp_mix: f32,
    pub comp_attack: f32,
    pub comp_release: f32,
    pub comp_character: f32,
    pub comp_makeup: f32,
    pub inflate_effect: f32,
    pub inflate_curve: f32,
    pub inflate_band_split: bool,
    pub inflate_clip: bool,
    pub stereo_width: f32,
    pub pan: f32,
    pub output_gain: f32,
}

impl Default for MeridianProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            hpf_freq: 2.0,
            lpf_freq: 35000.0,
            cut_slope: 0,
            bass_gain: 0.0,
            bass_slope: 1,
            lo_mid_gain: 0.0,
            lo_mid_slope: 1,
            mid_gain: 0.0,
            mid_slope: 1,
            high_gain: 0.0,
            high_slope: 1,
            excite_gain: 0.0,
            excite_slope: 1,
            eq_freq_1: 80.0,
            eq_freq_2: 300.0,
            eq_freq_3: 1000.0,
            eq_freq_4: 4000.0,
            eq_freq_5: 12000.0,
            tilt_gain: 0.0,
            warmth_drive: 0.0,
            warmth_mix: 0.0,
            excite_amount: 0.0,
            excite_blend: 0.0,
            excite_freq: 8000.0,
            comp_threshold: 0.0,
            comp_mix: 0.0,
            comp_attack: 15.0,
            comp_release: 120.0,
            comp_character: 2.0,
            comp_makeup: 0.0,
            inflate_effect: 0.0,
            inflate_curve: 0.0,
            inflate_band_split: false,
            inflate_clip: false,
            stereo_width: 100.0,
            pan: 0.0,
            output_gain: 0.0,
        }
    }
}

fn slope_char(s: i32) -> &'static str {
    match s {
        0 => "A",
        1 => "B",
        _ => "C",
    }
}

fn parse_slope(s: &str) -> i32 {
    match s {
        "A" => 0,
        "B" => 1,
        "C" => 2,
        _ => 1,
    }
}

// ─── Parse / export ──────────────────────────────────────────────────────────

pub fn parse_meridian_markdown(content: &str) -> Option<MeridianProfile> {
    match lx_analysis::preset_plugin_name(content).as_deref() {
        Some("meridian") => {}
        _ => return None,
    }
    let mut p = MeridianProfile::default();
    let mut has_hpf = false;
    let mut has_lpf = false;
    let mut has_bass = false;
    let mut has_mid = false;
    let mut has_output = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('|').map(|s| s.trim()).collect();
        if parts.len() < 4 {
            continue;
        }
        match parts[1].to_lowercase().as_str() {
            "hpf" => {
                if let Ok(v) = parts[2].parse() {
                    p.hpf_freq = v;
                    has_hpf = true;
                }
            }
            "lpf" => {
                if let Ok(v) = parts[2].parse() {
                    p.lpf_freq = v;
                    has_lpf = true;
                }
            }
            "cut slope" => {
                p.cut_slope = if parts[2] == "B" { 1 } else { 0 };
            }
            "bass gain" => {
                if let Ok(v) = parts[2].parse() {
                    p.bass_gain = v;
                    has_bass = true;
                }
            }
            "bass slope" => p.bass_slope = parse_slope(parts[2]),
            "eq freq 1" => {
                if let Ok(v) = parts[2].parse() {
                    p.eq_freq_1 = v;
                }
            }
            "lo-mid gain" => {
                if let Ok(v) = parts[2].parse() {
                    p.lo_mid_gain = v;
                }
            }
            "lo-mid slope" => p.lo_mid_slope = parse_slope(parts[2]),
            "eq freq 2" => {
                if let Ok(v) = parts[2].parse() {
                    p.eq_freq_2 = v;
                }
            }
            "mid gain" => {
                if let Ok(v) = parts[2].parse() {
                    p.mid_gain = v;
                    has_mid = true;
                }
            }
            "mid slope" => p.mid_slope = parse_slope(parts[2]),
            "eq freq 3" => {
                if let Ok(v) = parts[2].parse() {
                    p.eq_freq_3 = v;
                }
            }
            "high gain" => {
                if let Ok(v) = parts[2].parse() {
                    p.high_gain = v;
                }
            }
            "high slope" => p.high_slope = parse_slope(parts[2]),
            "eq freq 4" => {
                if let Ok(v) = parts[2].parse() {
                    p.eq_freq_4 = v;
                }
            }
            "excite gain" => {
                if let Ok(v) = parts[2].parse() {
                    p.excite_gain = v;
                }
            }
            "excite slope" => p.excite_slope = parse_slope(parts[2]),
            "eq freq 5" => {
                if let Ok(v) = parts[2].parse() {
                    p.eq_freq_5 = v;
                }
            }
            "comp threshold" => {
                if let Ok(v) = parts[2].parse() {
                    p.comp_threshold = v;
                }
            }
            "comp mix" => {
                if let Ok(v) = parts[2].parse() {
                    p.comp_mix = v;
                }
            }
            "comp attack" => {
                if let Ok(v) = parts[2].parse() {
                    p.comp_attack = v;
                }
            }
            "comp release" => {
                if let Ok(v) = parts[2].parse() {
                    p.comp_release = v;
                }
            }
            "comp character" => {
                if let Ok(v) = parts[2].parse() {
                    p.comp_character = v;
                }
            }
            "comp makeup" => {
                if let Ok(v) = parts[2].parse() {
                    p.comp_makeup = v;
                }
            }
            "inflate effect" => {
                if let Ok(v) = parts[2].parse() {
                    p.inflate_effect = v;
                }
            }
            "inflate curve" => {
                if let Ok(v) = parts[2].parse() {
                    p.inflate_curve = v;
                }
            }
            "inflate band split" => p.inflate_band_split = parts[2] == "On",
            "inflate clip" => p.inflate_clip = parts[2] == "On",
            "warmth drive" => {
                if let Ok(v) = parts[2].parse() {
                    p.warmth_drive = v;
                }
            }
            "warmth mix" => {
                if let Ok(v) = parts[2].parse() {
                    p.warmth_mix = v;
                }
            }
            "excite amount" => {
                if let Ok(v) = parts[2].parse() {
                    p.excite_amount = v;
                }
            }
            "excite blend" => {
                if let Ok(v) = parts[2].parse() {
                    p.excite_blend = v;
                }
            }
            "excite freq" => {
                if let Ok(v) = parts[2].parse() {
                    p.excite_freq = v;
                }
            }
            "tilt" => {
                if let Ok(v) = parts[2].parse() {
                    p.tilt_gain = v;
                }
            }
            "stereo width" => {
                if let Ok(v) = parts[2].parse() {
                    p.stereo_width = v;
                }
            }
            "pan" => {
                if let Ok(v) = parts[2].parse() {
                    p.pan = v;
                }
            }
            "output gain" => {
                if let Ok(v) = parts[2].parse() {
                    p.output_gain = v;
                    has_output = true;
                }
            }
            _ => {}
        }
    }
    if has_hpf && has_lpf && has_bass && has_mid && has_output {
        Some(p)
    } else {
        None
    }
}

pub fn export_meridian_markdown(p: &MeridianProfile) -> String {
    let mut s = String::new();
    s.push_str("---\nplugin: meridian\ntype: preset\n---\n\n");
    s.push_str("> Warning: Do NOT modify column names or table structure.\n\n");
    s.push_str("## Parameter\n\n| Parameter | Wert | Einheit |\n|---|---|---|\n");
    s.push_str(&format!("| HPF | {:.1} | Hz |\n", p.hpf_freq));
    s.push_str(&format!("| LPF | {:.1} | Hz |\n", p.lpf_freq));
    s.push_str(&format!(
        "| Cut Slope | {} | |\n",
        if p.cut_slope >= 1 { "B" } else { "A" }
    ));
    s.push_str(&format!("| Bass Gain | {:.1} | dB |\n", p.bass_gain));
    s.push_str(&format!("| Bass Slope | {} | |\n", slope_char(p.bass_slope)));
    s.push_str(&format!("| EQ Freq 1 | {:.0} | Hz |\n", p.eq_freq_1));
    s.push_str(&format!("| Lo-Mid Gain | {:.1} | dB |\n", p.lo_mid_gain));
    s.push_str(&format!(
        "| Lo-Mid Slope | {} | |\n",
        slope_char(p.lo_mid_slope)
    ));
    s.push_str(&format!("| EQ Freq 2 | {:.0} | Hz |\n", p.eq_freq_2));
    s.push_str(&format!("| Mid Gain | {:.1} | dB |\n", p.mid_gain));
    s.push_str(&format!("| Mid Slope | {} | |\n", slope_char(p.mid_slope)));
    s.push_str(&format!("| EQ Freq 3 | {:.0} | Hz |\n", p.eq_freq_3));
    s.push_str(&format!("| High Gain | {:.1} | dB |\n", p.high_gain));
    s.push_str(&format!("| High Slope | {} | |\n", slope_char(p.high_slope)));
    s.push_str(&format!("| EQ Freq 4 | {:.0} | Hz |\n", p.eq_freq_4));
    s.push_str(&format!("| Excite Gain | {:.1} | dB |\n", p.excite_gain));
    s.push_str(&format!(
        "| Excite Slope | {} | |\n",
        slope_char(p.excite_slope)
    ));
    s.push_str(&format!("| EQ Freq 5 | {:.0} | Hz |\n", p.eq_freq_5));
    s.push_str(&format!(
        "| Comp Threshold | {:.1} | dB |\n",
        p.comp_threshold
    ));
    s.push_str(&format!("| Comp Mix | {:.1} | % |\n", p.comp_mix));
    s.push_str(&format!("| Comp Attack | {:.1} | ms |\n", p.comp_attack));
    s.push_str(&format!("| Comp Release | {:.1} | ms |\n", p.comp_release));
    s.push_str(&format!("| Comp Character | {:.1} | |\n", p.comp_character));
    s.push_str(&format!("| Comp Makeup | {:.1} | dB |\n", p.comp_makeup));
    s.push_str(&format!(
        "| Inflate Effect | {:.1} | % |\n",
        p.inflate_effect
    ));
    s.push_str(&format!("| Inflate Curve | {:.1} | |\n", p.inflate_curve));
    s.push_str(&format!(
        "| Inflate Band Split | {} | |\n",
        if p.inflate_band_split { "On" } else { "Off" }
    ));
    s.push_str(&format!(
        "| Inflate Clip | {} | |\n",
        if p.inflate_clip { "On" } else { "Off" }
    ));
    s.push_str(&format!("| Warmth Drive | {:.1} | dB |\n", p.warmth_drive));
    s.push_str(&format!("| Warmth Mix | {:.1} | % |\n", p.warmth_mix));
    s.push_str(&format!("| Excite Amount | {:.1} | % |\n", p.excite_amount));
    s.push_str(&format!("| Excite Blend | {:.1} | % |\n", p.excite_blend));
    s.push_str(&format!("| Excite Freq | {:.0} | Hz |\n", p.excite_freq));
    s.push_str(&format!("| Tilt | {:.1} | dB |\n", p.tilt_gain));
    s.push_str(&format!("| Stereo Width | {:.1} | % |\n", p.stereo_width));
    s.push_str(&format!("| Pan | {:.2} | |\n", p.pan));
    s.push_str(&format!("| Output Gain | {:.1} | dB |\n", p.output_gain));
    s
}

pub fn profile_from_params(params: &MeridianParams, name: &str) -> MeridianProfile {
    MeridianProfile {
        name: name.to_string(),
        hpf_freq: params.hpf_freq.raw_target() as f32,
        lpf_freq: params.lpf_freq.raw_target() as f32,
        cut_slope: params.cut_slope.value() as i32,
        bass_gain: params.bass_gain.raw_target() as f32,
        bass_slope: params.bass_slope.value() as i32,
        lo_mid_gain: params.lo_mid_gain.raw_target() as f32,
        lo_mid_slope: params.lo_mid_slope.value() as i32,
        mid_gain: params.mid_gain.raw_target() as f32,
        mid_slope: params.mid_slope.value() as i32,
        high_gain: params.high_gain.raw_target() as f32,
        high_slope: params.high_slope.value() as i32,
        excite_gain: params.excite_gain.raw_target() as f32,
        excite_slope: params.excite_slope.value() as i32,
        eq_freq_1: params.eq_freq_1.raw_target() as f32,
        eq_freq_2: params.eq_freq_2.raw_target() as f32,
        eq_freq_3: params.eq_freq_3.raw_target() as f32,
        eq_freq_4: params.eq_freq_4.raw_target() as f32,
        eq_freq_5: params.eq_freq_5.raw_target() as f32,
        tilt_gain: params.tilt_gain.raw_target() as f32,
        warmth_drive: params.warmth_drive.raw_target() as f32,
        warmth_mix: params.warmth_mix.raw_target() as f32,
        excite_amount: params.excite_amount.raw_target() as f32,
        excite_blend: params.excite_blend.raw_target() as f32,
        excite_freq: params.excite_freq.raw_target() as f32,
        comp_threshold: params.comp_threshold.raw_target() as f32,
        comp_mix: params.comp_mix.raw_target() as f32,
        comp_attack: params.comp_attack.raw_target() as f32,
        comp_release: params.comp_release.raw_target() as f32,
        comp_character: params.comp_character.raw_target() as f32,
        comp_makeup: params.comp_makeup.raw_target() as f32,
        inflate_effect: params.inflate_effect.raw_target() as f32,
        inflate_curve: params.inflate_curve.raw_target() as f32,
        inflate_band_split: params.inflate_band_split.value(),
        inflate_clip: params.inflate_clip.value(),
        stereo_width: params.stereo_width.raw_target() as f32,
        pan: params.pan.raw_target() as f32,
        output_gain: params.output_gain.raw_target() as f32,
    }
}

/// Apply via host automate; normalize through each param range (log freqs).
pub fn apply_profile(
    ctx: &PluginContext<MeridianParams>,
    params: &MeridianParams,
    profile: &MeridianProfile,
) {
    let f = |fp: &FloatParam, v: f32| fp.info.range.normalize(v as f64);
    let i = |ip: &IntParam, v: i32| ip.info.range.normalize(v as f64);
    ctx.automate(P::HpfFreq, f(&params.hpf_freq, profile.hpf_freq));
    ctx.automate(P::LpfFreq, f(&params.lpf_freq, profile.lpf_freq));
    ctx.automate(P::CutSlope, i(&params.cut_slope, profile.cut_slope));
    ctx.automate(P::BassGain, f(&params.bass_gain, profile.bass_gain));
    ctx.automate(P::BassSlope, i(&params.bass_slope, profile.bass_slope));
    ctx.automate(P::LoMidGain, f(&params.lo_mid_gain, profile.lo_mid_gain));
    ctx.automate(P::LoMidSlope, i(&params.lo_mid_slope, profile.lo_mid_slope));
    ctx.automate(P::MidGain, f(&params.mid_gain, profile.mid_gain));
    ctx.automate(P::MidSlope, i(&params.mid_slope, profile.mid_slope));
    ctx.automate(P::HighGain, f(&params.high_gain, profile.high_gain));
    ctx.automate(P::HighSlope, i(&params.high_slope, profile.high_slope));
    ctx.automate(P::ExciteGain, f(&params.excite_gain, profile.excite_gain));
    ctx.automate(P::ExciteSlope, i(&params.excite_slope, profile.excite_slope));
    ctx.automate(P::EqFreq1, f(&params.eq_freq_1, profile.eq_freq_1));
    ctx.automate(P::EqFreq2, f(&params.eq_freq_2, profile.eq_freq_2));
    ctx.automate(P::EqFreq3, f(&params.eq_freq_3, profile.eq_freq_3));
    ctx.automate(P::EqFreq4, f(&params.eq_freq_4, profile.eq_freq_4));
    ctx.automate(P::EqFreq5, f(&params.eq_freq_5, profile.eq_freq_5));
    ctx.automate(P::TiltGain, f(&params.tilt_gain, profile.tilt_gain));
    ctx.automate(P::WarmthDrive, f(&params.warmth_drive, profile.warmth_drive));
    ctx.automate(P::WarmthMix, f(&params.warmth_mix, profile.warmth_mix));
    ctx.automate(P::ExciteAmount, f(&params.excite_amount, profile.excite_amount));
    ctx.automate(P::ExciteBlend, f(&params.excite_blend, profile.excite_blend));
    ctx.automate(P::ExciteFreq, f(&params.excite_freq, profile.excite_freq));
    ctx.automate(
        P::CompThreshold,
        f(&params.comp_threshold, profile.comp_threshold),
    );
    ctx.automate(P::CompMix, f(&params.comp_mix, profile.comp_mix));
    ctx.automate(P::CompAttack, f(&params.comp_attack, profile.comp_attack));
    ctx.automate(P::CompRelease, f(&params.comp_release, profile.comp_release));
    ctx.automate(
        P::CompCharacter,
        f(&params.comp_character, profile.comp_character),
    );
    ctx.automate(P::CompMakeup, f(&params.comp_makeup, profile.comp_makeup));
    ctx.automate(
        P::InflateEffect,
        f(&params.inflate_effect, profile.inflate_effect),
    );
    ctx.automate(P::InflateCurve, f(&params.inflate_curve, profile.inflate_curve));
    ctx.automate(
        P::InflateBandSplit,
        if profile.inflate_band_split { 1.0 } else { 0.0 },
    );
    ctx.automate(P::InflateClip, if profile.inflate_clip { 1.0 } else { 0.0 });
    ctx.automate(P::StereoWidth, f(&params.stereo_width, profile.stereo_width));
    ctx.automate(P::Pan, f(&params.pan, profile.pan));
    ctx.automate(P::OutputGain, f(&params.output_gain, profile.output_gain));
}

// ─── Scan ────────────────────────────────────────────────────────────────────

pub fn scan_meridian_presets(dir: &Path) -> Vec<PresetEntry> {
    let mut v = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let path = e.path();
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if path.is_file()
                && path.extension().is_some_and(|x| x == "md")
                && !stem.starts_with("SNAPSHOT-")
                && seen.insert(path.clone())
                && let Ok(c) = std::fs::read_to_string(&path)
                && let Some(mut pf) = parse_meridian_markdown(&c)
            {
                pf.name = stem.clone();
                v.push((stem, path, pf));
            }
        }
    }
    v
}

pub fn list_meridian_presets(vault_path: Option<&str>) -> Vec<PresetEntry> {
    let mut presets = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let local = lx_analysis::get_plugin_dir("Meridian").join("presets");
    let _ = std::fs::create_dir_all(&local);
    for entry in scan_meridian_presets(&local) {
        if seen.insert(entry.0.clone()) {
            presets.push(entry);
        }
    }
    if let Some(vp) = vault_path.filter(|s| !s.is_empty()) {
        for entry in scan_meridian_presets(Path::new(vp)) {
            if seen.insert(entry.0.clone()) {
                presets.push(entry);
            }
        }
    }
    presets
}

pub fn find_profile(
    name: &str,
    _vault_path: &Option<String>,
    cache: &[PresetEntry],
) -> Option<MeridianProfile> {
    if let Some((_, _, p)) = cache.iter().find(|(n, _, _)| n == name) {
        return Some(p.clone());
    }
    let local = lx_analysis::get_plugin_dir("Meridian").join("presets");
    let candidate = local.join(format!("{name}.md"));
    if candidate.is_file()
        && let Ok(c) = std::fs::read_to_string(&candidate)
        && let Some(mut pf) = parse_meridian_markdown(&c)
    {
        if pf.name.is_empty() {
            pf.name = name.to_string();
        }
        return Some(pf);
    }
    None
}

pub fn preset_save_dir(vault_path: &Option<String>) -> PathBuf {
    match vault_path {
        Some(vp) if !vp.is_empty() => PathBuf::from(vp),
        _ => lx_analysis::get_plugin_dir("Meridian").join("presets"),
    }
}

pub fn merge_preset_names(scanned: &[PresetEntry]) -> Vec<String> {
    scanned.iter().map(|(n, _, _)| n.clone()).collect()
}

// ─── SNAP file ───────────────────────────────────────────────────────────────

pub fn snap_filename(vault_path: &str) -> String {
    let dir = Path::new(vault_path);
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

#[allow(clippy::too_many_arguments)]
pub fn snap_markdown(
    stereo: &[f32],
    mono: &[f32],
    delta: &[f32],
    band_levels: [f32; 5],
    corr: f32,
    pl: f32,
    pr: f32,
    sr: f32,
) -> String {
    let fft_sz = 2048.0;
    let freqs: &[f32] = &[
        20.0, 40.0, 80.0, 160.0, 315.0, 630.0, 1250.0, 2500.0, 5000.0, 10000.0, 16000.0, 20000.0,
    ];
    let tbl = |s: &[f32]| {
        freqs
            .iter()
            .map(|&f| {
                let bin = ((f * fft_sz / sr) as usize).min(s.len().saturating_sub(1));
                format!(
                    "| {} | {:.1} |",
                    if f >= 1000.0 {
                        format!("{:.0}k", f / 1000.0)
                    } else {
                        format!("{:.0}", f)
                    },
                    s[bin]
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "---\nplugin: meridian\ntype: snapshot\n---\n\n# Meridian Snapshot\n\n\
        ## Signal\n| | L | R |\n|--|--|--|\n| Peak | {pl:.1} dB | {pr:.1} dB |\n| Korrelation | {co:.2} | |\n\n\
        ## Spektrum — Stereo\n| Hz | dB |\n|----|-----|\n{st}\n\n\
        ## Spektrum — Mono\n| Hz | dB |\n|----|-----|\n{mn}\n\n\
        ## Delta\n| Hz | dB |\n|----|-----|\n{dt}\n\n\
        ## 5-Band\n| Band | Pegel |\n|------|-------|\n\
        | Sub | {b0:.1} dB |\n| Bass | {b1:.1} dB |\n| Mid | {b2:.1} dB |\n| Presence | {b3:.1} dB |\n| Air | {b4:.1} dB |\n",
        pl = pl,
        pr = pr,
        co = corr,
        st = tbl(stereo),
        mn = tbl(mono),
        dt = tbl(delta),
        b0 = band_levels[0],
        b1 = band_levels[1],
        b2 = band_levels[2],
        b3 = band_levels[3],
        b4 = band_levels[4],
    )
}

// ─── Background scan ─────────────────────────────────────────────────────────

pub struct PendingPresets {
    pub ready: AtomicBool,
    pub generation: AtomicU32,
    pub presets: Mutex<Option<(u32, Vec<PresetEntry>)>>,
}

impl PendingPresets {
    pub fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
            generation: AtomicU32::new(0),
            presets: Mutex::new(None),
        }
    }

    pub fn bump_generation(&self) -> u32 {
        let new = self.generation.load(Ordering::Relaxed).wrapping_add(1);
        self.generation.store(new, Ordering::Release);
        self.ready.store(false, Ordering::Release);
        if let Ok(mut guard) = self.presets.lock() {
            *guard = None;
        }
        new
    }
}

impl Default for PendingPresets {
    fn default() -> Self {
        Self::new()
    }
}

/// `vp` may be vault path or empty (local-only). Always merges local presets.
pub fn spawn_vault_scan(vp: String, pending: Arc<PendingPresets>, generation: u32) {
    std::thread::spawn(move || {
        let scanned = list_meridian_presets(if vp.is_empty() { None } else { Some(vp.as_str()) });
        if let Ok(mut guard) = pending.presets.lock() {
            *guard = Some((generation, scanned));
        }
        pending.ready.store(true, Ordering::Release);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_roundtrip_core_fields() {
        let p = MeridianProfile {
            hpf_freq: 40.0,
            lpf_freq: 12000.0,
            bass_gain: 1.5,
            mid_gain: -0.5,
            output_gain: 2.0,
            cut_slope: 1,
            inflate_clip: true,
            ..Default::default()
        };
        let md = export_meridian_markdown(&p);
        let back = parse_meridian_markdown(&md).expect("parse");
        assert!((back.hpf_freq - 40.0).abs() < 0.01);
        assert!((back.lpf_freq - 12000.0).abs() < 0.01);
        assert!((back.bass_gain - 1.5).abs() < 0.01);
        assert!((back.mid_gain - (-0.5)).abs() < 0.01);
        assert!((back.output_gain - 2.0).abs() < 0.01);
        assert_eq!(back.cut_slope, 1);
        assert!(back.inflate_clip);
    }

    #[test]
    fn snap_filename_increments() {
        let dir = std::env::temp_dir().join(format!(
            "meridian_snap_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SNAPSHOT-001.md"), "x").unwrap();
        std::fs::write(dir.join("SNAPSHOT-007.md"), "x").unwrap();
        assert_eq!(snap_filename(dir.to_str().unwrap()), "SNAPSHOT-008.md");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
