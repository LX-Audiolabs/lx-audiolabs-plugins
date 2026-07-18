//! Lucent Relay Slint editor — name, target, connection status.
//! truce-slint software renderer.

use std::sync::Arc;

use slint::{ModelRc, SharedString, VecModel};
use truce::prelude::*;
use truce_core::editor::Editor;
use truce_slint::{PluginContext, SlintEditor, SyncFn};

use crate::{editor_publish_heartbeat, sync_live, LucentRelayParams};
use lx_analysis::relay_hub;

slint::include_modules!();

const WINDOW_W: u32 = 260;
const WINDOW_H: u32 = 160;
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn names_model(names: &[String]) -> ModelRc<SharedString> {
    let v: Vec<SharedString> = names.iter().map(|s| SharedString::from(s.as_str())).collect();
    ModelRc::new(VecModel::from(v))
}

pub fn build_editor(params: Arc<LucentRelayParams>) -> Box<dyn Editor> {
    SlintEditor::new(
        params.clone(),
        (WINDOW_W, WINDOW_H),
        move |_state: PluginContext<LucentRelayParams>| -> SyncFn<LucentRelayParams> {
            let ui = match LucentRelayUi::new() {
                Ok(u) => u,
                Err(e) => {
                    tracing::error!("LucentRelayUi::new failed: {e:?}");
                    return Box::new(|_: &PluginContext<LucentRelayParams>| {});
                }
            };

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
            ui.on_target_index_changed(move |idx: i32| {
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

            let params_sync = params.clone();
            Box::new(move |_state: &PluginContext<LucentRelayParams>| {
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

                let mut opts = vec!["(broadcast)".to_string()];
                opts.extend(lucent_list.iter().cloned());
                ui.set_target_names(names_model(&opts));

                let idx = if current.is_empty() {
                    0
                } else {
                    lucent_list
                        .iter()
                        .position(|l| *l == current)
                        .map(|i| i + 1)
                        .unwrap_or(0)
                };
                ui.set_target_index(idx as i32);

                let connected = relay_hub()
                    .map(|hub| {
                        if current.is_empty() {
                            !hub.read_consumers(now_ms).is_empty()
                        } else {
                            hub.consumer_exists(&current, now_ms)
                        }
                    })
                    .unwrap_or(false);
                ui.set_connected(connected);
                ui.set_status_text(SharedString::from(if connected {
                    "connected"
                } else if lucent_list.is_empty() {
                    "no Lucent online"
                } else {
                    "select target"
                }));
            })
        },
    )
    .resizable(false)
    .into_editor()
}
