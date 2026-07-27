//! Lucent Relay — lx-slint-editor (name, target, connection status).

use std::sync::{Arc, Mutex};

use lx_analysis::relay_hub;
use lx_slint_editor::{LxSlintEditor, PluginContext};
use slint::{ModelRc, SharedString, VecModel};
use truce_core::editor::Editor;

use crate::{editor_publish_heartbeat, sync_live, LucentRelayParams};

slint::include_modules!();

const WINDOW_W: u32 = 260;
const WINDOW_H: u32 = 160;
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn names_model(names: &[String]) -> ModelRc<SharedString> {
    let v: Vec<SharedString> = names.iter().map(|s| SharedString::from(s.as_str())).collect();
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
    let sync_cache = Mutex::new(SyncCache {
        opts: Vec::new(),
        idx: None,
        connected: None,
        status: String::new(),
    });
    LxSlintEditor::new(
        params.clone(),
        (WINDOW_W, WINDOW_H),
        {
            let params = params.clone();
            move |_state: PluginContext<LucentRelayParams>| {
            let ui = LucentRelayUi::new().expect("LucentRelayUi::new");

            ui.set_version(SharedString::from(VERSION));
            let initial_name = params.name.read().map(|s| s.clone()).unwrap_or_default();
            ui.set_relay_name(SharedString::from(initial_name.as_str()));

            let p = params.clone();
            ui.on_relay_name_changed(move |txt: SharedString| {
                if let Ok(mut n) = p.name.write() {
                    *n = txt.as_str().to_string();
                }
                sync_live(&p);
            });

            let p = params.clone();
            ui.on_target_selected(move |idx: i32| {
                let idx = idx.max(0) as usize;
                // 0 = broadcast (empty target); 1.. = consumer from last options
                // Options are rebuilt in sync; store by index into current hub list.
                let now_ms = lx_analysis::shm::now_ms();
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
            move |ui: &LucentRelayUi, _state: &PluginContext<LucentRelayParams>| {
                editor_publish_heartbeat(&params_sync);

                let now_ms = lx_analysis::shm::now_ms();
                let lucent_list = relay_hub()
                    .map(|hub| hub.read_consumers(now_ms))
                    .unwrap_or_default();

                let current = params_sync
                    .target
                    .read()
                    .map(|s| s.clone())
                    .unwrap_or_default();

                // Auto-target sole consumer
                if current.is_empty() && lucent_list.len() == 1 {
                    let auto = lucent_list[0].clone();
                    if let Ok(mut t) = params_sync.target.write() {
                        *t = auto;
                    }
                    sync_live(&params_sync);
                } else if !current.is_empty() && !lucent_list.contains(&current) {
                    if let Ok(mut t) = params_sync.target.write() {
                        t.clear();
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
