//! Aether preset vault — markdown profiles compatible with the Vizia Aether.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aura_editor::typed::*;
pub use lx_editor_utils::snap::{PendingPresets, spawn_vault_scan as spawn_vault_scan_impl};
use lx_vault::*;

use crate::AetherParams;
use crate::AetherParamsParamId as P;
use crate::{band_type_label, realism_label};

const FREQ_MIN: f32 = 20.0;
const Q_MIN: f32 = 0.3;
const Q_MAX: f32 = 8.0;

pub type PresetEntry = (String, PathBuf, AetherProfile);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AetherProfile {
    pub name: String,
    /// (type, freq, gain, Q) × 5
    pub bands: [(i32, f32, f32, f32); 5],
    pub cf_angle: f32,
    pub cf_amount: f32,
    pub cf_realism: i32,
    pub blend: f32,
    pub gain: f32,
}

pub fn harman_flat_profile() -> AetherProfile {
    AetherProfile {
        name: "Harman Flat".into(),
        bands: [
            (1, 105.0, 0.0, 0.7),
            (2, 300.0, 0.0, 1.0),
            (2, 1200.0, 0.0, 1.0),
            (2, 4000.0, 0.0, 1.0),
            (3, 10000.0, 0.0, 0.7),
        ],
        cf_angle: 60.0,
        cf_amount: 0.0,
        cf_realism: 0,
        blend: 100.0,
        gain: 0.0,
    }
}

pub fn default_preset_names() -> Vec<String> {
    vec!["Harman Flat".into()]
}

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

// ─── Parse / scan ────────────────────────────────────────────────────────────

pub fn parse_aether_preset(content: &str) -> Option<AetherProfile> {
    match preset_plugin_name(content).as_deref() {
        Some("aether") => {}
        _ => return None,
    }
    let mut bands = [(1i32, 105.0f32, 0.0f32, 0.7f32); 5];
    let mut cf_angle = 60.0f32;
    let mut cf_amount = 0.0f32;
    let mut cf_realism = 0i32;
    let mut blend = 100.0f32;
    let mut gain = 0.0f32;
    let mut name = String::new();
    let mut has_freq = [false; 5];
    let mut has_gain = [false; 5];
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('|') {
            let parts: Vec<&str> = t.split('|').map(|s| s.trim()).collect();
            if parts.len() >= 4 {
                match parts[1].to_lowercase().as_str() {
                    s if s.starts_with("eq") && s.contains("type") => {
                        if let Some(bi) = s
                            .chars()
                            .find(|c| c.is_ascii_digit())
                            .and_then(|c| c.to_digit(10))
                        {
                            let idx = (bi as usize).saturating_sub(1).min(4);
                            bands[idx].0 = match parts[2] {
                                "LSC" | "LS" => 1,
                                "PK" | "PEQ" => 2,
                                "HSC" | "HS" => 3,
                                _ => 0,
                            };
                        }
                    }
                    s if s.starts_with("eq") && s.contains("freq") => {
                        if let Some(bi) = s
                            .chars()
                            .find(|c| c.is_ascii_digit())
                            .and_then(|c| c.to_digit(10))
                        {
                            let idx = (bi as usize).saturating_sub(1).min(4);
                            if let Ok(v) = parts[2].parse() {
                                bands[idx].1 = v;
                                has_freq[idx] = true;
                            }
                        }
                    }
                    s if s.starts_with("eq") && s.contains("gain") => {
                        if let Some(bi) = s
                            .chars()
                            .find(|c| c.is_ascii_digit())
                            .and_then(|c| c.to_digit(10))
                        {
                            let idx = (bi as usize).saturating_sub(1).min(4);
                            if let Ok(v) = parts[2].parse() {
                                bands[idx].2 = v;
                                has_gain[idx] = true;
                            }
                        }
                    }
                    s if s.starts_with("eq") && s.contains('q') => {
                        if let Some(bi) = s
                            .chars()
                            .find(|c| c.is_ascii_digit())
                            .and_then(|c| c.to_digit(10))
                        {
                            let idx = (bi as usize).saturating_sub(1).min(4);
                            if let Ok(v) = parts[2].parse() {
                                bands[idx].3 = v;
                            }
                        }
                    }
                    "crossfeed angle" => {
                        if let Ok(v) = parts[2].parse() {
                            cf_angle = v;
                        }
                    }
                    "crossfeed amount" => {
                        if let Ok(v) = parts[2].parse() {
                            cf_amount = v;
                        }
                    }
                    "crossfeed realism" => {
                        cf_realism = match parts[2] {
                            "LIFELIKE" => 1,
                            "HYPERREAL" | "HYPERREALISTIC" => 2,
                            _ => 0,
                        };
                    }
                    "blend" => {
                        if let Ok(v) = parts[2].parse() {
                            blend = v;
                        }
                    }
                    "gain" => {
                        if let Ok(v) = parts[2].parse() {
                            gain = v;
                        }
                    }
                    _ => {}
                }
            }
        }
        if t.starts_with("# ") && !t.starts_with("## ") {
            name = t.trim_start_matches("# ").trim().to_string();
        }
    }
    if has_freq.iter().all(|&h| h) && has_gain.iter().all(|&h| h) {
        Some(AetherProfile {
            name,
            bands,
            cf_angle,
            cf_amount,
            cf_realism,
            blend,
            gain,
        })
    } else {
        None
    }
}

pub fn scan_aether_presets(dir: &Path) -> Vec<PresetEntry> {
    let mut v = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "md")
                && let Ok(c) = std::fs::read_to_string(&p)
                && let Some(mut pf) = parse_aether_preset(&c)
            {
                if pf.name.is_empty() {
                    pf.name = p
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unnamed")
                        .to_string();
                }
                v.push((pf.name.clone(), p, pf));
            }
        }
    }
    v
}

// ─── Apply / build ───────────────────────────────────────────────────────────

pub fn apply_profile(ctx: &LxPluginContext<AetherParams>, p: &AetherProfile) {
    let band_ids = [
        (P::Eq1Type, P::Eq1Freq, P::Eq1Gain, P::Eq1Q),
        (P::Eq2Type, P::Eq2Freq, P::Eq2Gain, P::Eq2Q),
        (P::Eq3Type, P::Eq3Freq, P::Eq3Gain, P::Eq3Q),
        (P::Eq4Type, P::Eq4Freq, P::Eq4Gain, P::Eq4Q),
        (P::Eq5Type, P::Eq5Freq, P::Eq5Gain, P::Eq5Q),
    ];
    for (i, &(tp, fp, gp, qp)) in band_ids.iter().enumerate() {
        let (tc, fc, gn, q) = p.bands[i];
        ctx.automate(tp, discrete_norm(tc.max(0) as usize, 4));
        ctx.automate(fp, freq_to_norm(fc));
        ctx.automate(gp, gain_to_norm(gn));
        ctx.automate(qp, q_to_norm(q));
    }
    ctx.automate(P::Blend, (p.blend as f64 / 100.0).clamp(0.0, 1.0));
    ctx.automate(
        P::CfAngle,
        ((p.cf_angle as f64 - 30.0) / 45.0).clamp(0.0, 1.0),
    );
    ctx.automate(P::CfAmount, (p.cf_amount as f64 / 100.0).clamp(0.0, 1.0));
    ctx.automate(P::CfRealism, discrete_norm(p.cf_realism.max(0) as usize, 3));
    ctx.automate(P::Gain, ((p.gain as f64 + 12.0) / 24.0).clamp(0.0, 1.0));
}

fn eq_vals(params: &AetherParams, i: usize) -> (i32, f32, f32, f32) {
    match i {
        0 => (
            params.eq1_type.value_i32(),
            params.eq1_freq.raw_target() as f32,
            params.eq1_gain.raw_target() as f32,
            params.eq1_q.raw_target() as f32,
        ),
        1 => (
            params.eq2_type.value_i32(),
            params.eq2_freq.raw_target() as f32,
            params.eq2_gain.raw_target() as f32,
            params.eq2_q.raw_target() as f32,
        ),
        2 => (
            params.eq3_type.value_i32(),
            params.eq3_freq.raw_target() as f32,
            params.eq3_gain.raw_target() as f32,
            params.eq3_q.raw_target() as f32,
        ),
        3 => (
            params.eq4_type.value_i32(),
            params.eq4_freq.raw_target() as f32,
            params.eq4_gain.raw_target() as f32,
            params.eq4_q.raw_target() as f32,
        ),
        _ => (
            params.eq5_type.value_i32(),
            params.eq5_freq.raw_target() as f32,
            params.eq5_gain.raw_target() as f32,
            params.eq5_q.raw_target() as f32,
        ),
    }
}

pub fn profile_from_params(params: &AetherParams, name: &str) -> AetherProfile {
    AetherProfile {
        name: name.to_string(),
        bands: [
            eq_vals(params, 0),
            eq_vals(params, 1),
            eq_vals(params, 2),
            eq_vals(params, 3),
            eq_vals(params, 4),
        ],
        cf_angle: params.cf_angle.raw_target() as f32,
        cf_amount: params.cf_amount.raw_target() as f32,
        cf_realism: params.cf_realism.value_i32(),
        blend: params.blend.raw_target() as f32,
        gain: params.gain.raw_target() as f32,
    }
}

pub fn build_profile_md(params: &AetherParams) -> String {
    let mut s = String::from(
        "---\nplugin: aether\ntype: preset\n---\n\n> Warning: Do NOT modify column names or table structure.\n\n## Parameter\n\n| Parameter | Wert | Einheit |\n|---|---|---|\n",
    );
    for i in 0..5 {
        let (tc, fc, gn, q) = eq_vals(params, i);
        s.push_str(&format!(
            "| EQ{} Type | {} | |\n",
            i + 1,
            band_type_label(tc)
        ));
        s.push_str(&format!("| EQ{} Freq | {:.0} | Hz |\n", i + 1, fc));
        s.push_str(&format!("| EQ{} Gain | {:.1} | dB |\n", i + 1, gn));
        s.push_str(&format!("| EQ{} Q | {:.2} | |\n", i + 1, q));
    }
    s.push_str(&format!(
        "| Crossfeed Angle | {:.0} | ° |\n",
        params.cf_angle.raw_target() as f32
    ));
    s.push_str(&format!(
        "| Crossfeed Amount | {:.0} | % |\n",
        params.cf_amount.raw_target() as f32
    ));
    s.push_str(&format!(
        "| Crossfeed Realism | {} | |\n",
        realism_label(params.cf_realism.value_i32())
    ));
    s.push_str(&format!(
        "| Blend | {:.0} | % |\n",
        params.blend.raw_target() as f32
    ));
    s.push_str(&format!(
        "| Gain | {:.1} | dB |\n",
        params.gain.raw_target() as f32
    ));
    s
}

// ─── Config / cache ──────────────────────────────────────────────────────────

fn last_profile_cache_path() -> PathBuf {
    get_plugin_dir("Aether").join("last_profile.json")
}

pub fn load_cached_last_profile() -> Option<AetherProfile> {
    let path = last_profile_cache_path();
    if let Ok(content) = std::fs::read_to_string(path)
        && let Ok(profile) = serde_json::from_str::<AetherProfile>(&content)
    {
        return Some(profile);
    }
    None
}

pub fn save_last_preset(vault_path: &Option<String>, profile: &AetherProfile) {
    // Merge-update: never clear vault_path when caller passes None (select
    // without vault / lock fail used to wipe APPDATA config on every pick).
    let mut cfg = load_config("Aether");
    if let Some(vp) = vault_path {
        cfg.vault_path = if vp.trim().is_empty() {
            None
        } else {
            Some(vp.clone())
        };
    }
    if !profile.name.trim().is_empty() {
        cfg.last_preset = Some(profile.name.clone());
    }
    let _ = save_config("Aether", &cfg);
    let path = last_profile_cache_path();
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
    if let Ok(content) = serde_json::to_string_pretty(profile) {
        let _ = std::fs::write(path, content);
    }
}

/// Resolve a profile from cache / built-ins only.
/// Never does a synchronous vault directory scan (that can hang the UI thread
/// on large Obsidian vaults). Callers must wait for background scan cache.
pub fn find_profile(
    name: &str,
    _vault_path: &Option<String>,
    cache: &[PresetEntry],
) -> Option<AetherProfile> {
    if name == "Harman Flat" {
        return Some(harman_flat_profile());
    }
    if let Some((_, _, p)) = cache.iter().find(|(n, _, _)| n == name) {
        return Some(p.clone());
    }
    // Single-file load by name under the local presets dir only (small, bounded).
    let local = get_plugin_dir("Aether").join("presets");
    let candidate = local.join(format!("{name}.md"));
    if candidate.is_file()
        && let Ok(c) = std::fs::read_to_string(&candidate)
        && let Some(mut pf) = parse_aether_preset(&c)
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
        _ => get_plugin_dir("Aether").join("presets"),
    }
}

// ─── Background scan ─────────────────────────────────────────────────────────

pub fn spawn_vault_scan(vp: String, pending: Arc<PendingPresets<PresetEntry>>, generation: u32) {
    spawn_vault_scan_impl(pending, generation, move || {
        let mut scanned = scan_aether_presets(Path::new(&vp));
        // Also pick up local plugin presets that aren't in the vault.
        let local = get_plugin_dir("Aether").join("presets");
        for entry in scan_aether_presets(&local) {
            if !scanned.iter().any(|(n, _, _)| n == &entry.0) {
                scanned.push(entry);
            }
        }
        scanned
    });
}

/// Build display names: built-ins first, then scanned.
pub fn merge_preset_names(scanned: &[PresetEntry]) -> Vec<String> {
    let mut names = default_preset_names();
    for (n, _, _) in scanned {
        if !names.iter().any(|x| x == n) {
            names.push(n.clone());
        }
    }
    names
}
