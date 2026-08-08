//! Equilibrium vault presets + SNAP markdown — Vizia parity.
//!
//! Profile types and markdown parsing moved here from the old `lx-vault` crate.

use std::path::{Path, PathBuf};

use aura_editor::typed::*;
use serde::{Deserialize, Serialize};

use crate::EquilibriumParams;
use crate::EquilibriumParamsParamId as P;

// ─── Profile (preset data) ────────────────────────────────────────────────────

pub const DEFAULT_TOLERANCES: [f32; 5] = [1.5, 2.0, 3.5, 4.5, 4.5];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub bands: [f32; 5],
    pub tolerances: [f32; 5],
    /// Per-band pan values (-1.0 = full L, 0 = C, 1.0 = full R)
    pub pans: [f32; 5],
    /// Per-band width values (0–150%)
    pub widths: [f32; 5],
    /// Mono Floor frequency in Hz (0 = off)
    pub mono_floor_hz: f32,
    /// Obsidian-compatible tags e.g. ["deep-techno", "kick", "premaster"]
    pub tags: Vec<String>,
    /// Format version — bump when adding new fields
    pub version: u32,
    /// Free-text notes shown in Obsidian and preset list
    pub notes: String,
    /// How the preset was created: "manual" | "analyze" | "claude"
    pub source: String,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "Unnamed".to_string(),
            bands: [0.0; 5],
            tolerances: DEFAULT_TOLERANCES,
            pans: [0.0; 5],
            widths: [100.0; 5],
            mono_floor_hz: 0.0,
            tags: Vec::new(),
            version: 2,
            notes: String::new(),
            source: "manual".to_string(),
        }
    }
}

// ─── Markdown serialization ───────────────────────────────────────────────────

fn format_pan_str(pan: f32) -> String {
    if pan.abs() < 0.01 {
        "C".to_string()
    } else if pan < 0.0 {
        format!("L {:.0}%", -pan * 100.0)
    } else {
        format!("R {:.0}%", pan * 100.0)
    }
}

/// Serialize a profile to Markdown with Obsidian-compatible frontmatter
pub fn export_preset_to_markdown(profile: &Profile) -> String {
    format!(
        "---\nplugin: equilibrium\ntype: preset\n---\n\n\
         > Warning: Do NOT modify column names or table structure. Plugin requires exact format for import. Only the NUMBERS may be changed.\n\n\
         ## Spektrale Balance (Baender)\n\n\
         | Band | Frequenzbereich | Relativer Level (dB) | Toleranz (dB) |\n\
         |---|---|---|---|\n\
         | Sub      | 0 - 80 Hz     | {b0:.1} | {t0:.1} |\n\
         | Bass     | 80 - 300 Hz   | {b1:.1} | {t1:.1} |\n\
         | Mid      | 300 - 2000 Hz | {b2:.1} | {t2:.1} |\n\
         | Presence | 2k - 6 kHz    | {b3:.1} | {t3:.1} |\n\
         | Air      | > 6 kHz       | {b4:.1} | {t4:.1} |\n\n\
         ## Stereo Settings\n\n\
         | Band | Pan | Width |\n\
         |---|---|---|\n\
         | Sub | {p0} | {w0:.0}% |\n\
         | Bass | {p1} | {w1:.0}% |\n\
         | Mid | {p2} | {w2:.0}% |\n\
         | Presence | {p3} | {w3:.0}% |\n\
         | Air | {p4} | {w4:.0}% |\n\n\
         ## Mono Floor\n\n\
         {mf:.0} Hz\n",
        b0 = profile.bands[0], t0 = profile.tolerances[0],
        b1 = profile.bands[1], t1 = profile.tolerances[1],
        b2 = profile.bands[2], t2 = profile.tolerances[2],
        b3 = profile.bands[3], t3 = profile.tolerances[3],
        b4 = profile.bands[4], t4 = profile.tolerances[4],
        p0 = format_pan_str(profile.pans[0]),
        p1 = format_pan_str(profile.pans[1]),
        p2 = format_pan_str(profile.pans[2]),
        p3 = format_pan_str(profile.pans[3]),
        p4 = format_pan_str(profile.pans[4]),
        w0 = profile.widths[0], w1 = profile.widths[1], w2 = profile.widths[2],
        w3 = profile.widths[3], w4 = profile.widths[4],
        mf = profile.mono_floor_hz,
    )
}

fn parse_pan_str(s: &str) -> f32 {
    let s = s.trim();
    if s.eq_ignore_ascii_case("c") || s.eq_ignore_ascii_case("center") {
        return 0.0;
    }
    if let Some(rest) = s.strip_prefix(|c: char| c == 'L' || c == 'l')
        && let Ok(n) = rest.trim().trim_end_matches('%').trim().parse::<f32>()
    {
        return -(n / 100.0).clamp(-1.0, 1.0);
    }
    if let Some(rest) = s.strip_prefix(|c: char| c == 'R' || c == 'r')
        && let Ok(n) = rest.trim().trim_end_matches('%').trim().parse::<f32>()
    {
        return (n / 100.0).clamp(-1.0, 1.0);
    }
    0.0
}

fn parse_frontmatter_list(content: &str, key: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut in_list = false;
    let mut lines = content.lines();

    if lines.next().map(|l| l.trim()) != Some("---") {
        return result;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if trimmed.starts_with(&format!("{}:", key)) {
            in_list = true;
            continue;
        }
        if in_list {
            if let Some(rest) = trimmed.strip_prefix("- ") {
                result.push(rest.trim().to_string());
            } else if trimmed.contains(':') {
                break;
            }
        }
    }
    result
}

/// Parse a preset/profile from Markdown — requires plugin: equilibrium frontmatter and all 5 bands
pub fn parse_preset_from_markdown(content: &str) -> Option<Profile> {
    let frontmatter = aura_dsp::analysis::vault::parse_frontmatter(content);

    match frontmatter.get("plugin").map(|s| s.as_str()) {
        Some("equilibrium") => {}
        _ => return None,
    }

    let tags = parse_frontmatter_list(content, "tags");
    let version = frontmatter
        .get("version")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let source = frontmatter
        .get("source")
        .cloned()
        .unwrap_or_else(|| "manual".to_string());

    let mut name = String::new();
    let mut notes = String::new();
    let mut bands = [0.0f32; 5];
    let mut tolerances = DEFAULT_TOLERANCES;
    let mut pans = [0.0f32; 5];
    let mut widths = [100.0f32; 5];
    let mut mono_floor_hz = 0.0f32;
    let mut has_bands = [false; 5];
    let mut in_stereo_table = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.contains("**Preset Name:**") {
            if let Some(pos) = trimmed.find("**Preset Name:**") {
                let start = pos + "**Preset Name:**".len();
                name = trimmed[start..].replace("**", "").trim().to_string();
            }
        } else if trimmed.contains("**Notizen:**") || trimmed.contains("**Notes:**") {
            let marker = if trimmed.contains("**Notizen:**") {
                "**Notizen:**"
            } else {
                "**Notes:**"
            };
            if let Some(pos) = trimmed.find(marker) {
                notes = trimmed[pos + marker.len()..]
                    .replace("**", "")
                    .trim()
                    .to_string();
            }
        } else if trimmed.contains("## Stereo Settings")
            || trimmed.contains("## Stereo-Einstellungen")
        {
            in_stereo_table = true;
        } else if trimmed.contains("## Mono Floor") {
            in_stereo_table = false;
        } else if trimmed.starts_with('|') && in_stereo_table {
            let parts: Vec<&str> = trimmed.split('|').map(|s| s.trim()).collect();
            if parts.len() >= 4 && !parts[1].contains("Band") && !parts[1].contains("---") {
                let band_name = parts[1].to_lowercase();
                let idx = match band_name.as_str() {
                    "sub" => Some(0),
                    "bass" => Some(1),
                    "mid" => Some(2),
                    "presence" | "high mid" | "high-mid" | "pres" => Some(3),
                    "air" | "high" => Some(4),
                    _ => None,
                };
                if let Some(b) = idx {
                    let pan_str = parts[2];
                    pans[b] = parse_pan_str(pan_str);
                    if let Ok(w) = parts[3].trim_end_matches('%').parse::<f32>() {
                        widths[b] = w;
                    }
                }
            }
        } else if trimmed.starts_with('|') {
            let parts: Vec<&str> = trimmed.split('|').map(|s| s.trim()).collect();
            if parts.len() >= 4 {
                let band_name = parts[1].to_lowercase();
                let idx = match band_name.as_str() {
                    "sub" => Some(0),
                    "bass" => Some(1),
                    "mid" => Some(2),
                    "presence" | "high mid" | "high-mid" | "pres" => Some(3),
                    "air" | "high" => Some(4),
                    _ => None,
                };
                if let Some(b) = idx {
                    if let Ok(db) = parts[3].parse::<f32>() {
                        bands[b] = db;
                        has_bands[b] = true;
                    }
                    if parts.len() >= 5
                        && let Ok(tol) = parts[4].parse::<f32>()
                    {
                        tolerances[b] = tol;
                    }
                }
            }
        }
        if !in_stereo_table
            && trimmed.chars().any(|c| c.is_ascii_digit())
            && !trimmed.starts_with('|')
            && !trimmed.contains('#')
            && let Some(hz_str) = trimmed.split_whitespace().next()
            && let Ok(hz) = hz_str.parse::<f32>()
        {
            mono_floor_hz = hz;
        }
    }

    if has_bands.iter().all(|&h| h) {
        if name.is_empty() {
            name = "Unnamed".to_string();
        }
        Some(Profile {
            name,
            bands,
            tolerances,
            pans,
            widths,
            mono_floor_hz,
            tags,
            version,
            notes,
            source,
        })
    } else {
        None
    }
}

// ─── Preset file scanning ─────────────────────────────────────────────────────

pub fn list_custom_presets(
    plugin_name: &str,
    vault_path: Option<&str>,
) -> Vec<(String, PathBuf, Profile)> {
    let mut presets = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();

    let local_dir = aura_dsp::analysis::vault::get_plugin_dir(plugin_name).join("presets");
    let _ = std::fs::create_dir_all(&local_dir);

    let mut scan_dir = |dir: &Path| {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if path.is_file()
                    && path.extension().is_some_and(|ext| ext == "md")
                    && !stem.starts_with("SNAPSHOT-")
                    && seen_paths.insert(path.clone())
                    && let Ok(content) = std::fs::read_to_string(&path)
                    && let Some(mut profile) = parse_preset_from_markdown(&content)
                {
                    profile.name = stem.clone();
                    presets.push((stem, path, profile));
                }
            }
        }
    };

    scan_dir(&local_dir);

    if let Some(vp) = vault_path
        && !vp.is_empty()
    {
        let vault_dir = Path::new(vp);
        scan_dir(vault_dir);
    }

    presets
}

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
        _ => aura_dsp::analysis::vault::get_plugin_dir("Equilibrium").join("presets"),
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
