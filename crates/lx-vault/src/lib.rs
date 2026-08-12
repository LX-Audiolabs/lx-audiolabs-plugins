//! LX product vault helpers — **not** part of the AURA framework.
//!
//! Plugin config (`config.json`), AppData/config dirs, and Markdown
//! frontmatter parsing for Obsidian-style presets/profiles/SNAP notes.
//!
//! Profile types and SNAP→MD writers live in individual plugins /
//! `lx-editor-utils::snap`.

use serde::{Deserialize, Serialize};

// ─── Plugin Config ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PluginConfig {
    #[serde(default)]
    pub vault_path: Option<String>,
    /// Last preset the user selected — default for fresh instances.
    /// `serde(default)` keeps old config.json without this field parseable.
    #[serde(default)]
    pub last_preset: Option<String>,
}

#[must_use]
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

#[must_use]
pub fn load_config(plugin_name: &str) -> PluginConfig {
    let path = get_plugin_dir(plugin_name).join("config.json");
    if let Ok(content) = std::fs::read_to_string(path)
        && let Ok(config) = serde_json::from_str::<PluginConfig>(&content)
    {
        return config;
    }
    PluginConfig::default()
}

pub fn save_config(plugin_name: &str, config: &PluginConfig) -> Result<(), std::io::Error> {
    let path = get_plugin_dir(plugin_name).join("config.json");
    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(path, content)
}

/// Persist vault path (empty → clear). Loads existing config so `last_preset` is kept.
pub fn set_vault_path(plugin_name: &str, vault_path: Option<String>) -> Result<(), std::io::Error> {
    let mut cfg = load_config(plugin_name);
    cfg.vault_path = vault_path.filter(|s| !s.trim().is_empty());
    save_config(plugin_name, &cfg)
}

/// Persist last selected preset name. Loads existing config so `vault_path` is kept.
/// Empty names are ignored (never wipe a good last_preset by accident).
pub fn set_last_preset(plugin_name: &str, name: &str) -> Result<(), std::io::Error> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(());
    }
    let mut cfg = load_config(plugin_name);
    cfg.last_preset = Some(name.to_string());
    save_config(plugin_name, &cfg)
}

// ─── Frontmatter parsing ─────────────────────────────────────────────────────

#[must_use]
pub fn parse_frontmatter(content: &str) -> std::collections::HashMap<String, String> {
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

/// Returns the `plugin:` field from frontmatter, or `None` if missing.
#[must_use]
pub fn preset_plugin_name(content: &str) -> Option<String> {
    parse_frontmatter(content).remove("plugin")
}
