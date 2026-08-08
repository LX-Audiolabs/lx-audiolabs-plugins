//! Equilibrium vault presets + SNAP markdown — Vizia parity via lx_analysis::Profile.

use std::path::{Path, PathBuf};

use lx_analysis::{list_custom_presets, Profile, DEFAULT_TOLERANCES};
use lx_slint_editor::LxPluginContext;

use crate::EquilibriumParams;
use crate::EquilibriumParamsParamId as P;

#[derive(Debug, Clone)]
pub struct EqPreset {
    pub name: String,
    pub bands: [f32; 5],
    pub tolerances: [f32; 5],
    pub pans: [f32; 5],
    pub widths: [f32; 5],
    pub mono_floor_hz: f32,
}

pub type PresetEntry = (String, Option<PathBuf>, EqPreset);

pub fn pink_noise_preset() -> EqPreset {
    EqPreset {
        name: "Pink Noise".into(),
        // Band power is per-octave normalized in DSP → pink reads flat.
        bands: [0.0; 5],
        tolerances: DEFAULT_TOLERANCES,
        pans: [0.0; 5],
        widths: [100.0; 5],
        mono_floor_hz: 0.0,
    }
}

pub fn load_presets(vault_path: Option<&str>) -> Vec<PresetEntry> {
    let mut presets = vec![(
        "Pink Noise".to_string(),
        None,
        pink_noise_preset(),
    )];
    for (name, path, profile) in list_custom_presets("Equilibrium", vault_path) {
        presets.push((
            name.clone(),
            Some(path),
            EqPreset {
                name,
                bands: profile.bands,
                tolerances: profile.tolerances,
                pans: profile.pans,
                widths: profile.widths,
                mono_floor_hz: profile.mono_floor_hz,
            },
        ));
    }
    presets
}

pub fn preset_names(entries: &[PresetEntry]) -> Vec<String> {
    entries.iter().map(|(n, _, _)| n.clone()).collect()
}

/// Normalize plain param value (fixed linear ranges from #[param]).
pub fn param_norm(id: P, plain: f64) -> f64 {
    let (min, max) = match id {
        P::LowGain | P::BassGain | P::MidGain | P::HighMidGain | P::HighGain | P::OutputGain => {
            (-12.0, 12.0)
        }
        P::LowWidth | P::BassWidth | P::MidWidth | P::HighMidWidth | P::HighWidth => (0.0, 150.0),
        P::LowPan | P::BassPan | P::MidPan | P::HighMidPan | P::HighPan => (-1.0, 1.0),
        P::MonoFloor => (0.0, 300.0),
        P::PreMasterTargetDb => (-6.0, -3.0),
        _ => (0.0, 1.0),
    };
    ((plain - min) / (max - min)).clamp(0.0, 1.0)
}

/// Apply stereo settings from a target profile (not band gains — those are analysis targets).
pub fn apply_stereo_from_preset(ctx: &LxPluginContext<EquilibriumParams>, p: &EqPreset) {
    for (id, val) in [
        (P::LowWidth, p.widths[0] as f64),
        (P::BassWidth, p.widths[1] as f64),
        (P::MidWidth, p.widths[2] as f64),
        (P::HighMidWidth, p.widths[3] as f64),
        (P::HighWidth, p.widths[4] as f64),
        (P::LowPan, p.pans[0] as f64),
        (P::BassPan, p.pans[1] as f64),
        (P::MidPan, p.pans[2] as f64),
        (P::HighMidPan, p.pans[3] as f64),
        (P::HighPan, p.pans[4] as f64),
        (P::MonoFloor, p.mono_floor_hz as f64),
    ] {
        ctx.automate(id, param_norm(id, val));
    }
}

pub fn profile_for_save(
    name: &str,
    bands: [f32; 5],
    tolerances: [f32; 5],
    params: &EquilibriumParams,
) -> Profile {
    Profile {
        name: name.to_string(),
        bands,
        tolerances,
        pans: [
            params.low_pan.raw_target() as f32,
            params.bass_pan.raw_target() as f32,
            params.mid_pan.raw_target() as f32,
            params.high_mid_pan.raw_target() as f32,
            params.high_pan.raw_target() as f32,
        ],
        widths: [
            params.low_width.raw_target() as f32,
            params.bass_width.raw_target() as f32,
            params.mid_width.raw_target() as f32,
            params.high_mid_width.raw_target() as f32,
            params.high_width.raw_target() as f32,
        ],
        mono_floor_hz: params.mono_floor.raw_target() as f32,
        ..Profile::default()
    }
}

pub fn preset_save_dir(vault_path: &Option<String>) -> PathBuf {
    match vault_path {
        Some(vp) if !vp.is_empty() => PathBuf::from(vp),
        _ => lx_analysis::get_plugin_dir("Equilibrium").join("presets"),
    }
}

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
                    s.get(bin).copied().unwrap_or(-90.0)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "---\nplugin: equilibrium\ntype: snapshot\n---\n\n# Equilibrium Snapshot\n\n\
        ## Signal\n| | L | R |\n|--|--|--|\n| Peak | {pl:.1} dB | {pr:.1} dB |\n| Korrelation | {co:.2} | |\n\n\
        ## Spektrum — Stereo\n| Hz | dB |\n|----|-----|\n{st}\n\n\
        ## Spektrum — Mono\n| Hz | dB |\n|----|-----|\n{mn}\n\n\
        ## Delta\n| Hz | dB |\n|----|-----|\n{dt}\n\n\
        ## 5-Band\n| Band | Pegel |\n|------|-------|\n\
        | Low | {b0:.1} dB |\n| Bass | {b1:.1} dB |\n| Mid | {b2:.1} dB |\n| Hi-Mid | {b3:.1} dB |\n| High | {b4:.1} dB |\n",
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
