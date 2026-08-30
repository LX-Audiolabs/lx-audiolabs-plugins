//! Lucent Relay — aura-editor (name, target, connection status).

use std::sync::{Arc, Mutex};

use aura::prelude::*;
use aura_editor::typed::*;
use aura_editor::ui_zoom::UiZoom;
use lx_shm::*;
use slint::{ModelRc, SharedString, VecModel};

use crate::{LucentRelayParams, editor_publish_heartbeat, sync_live};

slint::include_modules!();

const WINDOW_W: u32 = 300;
const WINDOW_H: u32 = 160;
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn names_model(names: &[String]) -> ModelRc<SharedString> {
    let v: Vec<SharedString> = names
        .iter()
        .map(|s| SharedString::from(s.as_str()))
        .collect();
    ModelRc::new(VecModel::from(v))
}

/// Last values pushed to the UI — setters only fire on change so the
/// names model is not rebuilt every frame.
struct SyncCache {
    opts: Vec<String>,
    idx: Option<i32>,
    connected: Option<bool>,
    status: String,
}

pub fn build_editor(params: Arc<LucentRelayParams>) -> Box<dyn Editor> {
    let sync_cache = Arc::new(Mutex::new(SyncCache {
        opts: Vec::new(),
        idx: None,
        connected: None,
        status: String::new(),
    }));
    // Compact UI — always 100%, ignore global zoom preference.
    let ui_zoom = UiZoom::with_percent(WINDOW_W, WINDOW_H, 100);
    LxSlintEditor::new_with_zoom(
        params.clone(),
        ui_zoom,
        {
            let params = params.clone();
            let sync_cache = Arc::clone(&sync_cache);
            move |_state: LxPluginContext<LucentRelayParams>| {
                // New Slint component each open; wipe dirty mirrors so labels
                // are re-pushed (stale cache → empty / wrong UI until a value moves).
                if let Ok(mut c) = sync_cache.lock() {
                    *c = SyncCache {
                        opts: Vec::new(),
                        idx: None,
                        connected: None,
                        status: String::new(),
                    };
                }
                let ui = LucentRelayUi::new().expect("LucentRelayUi::new");

                ui.set_version(SharedString::from(VERSION));
                let initial_name = params.name.read().map(|s| s.clone()).unwrap_or_default();
                ui.set_relay_name(SharedString::from(initial_name.as_str()));

                let p = params.clone();
                ui.on_relay_name_changed(move |txt: SharedString| {
                    if let Ok(mut n) = p.name.write() {
                        *n = txt.as_str().to_string();
                    }
                    // Immediate live mirror + SHM touch so Lucent button renames
                    // without waiting for the next editor tick.
                    sync_live(&p);
                    editor_publish_heartbeat(&p);
                });

                let p = params.clone();
                ui.on_target_selected(move |idx: i32| {
                    let idx = idx.max(0) as usize;
                    // 0 = broadcast (empty target); 1.. = consumer from last options
                    // Options are rebuilt in sync; store by index into current hub list.
                    let now_ms = now_ms();
                    let lucents = relay_hub()
                        .map(|hub| hub.read_consumers(now_ms))
                        .unwrap_or_default();
                    let val = if idx == 0 {
                        String::new()
                    } else {
                        lucents.get(idx - 1).cloned().unwrap_or_default()
                    };
                    if let Ok(mut t) = p.target.write() {
                        *t = val;
                    }
                    sync_live(&p);
                });

                ui
            }
        },
        {
            let params_sync = params.clone();
            move |ui: &LucentRelayUi, _state: &LxPluginContext<LucentRelayParams>| {
                editor_publish_heartbeat(&params_sync);

                let now_ms = now_ms();
                let lucent_list = relay_hub()
                    .map(|hub| hub.read_consumers(now_ms))
                    .unwrap_or_default();

                let current = params_sync
                    .target
                    .read()
                    .map(|s| s.clone())
                    .unwrap_or_default();

                // Soft-reconcile stale target (Vizia process path). Hard-clear
                // caused connect flicker when Lucent renames Hub N → custom name:
                // sole consumer auto-retargets; multi → broadcast; match stays.
                use lx_shm::resolve_from_consumers;
                let resolved = resolve_from_consumers(&current, &lucent_list).unwrap_or_default();
                if resolved != current {
                    if let Ok(mut t) = params_sync.target.write() {
                        *t = resolved;
                    }
                    sync_live(&params_sync);
                }

                let current = params_sync
                    .target
                    .read()
                    .map(|s| s.clone())
                    .unwrap_or_default();

                let mut cache = sync_cache.lock().unwrap();

                let mut opts = vec!["(broadcast)".to_string()];
                opts.extend(lucent_list.iter().cloned());
                if cache.opts != opts {
                    ui.set_target_names(names_model(&opts));
                    cache.opts = opts;
                }

                let idx = if current.is_empty() {
                    0
                } else {
                    lucent_list
                        .iter()
                        .position(|l| *l == current)
                        .map(|i| i + 1)
                        .unwrap_or(0)
                };
                if cache.idx != Some(idx as i32) {
                    ui.set_target_index(idx as i32);
                    cache.idx = Some(idx as i32);
                }

                let connected = relay_hub()
                    .map(|hub| {
                        if current.is_empty() {
                            !hub.read_consumers(now_ms).is_empty()
                        } else {
                            hub.consumer_exists(&current, now_ms)
                        }
                    })
                    .unwrap_or(false);
                if cache.connected != Some(connected) {
                    ui.set_connected(connected);
                    cache.connected = Some(connected);
                }
                let status = if connected {
                    "connected"
                } else if lucent_list.is_empty() {
                    "no Lucent online"
                } else {
                    "select target"
                };
                if cache.status != status {
                    ui.set_status_text(SharedString::from(status));
                    cache.status = status.to_string();
                }
            }
        },
    )
    .resizable(false)
    .into()
}
