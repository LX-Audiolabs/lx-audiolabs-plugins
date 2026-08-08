#![allow(unsafe_op_in_unsafe_fn)]

use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex};
use std::sync::{Arc, RwLock};
use aura::prelude::*;

use aura_dsp::analysis::{SPECTRUM_BINS, ShmClaimShared};
use aura_shm::{now_ms, relay_hub, resolve_from_consumers, resolve_relay_target, RelayHub};

mod editor;
mod process;

pub(crate) const FFT_SIZE: usize = 2048;
/// How often the DSP side re-resolves its publish target against the SHM
/// consumer table. Well under `STALE_MS` (500), so heartbeats never go stale;
/// target-param changes bypass the interval and resolve immediately.
const RESOLVE_INTERVAL_MS: u64 = 250;

// ─── Parameters ──────────────────────────────────────────────────────────────
// AURA requires at least one Param field. `process()` always copies input to
// output regardless of bypass state (pure pass-through analyzer), so a visible
// Bypass control is a no-op from the user's perspective - hidden per user request.
// ponytail: _flush_sentinel FloatParam works around truce flush edge-case
// with single-BoolParam plugins (clap-validator state-reproducibility-flush).
// Hidden per user request - re-check clap-validator after this change; if the
// edge-case resurfaces (validator only tests non-hidden params), un-hide this one.

#[derive(Params)]
pub struct LucentRelayParams {
    #[param(id = 1, name = "Bypass", default = 0, flags = "bypass|hidden")]
    pub bypass: BoolParam,
    #[param(
        id = 2,
        name = "_flush_sentinel",
        default = 0.0,
        range = "linear(0.0, 1.0)",
        flags = "hidden"
    )]
    pub _flush_sentinel: FloatParam,
    #[persist]
    pub name: RwLock<String>,
    #[persist]
    pub target: RwLock<String>,
    /// Live (name, target) mirror for the liveness thread — same Arc the
    /// editor and audio thread share via AURA's `Arc<LucentRelayParams>`.
    /// Updated from `process()` / editor edits so
    /// `touch()` keeps working when transport is stopped.
    #[skip]
    pub live: Arc<RwLock<(String, String)>>,
    /// SHM publisher slot + generation — shared with the editor so claim/touch
    /// work before `reset()` runs (transport stopped).
    #[skip]
    pub shm: Arc<ShmClaimShared>,
}

pub(crate) fn read_persisted(params: &LucentRelayParams) -> (String, String) {
    let name = params.name.read().map(|s| s.clone()).unwrap_or_default();
    let target = params.target.read().map(|s| s.clone()).unwrap_or_default();
    (name, target)
}

pub(crate) fn sync_live(params: &LucentRelayParams) {
    let pair = read_persisted(params);
    if let Ok(mut live) = params.live.write() {
        *live = pair;
    }
}

/// Resolve `selected` against one consumer-list snapshot and persist the fix
/// when the persisted target went stale. Returns the target to publish with.
fn resolve_and_reconcile(
    hub: &RelayHub,
    params: &LucentRelayParams,
    consumers: &mut Vec<String>,
    selected: &str,
    now_ms: u64,
) -> String {
    hub.read_consumers_into(now_ms, consumers);
    let resolved = resolve_from_consumers(selected, consumers).unwrap_or_default();
    if !selected.is_empty() && resolved != selected {
        // persisted target went stale — write the resolution back
        let mut changed = false;
        if let Ok(mut t) = params.target.write()
            && *t != resolved
        {
            *t = resolved.clone();
            changed = true;
        }
        if changed {
            sync_live(params);
        }
    }
    resolved
}

/// Editor tick path — claim publisher slot and refresh heartbeat without transport.
pub(crate) fn editor_publish_heartbeat(params: &LucentRelayParams) {
    use std::sync::atomic::Ordering;
    thread_local! {
        static CONSUMERS_SCRATCH: std::cell::RefCell<Vec<String>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }
    let now_ms = now_ms();
    let Some(hub) = relay_hub() else {
        return;
    };

    let mut slot = params.shm.slot.load(Ordering::Acquire);
    let mut generation = params.shm.generation.load(Ordering::Acquire);
    if slot < 0 {
        let Some((s, g)) = hub.claim_slot(now_ms) else {
            return;
        };
        slot = s as i32;
        generation = g;
        params.shm.slot.store(slot, Ordering::Release);
        params.shm.generation.store(generation, Ordering::Release);
    }

    let (raw, sel) = read_persisted(params);
    let target = CONSUMERS_SCRATCH.with(|scratch| {
        let mut cons = scratch.borrow_mut();
        cons.clear();
        resolve_and_reconcile(hub, params, &mut cons, &sel, now_ms)
    });
    let label = if raw.is_empty() {
        format!("Relay {}", slot as u8 + 1)
    } else {
        raw
    };
    let _ = hub.touch(slot as u8, generation, &label, &target, now_ms);
}

// ─── Plugin ───────────────────────────────────────────────────────────────────

pub struct LucentRelay;

pub struct LucentRelayDspState {
    pub(crate) shm_state: Arc<ShmClaimShared>,
    pub(crate) fft_fwd: Arc<dyn RealToComplex<f32>>,
    pub(crate) fft_input: Vec<f32>,
    pub(crate) fft_write_pos: usize,
    pub(crate) fft_hann: Vec<f32>,
    pub(crate) fft_windowed: Vec<f32>,
    pub(crate) fft_output: Vec<Complex<f32>>,
    pub(crate) fft_bins: Vec<f32>,
    pub(crate) sample_rate: f32,
    pub(crate) claimed_slot: Option<u8>,
    pub(crate) claimed_generation: u32,
    pub(crate) cached_name: String,
    pub(crate) fallback_label: String,
    pub(crate) cached_target: String,
    /// Last resolved publish target — refreshed by `resolve_target` at
    /// `RESOLVE_INTERVAL_MS` cadence (or instantly on target-param change)
    /// instead of re-scanning the SHM consumer table every block/hop.
    pub(crate) resolved_target: String,
    pub(crate) last_resolve_ms: Option<u64>,
    /// Reused consumer-list buffer for `read_consumers_into` (no per-resolve alloc).
    pub(crate) consumers_scratch: Vec<String>,
    pub(crate) liveness: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub(crate) instance_key: usize,
}

impl LucentRelayDspState {
    fn build_fft() -> (Arc<dyn RealToComplex<f32>>, Vec<Complex<f32>>) {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft_fwd = planner.plan_fft_forward(FFT_SIZE);
        let fft_output = fft_fwd.make_output_vec();
        (fft_fwd, fft_output)
    }
}

impl Default for LucentRelayDspState {
    fn default() -> Self {
        let hann: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let x = i as f32 / (FFT_SIZE - 1) as f32;
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * x).cos())
            })
            .collect();
        let (fft_fwd, fft_output) = Self::build_fft();
        Self {
            shm_state: Arc::new(ShmClaimShared::default()),
            fft_fwd,
            fft_input: vec![0.0; FFT_SIZE],
            fft_write_pos: 0,
            fft_hann: hann,
            fft_windowed: vec![0.0; FFT_SIZE],
            fft_output,
            fft_bins: vec![-90.0; SPECTRUM_BINS],
            sample_rate: 48000.0,
            claimed_slot: None,
            claimed_generation: 0,
            cached_name: String::new(),
            fallback_label: String::from("Relay"),
            cached_target: String::new(),
            resolved_target: String::new(),
            last_resolve_ms: None,
            consumers_scratch: Vec::new(),
            liveness: None,
            instance_key: 0,
        }
    }
}

impl LucentRelayDspState {
    fn claim_slot(&mut self) {
        use std::sync::atomic::Ordering;
        if self.claimed_slot.is_none() {
            let adopted = self.shm_state.slot.load(Ordering::Acquire);
            if adopted >= 0 {
                self.claimed_slot = Some(adopted as u8);
                self.claimed_generation = self.shm_state.generation.load(Ordering::Acquire);
                self.fallback_label = format!("Relay {}", adopted as u8 + 1);
            } else if let Some(hub) = relay_hub()
                && let Some((slot, generation)) = hub.claim_slot(now_ms())
            {
                self.claimed_slot = Some(slot);
                self.claimed_generation = generation;
                self.fallback_label = format!("Relay {}", slot + 1);
            }
        }
        self.shm_state.slot.store(
            self.claimed_slot.map(|s| s as i32).unwrap_or(-1),
            Ordering::Release,
        );
        self.shm_state.generation.store(
            self.claimed_generation,
            Ordering::Release,
        );
    }

    fn spawn_liveness_thread(&mut self, params: &LucentRelayParams) {
        use std::sync::atomic::Ordering;
        if let Some(alive) = self.liveness.take() {
            alive.store(false, Ordering::Release);
        }
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        self.liveness = Some(alive.clone());
        let ss = self.shm_state.clone();
        let live = params.live.clone();
        std::thread::spawn(move || {
            while alive.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let slot = ss.slot.load(Ordering::Acquire);
                if slot < 0 {
                    continue;
                }
                let generation = ss.generation.load(Ordering::Acquire);
                if let Some(hub) = relay_hub() {
                    let now = now_ms();
                    let (raw, sel) = live.read().map(|g| g.clone()).unwrap_or_default();
                    if let Some(target) = resolve_relay_target(hub, &sel, now) {
                        let label = if raw.is_empty() {
                            format!("Relay {}", slot + 1)
                        } else {
                            raw
                        };
                        let _touched = hub.touch(slot as u8, generation, &label, &target, now);
                    }
                }
            }
        });
    }

    /// Refresh `resolved_target` from the SHM consumer table — at most once
    /// per `RESOLVE_INTERVAL_MS`, or immediately when `force` (target param
    /// changed / state loaded). Keeps the per-block and per-FFT-hop paths
    /// free of consumer-table scans.
    pub(crate) fn resolve_target(&mut self, params: &LucentRelayParams, now_ms: u64, force: bool) {
        if !force
            && let Some(last) = self.last_resolve_ms
            && now_ms.wrapping_sub(last) < RESOLVE_INTERVAL_MS
        {
            return;
        }
        let Some(hub) = relay_hub() else {
            return;
        };
        self.last_resolve_ms = Some(now_ms);
        self.resolved_target = resolve_and_reconcile(
            hub,
            params,
            &mut self.consumers_scratch,
            &self.cached_target,
            now_ms,
        );
        if self.resolved_target != self.cached_target {
            self.cached_target = self.resolved_target.clone();
        }
    }

    pub(crate) fn publish_fft(&mut self, now_ms: u64) {
        let Some(slot) = self.claimed_slot else {
            return;
        };
        let Some(hub) = relay_hub() else { return };
        let label: &str = if self.cached_name.is_empty() {
            &self.fallback_label
        } else {
            &self.cached_name
        };
        {
            let ok = hub.write(
                slot,
                self.claimed_generation,
                label,
                &self.resolved_target,
                &self.fft_bins,
                &[-90.0f32; 5],
                now_ms,
            );
            if !ok {
                self.claimed_slot = None;
            }
        }
    }
}

impl PluginLogic for LucentRelay {
    type Params = LucentRelayParams;
    type DspState = LucentRelayDspState;

    fn info() -> PluginInfo {
        let mut info = PluginInfo::new(
            "Lucent Relay",
            "LX Audiolabs",
            env!("CARGO_PKG_VERSION"),
            "lucentrelay",
        );
        // Stable ship IDs — must match pre-AURA truce Lucent Relay (Bitwig keys
        // sessions on clap id; com.lx-audiolabs.* breaks existing projects).
        info.clap_id = "be.lxndr.lucentrelay";
        info.vst3_id = "be.lxndr.lucentrelay";
        info.lv2_uri = "https://lx-audiolabs.com/lv2/lucentrelay";
        info.category = PluginCategory::Analyzer;
        info
    }

    fn bus_layouts() -> Vec<BusLayout> {
        vec![BusLayout::stereo()]
    }

    fn init(params: &Self::Params, _sample_rate: f64) -> Self::DspState {
        // Custom init (truce parity): adopt the shared SHM slot, cache the
        // persisted name/target, claim a publisher slot and spawn the liveness
        // thread. AURA calls `reset` on activate right after `init` (same as
        // truce), which sets sample_rate and re-claims/re-spawns —
        // `spawn_liveness_thread` kills the old thread via its liveness flag.
        let mut state = LucentRelayDspState::default();
        state.shm_state = params.shm.clone();
        state.instance_key = params as *const _ as usize;
        let (name, target) = read_persisted(params);
        state.cached_name = name;
        state.cached_target = target;
        sync_live(params);
        state.claim_slot();
        state.spawn_liveness_thread(params);
        state
    }

    fn reset(state: &mut LucentRelayDspState, params: &LucentRelayParams, config: &AudioConfig) {
        state.sample_rate = config.sample_rate as f32;
        sync_live(params);
        state.claim_slot();
        state.spawn_liveness_thread(params);
    }

    fn process(
        state: &mut LucentRelayDspState,
        params: &LucentRelayParams,
        buffer: &mut AudioBuffer<'_, f32>,
        _ctx: &mut ProcessContext,
    ) -> ProcessStatus {
        process::run(state, params, buffer)
    }

    // AURA has no `state_changed` hook. Its work (re-sync cached_name /
    // cached_target from the persisted fields, force a target re-resolve,
    // sync_live) is preserved lazily at the top of `process::run`: each block
    // re-reads the persisted fields, updates the caches on change, forces the
    // resolve on a target change, and calls `sync_live` — so a host state load
    // is picked up on the first block after load. While transport is stopped
    // the editor tick (`editor_publish_heartbeat`) reads the persisted fields
    // directly, so heartbeats stay correct there too.

    fn editor(params: Arc<Self::Params>) -> Option<Box<dyn Editor>> {
        Some(editor::build_editor(params))
    }
}

impl Drop for LucentRelayDspState {
    fn drop(&mut self) {
        if let Some(alive) = self.liveness.take() {
            alive.store(false, std::sync::atomic::Ordering::Release);
        }
        if let Some(slot) = self.claimed_slot.take()
            && let Some(hub) = relay_hub()
        {
            hub.release_slot(slot);
        }
    }
}

#[cfg(feature = "clap")]
aura::export!(LucentRelay);

#[cfg(feature = "vst3")]
aura::export_vst3!(LucentRelay);

#[cfg(feature = "lv2")]
aura::export_lv2!(LucentRelay);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_pass_through() {
        let frames = 2048;
        let out = aura_test::process_with_input::<LucentRelay>(
            &[vec![0.5; frames], vec![0.5; frames]],
            frames,
        );
        aura_test::assert_no_nans(&out);
        aura_test::assert_nonzero(&out);
    }

    #[test]
    fn state_round_trips() {
        aura_test::assert_state_round_trip::<LucentRelay>();
    }
}
