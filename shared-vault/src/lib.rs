use serde::{Deserialize, Serialize};

pub const DEFAULT_TOLERANCES: [f32; 5] = [1.5, 2.0, 3.5, 4.5, 4.5];

// ─── Profile (Preset data) ────────────────────────────────────────────────────

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

fn format_pan_str(pan: f32) -> String {
    if pan.abs() < 0.01 {
        "C".to_string()
    } else if pan < 0.0 {
        format!("L {:.0}%", -pan * 100.0)
    } else {
        format!("R {:.0}%", pan * 100.0)
    }
}

fn parse_pan_str(s: &str) -> f32 {
    let s = s.trim();
    if s.eq_ignore_ascii_case("c") || s.eq_ignore_ascii_case("center") {
        return 0.0;
    }
    if let Some(rest) = s.strip_prefix(|c: char| c == 'L' || c == 'l') {
        if let Ok(n) = rest.trim().trim_end_matches('%').trim().parse::<f32>() {
            return -(n / 100.0).clamp(-1.0, 1.0);
        }
    }
    if let Some(rest) = s.strip_prefix(|c: char| c == 'R' || c == 'r') {
        if let Ok(n) = rest.trim().trim_end_matches('%').trim().parse::<f32>() {
            return (n / 100.0).clamp(-1.0, 1.0);
        }
    }
    0.0
}

// ─── Frontmatter parsing (internal) ──────────────────────────────────────────

fn parse_frontmatter(content: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut lines = content.lines();

    if lines.next().map(|l| l.trim()) != Some("---") {
        return map;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if trimmed.starts_with("- ") {
            continue;
        }
        if let Some(pos) = trimmed.find(':') {
            let key = trimmed[..pos].trim().to_string();
            let val = trimmed[pos + 1..].trim().to_string();
            map.insert(key, val);
        }
    }
    map
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

/// Returns the `plugin:` field from frontmatter, or None if missing
pub fn preset_plugin_name(content: &str) -> Option<String> {
    parse_frontmatter(content).remove("plugin")
}

/// Parse a preset/profile from Markdown — requires plugin: equilibrium frontmatter and all 5 bands
pub fn parse_preset_from_markdown(content: &str) -> Option<Profile> {
    let frontmatter = parse_frontmatter(content);

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
            // Table: | Band | Pan | Width |
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
                    // Parse Pan
                    let pan_str = parts[2];
                    pans[b] = parse_pan_str(pan_str);
                    // Parse Width
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
                    if parts.len() >= 5 {
                        if let Ok(tol) = parts[4].parse::<f32>() {
                            tolerances[b] = tol;
                        }
                    }
                }
            }
        }
        // Mono Floor Hz — single number line under ## Mono Floor
        if !in_stereo_table
            && trimmed.chars().any(|c| c.is_ascii_digit())
            && !trimmed.starts_with('|')
            && !trimmed.contains('#')
        {
            if let Some(hz_str) = trimmed.split_whitespace().next() {
                if let Ok(hz) = hz_str.parse::<f32>() {
                    mono_floor_hz = hz;
                }
            }
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

// ─── Plugin Config ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PluginConfig {
    #[serde(default)]
    pub vault_path: Option<String>,
    /// Last preset the user selected — used as the default for fresh plugin
    /// instances (per-plugin, e.g. Aether). `serde(default)` keeps old
    /// config.json files (without this field) parseable.
    #[serde(default)]
    pub last_preset: Option<String>,
}

pub fn get_plugin_dir(plugin_name: &str) -> std::path::PathBuf {
    let mut path = if let Ok(appdata) = std::env::var("APPDATA") {
        std::path::PathBuf::from(appdata)
    } else if let Ok(home) = std::env::var("HOME") {
        let mut p = std::path::PathBuf::from(home);
        p.push(".config");
        p
    } else {
        std::path::PathBuf::from(".")
    };
    path.push(plugin_name);
    let _ = std::fs::create_dir_all(&path);
    path
}

pub fn load_config(plugin_name: &str) -> PluginConfig {
    let path = get_plugin_dir(plugin_name).join("config.json");
    if let Ok(content) = std::fs::read_to_string(path) {
        if let Ok(config) = serde_json::from_str::<PluginConfig>(&content) {
            return config;
        }
    }
    PluginConfig::default()
}

pub fn save_config(plugin_name: &str, config: &PluginConfig) -> Result<(), std::io::Error> {
    let path = get_plugin_dir(plugin_name).join("config.json");
    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(path, content)
}

// ─── Preset file scanning ─────────────────────────────────────────────────────

pub fn list_custom_presets(
    plugin_name: &str,
    vault_path: Option<&str>,
) -> Vec<(String, std::path::PathBuf, Profile)> {
    let mut presets = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();

    let local_dir = get_plugin_dir(plugin_name).join("presets");
    let _ = std::fs::create_dir_all(&local_dir);

    let mut scan_dir = |dir: &std::path::Path| {
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
                {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Some(mut profile) = parse_preset_from_markdown(&content) {
                            profile.name = stem.clone();
                            presets.push((stem, path, profile));
                        }
                    }
                }
            }
        }
    };

    scan_dir(&local_dir);

    if let Some(vp) = vault_path {
        if !vp.is_empty() {
            let vault_dir = std::path::Path::new(vp);
            scan_dir(vault_dir);
        }
    }

    presets
}
