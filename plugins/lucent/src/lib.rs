#![allow(unsafe_op_in_unsafe_fn)]

use aura::prelude::*;
use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use aura_dsp::analysis::*;
use lx_analysis::product_shared::LucentShared;
use lx_shm::SPECTRUM_BINS;
use lx_shm::*;

/// Live SHM relay feed on the audio path: fixed bins, reused name String.
pub(crate) type RelayFeed = (u8, String, [f32; SPECTRUM_BINS]);

/// Claim a consumer slot (if needed) and refresh the Lucent display name in SHM.
/// Safe from the editor tick — relay discovery must not depend on analyze mode
/// or transport running.
///
/// Prefer `name_bg` (same Arc the audio heartbeat uses). Fall back to `name`.
/// Never treat a failed try_lock as empty name — that would flash "Hub N" in
/// Relay target lists while the user is typing (Vizia always publishes the
/// keystroke text immediately via `write_consumer_name`).
pub(crate) fn editor_ensure_consumer(params: &LucentParams, shared: &LucentShared) {
    let now_ms = now_ms();
    let mut slot = shared.shm.slot.load(Ordering::Acquire);
    if slot < 0
        && let Some(hub) = relay_hub()
        && let Some(claimed) = hub.claim_consumer_slot(now_ms)
    {
        slot = claimed as i32;
        shared.shm.slot.store(slot, Ordering::Release);
    }
    // Re-claim if our cached index looks free/stale (host reload, SHM v bump).
    if slot >= 0
        && let Some(hub) = relay_hub()
        && !hub.consumer_slot_live(slot as u8, now_ms)
    {
        if let Some(claimed) = hub.claim_consumer_slot(now_ms) {
            slot = claimed as i32;
            shared.shm.slot.store(slot, Ordering::Release);
        } else {
            slot = -1;
            shared.shm.slot.store(-1, Ordering::Release);
        }
    }
    if slot < 0 {
        return;
    }
    let raw = params
        .name_bg
        .try_read()
        .map(|n| n.clone())
        .or_else(|_| params.name.try_read().map(|n| n.clone()))
        .unwrap_or_else(|_| {
            // Last resort: blocking read so we never advertise "" → "Hub N"
            // while the writer holds the lock for a name edit.
            params
                .name_bg
                .read()
                .map(|n| n.clone())
                .or_else(|_| params.name.read().map(|n| n.clone()))
                .unwrap_or_default()
        });
    // Keep name_bg in lockstep so the audio heartbeat thread (if any) publishes
    // the same label Relay's dropdown lists.
    if let Ok(mut bg) = params.name_bg.try_write()
        && bg.as_str() != raw.as_str()
    {
        *bg = raw.clone();
    }
    let my_name = display_name(&raw, slot as u8);
    if let Some(hub) = relay_hub() {
        hub.write_consumer_name(slot as u8, &my_name, now_ms);
    }
}

/// Immediate SHM consumer rename (Vizia Textbox `on_edit` parity). Call from
/// the editor name callback so Relay target lists update without waiting for
/// the next 33 ms tick / audio block.
pub(crate) fn editor_publish_consumer_name(shared: &LucentShared, raw: &str) {
    let now_ms = now_ms();
    let mut slot = shared.shm.slot.load(Ordering::Acquire);
    if slot < 0
        && let Some(hub) = relay_hub()
        && let Some(claimed) = hub.claim_consumer_slot(now_ms)
    {
        slot = claimed as i32;
        shared.shm.slot.store(slot, Ordering::Release);
    }
    if slot < 0 {
        return;
    }
    let my_name = display_name(raw, slot as u8);
    if let Some(hub) = relay_hub() {
        hub.write_consumer_name(slot as u8, &my_name, now_ms);
    }
}

// ─── RT peak types (Copy, no String on the audio thread) ─────────────────────

/// Empty contributor slot.
pub const CONTRIB_NONE: u8 = u8::MAX;
/// Own bus (masking collisions only).
pub const CONTRIB_OWN: u8 = u8::MAX - 1;
// SHM publisher slots use raw `0..MAX_SLOTS` (fits: MAX_SLOTS=16).

/// Max contributor ids per attributed peak (masking pair or group resonance).
pub const MAX_PEAK_CONTRIBS: usize = 8;
/// Intermediate local-maxima cap in PeakTracker (before harmonic suppress).
pub const MAX_RAW_PEAKS: usize = 64;
/// Published resonance peaks (own or group) — matches prior truncate(16).
pub const MAX_RES_PEAKS: usize = 16;
/// Published masking local-maxima for SNAP / UI hold.
pub const MAX_MASK_PEAKS: usize = 64;

/// Peak with SHM-slot / Own contributor ids — resolved to names only in the editor.
#[derive(Clone, Copy, Debug)]
pub struct AttributedPeak {
    pub bin: usize,
    pub score: f32,
    pub ids: [u8; MAX_PEAK_CONTRIBS],
    pub n_ids: u8,
}

impl Default for AttributedPeak {
    fn default() -> Self {
        Self {
            bin: 0,
            score: 0.0,
            ids: [CONTRIB_NONE; MAX_PEAK_CONTRIBS],
            n_ids: 0,
        }
    }
}

impl AttributedPeak {
    pub fn new(bin: usize, score: f32) -> Self {
        Self {
            bin,
            score,
            ids: [CONTRIB_NONE; MAX_PEAK_CONTRIBS],
            n_ids: 0,
        }
    }

    pub fn push_id(&mut self, id: u8) {
        if id == CONTRIB_NONE {
            return;
        }
        let n = self.n_ids as usize;
        if n < MAX_PEAK_CONTRIBS {
            self.ids[n] = id;
            self.n_ids += 1;
        }
    }

    pub fn ids_slice(&self) -> &[u8] {
        &self.ids[..self.n_ids as usize]
    }
}

/// Resonance findings for one Lucent instance: `own` = peaks found in this
/// instance's own bus signal, `relay` = peaks found in the power-summed
/// spectrum of the Relay tracks it's listening to (group-level resonance
/// that can emerge from the sum even if no single track shows it).
#[derive(Default, Clone)]
pub struct ResonanceLists {
    pub own: Vec<(usize, f32)>,
    /// Group peaks with SHM-slot contributor ids (names resolved in the editor).
    pub relay: Vec<AttributedPeak>,
}

/// Magnitude floor (dB) above which a Relay track counts as a contributor to
/// a group resonance peak. Same value as `MaskingAnalyzer`'s `FLOOR`, kept as
/// its own constant here since the two aren't the same computation.
const CONTRIB_FLOOR_DB: f32 = -70.0;

/// For each (bin, score) group-level peak, record which Relay **SHM slots**
/// are above the floor at that bin. `mask` filters via [`relay_slot_active`].
/// Writes into fixed `out` / `out_n` — no heap, no strings.
pub(crate) fn attribute_contributors_into(
    peaks: &[(usize, f32)],
    relay_spectra: &[RelayFeed],
    mask: u32,
    out: &mut [AttributedPeak; MAX_RES_PEAKS],
    out_n: &mut usize,
) {
    let n = peaks.len().min(MAX_RES_PEAKS);
    for (i, &(bin, score)) in peaks.iter().take(n).enumerate() {
        let mut p = AttributedPeak::new(bin, score);
        for (slot, _, spec) in relay_spectra.iter() {
            if !relay_slot_active(mask, *slot) {
                continue;
            }
            if bin < spec.len() && spec[bin] > CONTRIB_FLOOR_DB {
                p.push_id(*slot);
            }
        }
        out[i] = p;
    }
    *out_n = n;
}

/// Copy score peaks into a reused registry buffer (no realloc after warmup).
fn sync_score_peaks(dest: &mut Vec<(usize, f32)>, src: &[(usize, f32)]) {
    while dest.len() < src.len() {
        dest.push((0, 0.0));
    }
    for (i, p) in src.iter().enumerate() {
        dest[i] = *p;
    }
    dest.truncate(src.len());
}

/// Copy attributed peaks (Copy) into a reused registry buffer.
fn sync_attributed_peaks(dest: &mut Vec<AttributedPeak>, src: &[AttributedPeak]) {
    while dest.len() < src.len() {
        dest.push(AttributedPeak::default());
    }
    for (i, p) in src.iter().enumerate() {
        dest[i] = *p;
    }
    dest.truncate(src.len());
}

/// Keyed by `Arc::as_ptr(&params)` — unique per plugin instance. A bare
/// `OnceLock<Vec<_>>` here would mean every Lucent instance in the process
/// overwrites the same global list (same failure mode as the Lucent-Relay
/// `RELAY_HANDLE` singleton bug).
type ResonanceRegistry = Arc<Mutex<HashMap<usize, ResonanceLists>>>;

fn resonance_registry() -> &'static ResonanceRegistry {
    static REG: OnceLock<ResonanceRegistry> = OnceLock::new();
    REG.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

/// In-place publish (like [`publish_relays`]): reuses registry capacity so the
/// FFT hop stays alloc-free after the first findings stabilize.
pub fn publish_resonance(key: usize, own: &[(usize, f32)], relay: &[AttributedPeak]) {
    // try_lock: audio thread must never block on the editor reader.
    if let Ok(mut m) = resonance_registry().try_lock() {
        let lists = m.entry(key).or_default();
        sync_score_peaks(&mut lists.own, own);
        sync_attributed_peaks(&mut lists.relay, relay);
    }
}

pub fn read_resonance(key: usize) -> ResonanceLists {
    resonance_registry()
        .try_lock()
        .ok()
        .and_then(|m| m.get(&key).cloned())
        .unwrap_or_default()
}

pub fn remove_resonance(key: usize) {
    if let Ok(mut m) = resonance_registry().try_lock() {
        m.remove(&key);
    }
}

/// Same instance-keyed pattern as `ResonanceRegistry`, for the top masking
/// collisions (bin, dB, contributor ids) of each Lucent instance.
type MaskingRegistry = Arc<Mutex<HashMap<usize, Vec<AttributedPeak>>>>;

fn masking_registry() -> &'static MaskingRegistry {
    static REG: OnceLock<MaskingRegistry> = OnceLock::new();
    REG.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

/// In-place publish — reuses peak storage across FFT hops (Copy ids, no strings).
pub fn publish_masking(key: usize, peaks: &[AttributedPeak]) {
    if let Ok(mut m) = masking_registry().try_lock() {
        let slot = m.entry(key).or_default();
        sync_attributed_peaks(slot, peaks);
    }
}

pub fn read_masking(key: usize) -> Vec<AttributedPeak> {
    masking_registry()
        .try_lock()
        .ok()
        .and_then(|m| m.get(&key).cloned())
        .unwrap_or_default()
}

pub fn remove_masking(key: usize) {
    if let Ok(mut m) = masking_registry().try_lock() {
        m.remove(&key);
    }
}

/// One relay feed snapshot for the editor: SHM publisher slot, display name,
/// fixed-size dB bins (no `Vec` on the publish path).
#[derive(Clone, Debug)]
pub struct RelaySlot {
    pub slot: u8,
    pub name: String,
    pub bins: [f32; SPECTRUM_BINS],
}

/// Max relay feeds shown in the UI (matches the old relay bar cap).
pub(crate) const MAX_RELAY_SLOTS: usize = 8;

/// Same instance-keyed pattern as `ResonanceRegistry`, for the live relay
/// spectra the editor draws as overlay curves + toggle bar.
type RelayRegistry = Arc<Mutex<HashMap<usize, Vec<RelaySlot>>>>;

fn relay_registry() -> &'static RelayRegistry {
    static REG: OnceLock<RelayRegistry> = OnceLock::new();
    REG.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

/// Publish the relays read this FFT hop (before the active-mask filter — the
/// UI shows/toggles all discovered relays). In-place update keeps String
/// storage hot across hops: no steady-state alloc on the audio thread.
pub fn publish_relays(key: usize, feeds: &[RelayFeed]) {
    if let Ok(mut m) = relay_registry().try_lock() {
        let slots = m.entry(key).or_default();
        let n = feeds.len().min(MAX_RELAY_SLOTS);
        for (i, (slot, name, bins)) in feeds.iter().take(n).enumerate() {
            if let Some(s) = slots.get_mut(i) {
                s.slot = *slot;
                s.name.clear();
                s.name.push_str(name);
                s.bins = *bins;
            } else {
                slots.push(RelaySlot {
                    slot: *slot,
                    name: name.clone(),
                    bins: *bins,
                });
            }
        }
        slots.truncate(n);
    }
}

pub fn read_relays(key: usize) -> Vec<RelaySlot> {
    relay_registry()
        .try_lock()
        .ok()
        .and_then(|m| m.get(&key).cloned())
        .unwrap_or_default()
}

pub fn remove_relays(key: usize) {
    if let Ok(mut m) = relay_registry().try_lock() {
        m.remove(&key);
    }
}

/// Per-bin power-sum (linear domain) of named dB spectra, into `out` (no alloc).
/// Models how tracks combine on a bus — e.g. two -6dB at same bin → ~-3dB.
/// `mask` filters relay slots via [`relay_slot_active`].
pub(crate) fn power_sum_named_into(relay_named: &[RelayFeed], mask: u32, out: &mut [f32]) {
    let n = out.len().min(SPECTRUM_BINS);
    out[..n].fill(-90.0);
    for (j, o) in out.iter_mut().enumerate().take(n) {
        let sum_lin: f32 = relay_named
            .iter()
            .filter(|(slot, _, _)| relay_slot_active(mask, *slot))
            .map(|(_, _, s)| 10f32.powf(s[j] / 10.0))
            .sum();
        *o = if sum_lin < 1e-9 {
            -90.0
        } else {
            10.0 * sum_lin.log10()
        };
    }
}

/// Drops peaks that are almost certainly a musical overtone of a louder,
/// lower peak rather than an independent resonance — normal harmonic
/// spectral structure, not a problem. FFT bins are linear in Hz, so the
/// nth harmonic of a peak at bin `k0` falls near bin `n * k0` exactly; no
/// pitch/fundamental tracking needed (which would be unreliable on a full
/// mix bus anyway). Only suppresses when the candidate harmonic isn't
/// louder than the fundamental by more than a few dB — a peak riding well
/// above where a harmonic should sit is kept, since that's more likely a
/// real resonance than natural overtone rolloff.
///
/// In-place on a fixed peak buffer: marks harmonics, then compacts (no heap).
fn suppress_harmonics_in_place(spectrum: &[f32], peaks: &mut [(usize, f32)], n: &mut usize) {
    const MAX_HARMONIC: usize = 8;
    const BIN_TOLERANCE: usize = 2;
    const LOUDER_MARGIN_DB: f32 = 3.0;

    let len = *n;
    for i in 0..len {
        let k = peaks[i].0;
        if k >= spectrum.len() {
            peaks[i].0 = usize::MAX;
            continue;
        }
        let is_harmonic = peaks[..len].iter().any(|&(k0, _)| {
            k0 != usize::MAX
                && k0 < k
                && spectrum[k] <= spectrum[k0] + LOUDER_MARGIN_DB
                && (2..=MAX_HARMONIC).any(|h| (k0 * h).abs_diff(k) <= BIN_TOLERANCE)
        });
        if is_harmonic {
            peaks[i].0 = usize::MAX;
        }
    }
    let mut w = 0;
    for i in 0..len {
        if peaks[i].0 != usize::MAX {
            peaks[w] = peaks[i];
            w += 1;
        }
    }
    *n = w;
}

mod editor;
mod process;
mod relay_state;

// Lucent compact shell (no footer / no OUT GAIN): 940 × 500
#[allow(dead_code)]
const WINDOW_W: u32 = 940;
#[allow(dead_code)]
const WINDOW_H: u32 = 500;

// ─── Sensitivity ────────────────────────────────────────────────

/// Derived from the `Sensitivity` knob (0.0 = strict/conservative, 1.0 =
/// sensitive). All six numbers below were the hand-tuned constants this
/// analyzer shipped with; they're now the sensitivity=0.5 midpoint of each
/// range so the knob's center detent reproduces the previous (already-tuned)
/// behavior exactly, and moving it scales — all at once — how loud, how
/// tonal, and how long a peak must be before it counts as a resonance or
/// masking collision. One knob, not two: Lucent only displays, it never
/// suggests or applies a cut, so there's no separate "how strong an action"
/// axis to control.
struct SensitivityThresholds {
    contrast_min_db: f32,
    flatness_max: f32,
    floor_db: f32,
    score_min: f32,
    persistence_min: u32,
    masking_floor_db: f32,
    /// Minimum Q (center freq / -3dB bandwidth) for a peak to count as
    /// narrowband. Rejects broad humps (formants, EQ buckets, room-mode
    /// clusters) that pass contrast+flatness but aren't a sharp resonance.
    min_q: f32,
}

pub(crate) fn sensitivity_thresholds(sensitivity: f32) -> SensitivityThresholds {
    let d = sensitivity.clamp(0.0, 1.0);
    let lerp = |a: f32, b: f32| a + (b - a) * d;
    SensitivityThresholds {
        contrast_min_db: lerp(8.0, 3.0),
        flatness_max: lerp(0.5, 0.85),
        floor_db: lerp(-65.0, -85.0),
        score_min: lerp(4.0, 1.0),
        persistence_min: lerp(20.0, 4.0) as u32,
        masking_floor_db: lerp(-55.0, -85.0),
        min_q: lerp(6.0, 2.0),
    }
}

// ─── Masking analyzer ────────────────────────────────────────────────────────

/// True when a track has enough physical energy to participate in masking.
fn track_has_masking_signal(spectrum: &[f32], sample_rate: f32) -> bool {
    let n = spectrum.len();
    if n == 0 {
        return false;
    }
    let bin_hz = sample_rate / (n as f32 * 2.0);
    spectrum.iter().enumerate().any(|(j, &db)| {
        let freq = j as f32 * bin_hz;
        spectrum_physical_db(db, freq) > SPECTRUM_TILT_RAW_GATE_DB
    })
}

struct MaskingAnalyzer {
    /// Persistence-gated collision level per bin — what everything outside
    /// this struct reads (FFT overlay bars, `peaks_above_floor`).
    masking_map: Vec<f32>,
    /// This frame's ERB-smoothed collision level, before the persistence
    /// gate. Kept separate so a single loud-but-brief collision doesn't
    /// immediately count as "masking" (see `persistence`).
    raw: Vec<f32>,
    persistence: Vec<u32>,
    scratch: Vec<f32>,
    /// Contributor ids for `scratch[j]` / `masking_map[j]`: `0` = Own,
    /// `1..=n` = index into `relay_names` + 1, `u8::MAX` = no collision.
    /// Ids instead of per-bin `Vec<String>` keep the FFT-hop path alloc-free;
    /// names are only resolved for the few reported peaks in
    /// `peaks_above_floor`.
    scratch_ids: Vec<(u8, u8)>,
    masking_ids: Vec<(u8, u8)>,
    /// Per-relay "has signal" flags for the current frame (reused buffer).
    relay_live: Vec<bool>,
    /// Fixed output of [`Self::fill_peaks_above_floor`] (ids only).
    peaks_scratch: [AttributedPeak; MAX_MASK_PEAKS],
    peaks_n: usize,
}

impl MaskingAnalyzer {
    fn new(_sample_rate: f32) -> Self {
        Self {
            masking_map: vec![-90.0; SPECTRUM_BINS],
            raw: vec![-90.0; SPECTRUM_BINS],
            persistence: vec![0u32; SPECTRUM_BINS],
            scratch: vec![-90.0; SPECTRUM_BINS],
            scratch_ids: vec![(CONTRIB_NONE, CONTRIB_NONE); SPECTRUM_BINS],
            masking_ids: vec![(CONTRIB_NONE, CONTRIB_NONE); SPECTRUM_BINS],
            relay_live: Vec::with_capacity(MAX_RELAY_SLOTS),
            peaks_scratch: [AttributedPeak::default(); MAX_MASK_PEAKS],
            peaks_n: 0,
        }
    }

    /// `relay_named` pairs each Relay spectrum with its track name so a
    /// masking collision can be attributed to the two tracks that caused it.
    /// `mask` filters via [`relay_slot_active`]. `persistence_min` is the
    /// Sensitivity knob's shared persistence gate
    /// (same field resonance uses) — a collision only counts once it holds
    /// for that many frames, not on a single-frame blip.
    fn compute_masking(
        &mut self,
        own_spectrum: Option<&[f32]>,
        relay_named: &[RelayFeed],
        mask: u32,
        floor_db: f32,
        sample_rate: f32,
        persistence_min: u32,
    ) {
        let n = self.masking_map.len();
        let bin_hz = sample_rate / (n as f32 * 2.0);
        let own_live = own_spectrum
            .map(|s| track_has_masking_signal(s, sample_rate))
            .unwrap_or(false);

        // Live flags only — contributor identity is SHM slot / CONTRIB_OWN.
        self.relay_live.clear();
        for (slot, _, s) in relay_named.iter() {
            self.relay_live
                .push(relay_slot_active(mask, *slot) && track_has_masking_signal(s, sample_rate));
        }

        for j in 0..n {
            let freq = j as f32 * bin_hz;
            // (level, contrib id) — CONTRIB_OWN or raw SHM publisher slot.
            let mut active: [(f32, u8); 17] = [(-90.0f32, CONTRIB_NONE); 17];
            let mut count = 0usize;

            if let Some(own_spec) = own_spectrum {
                let own = spectrum_physical_db(own_spec.get(j).copied().unwrap_or(-90.0), freq);
                if own_live && own > floor_db {
                    active[count] = (own, CONTRIB_OWN);
                    count += 1;
                }
            }
            for ((slot, _, relay), live) in relay_named.iter().zip(self.relay_live.iter()) {
                if j < relay.len() {
                    let phys = spectrum_physical_db(relay[j], freq);
                    if *live && phys > floor_db && count < active.len() {
                        active[count] = (phys, *slot);
                        count += 1;
                    }
                }
            }

            let mut best = -90.0f32;
            let mut best_pair = (CONTRIB_NONE, CONTRIB_NONE);
            for a in 0..count {
                for b in (a + 1)..count {
                    let collision = active[a].0.min(active[b].0);
                    if collision > best {
                        best = collision;
                        best_pair = (active[a].1, active[b].1);
                    }
                }
            }
            self.scratch[j] = best;
            self.scratch_ids[j] = best_pair;
        }

        // Smooth over the ERB (critical-band) width around each bin instead
        // of a fixed ±2 bins: FFT bins are linear in Hz but the ear's
        // critical bandwidth grows with frequency (~35Hz at 100Hz, ~1100Hz
        // at 10kHz per Glasberg & Moore), so a fixed bin window is roughly
        // right at low frequencies but far too narrow at high ones — it was
        // comparing frequencies as if they were in separate perceptual
        // bands when the ear would blend them together.
        for j in 0..n {
            let freq = j as f32 * bin_hz;
            let erb_hz = 24.7 * (4.37 * freq / 1000.0 + 1.0);
            let half_window = ((erb_hz / 2.0 / bin_hz).round() as usize).clamp(2, 40);
            let lo = j.saturating_sub(half_window);
            let hi = (j + half_window).min(n - 1);
            let mut m = -90.0f32;
            let mut m_idx = j;
            for k in lo..=hi {
                if self.scratch[k] > m {
                    m = self.scratch[k];
                    m_idx = k;
                }
            }
            self.raw[j] = m;
            self.masking_ids[j] = self.scratch_ids[m_idx];
        }

        const PERSIST_CAP: u32 = 40;
        for j in 0..n {
            if self.raw[j] > floor_db {
                self.persistence[j] = (self.persistence[j] + 1).min(PERSIST_CAP);
            } else {
                self.persistence[j] = self.persistence[j].saturating_sub(1);
            }
            self.masking_map[j] = if self.persistence[j] > persistence_min {
                self.raw[j]
            } else {
                -90.0
            };
        }
    }

    /// Local-maxima of the masking map above `floor_db`. Sorted by severity
    /// descending into fixed [`Self::peaks_scratch`] (cap [`MAX_MASK_PEAKS`]).
    /// Contributor ids only — editor resolves names via SHM slots.
    fn fill_peaks_above_floor(&mut self, floor_db: f32) {
        let n = self.masking_map.len();
        let mut count = 0usize;
        for i in 0..n {
            let db = self.masking_map[i];
            if db <= floor_db {
                continue;
            }
            let left = if i == 0 {
                f32::NEG_INFINITY
            } else {
                self.masking_map[i - 1]
            };
            let right = if i + 1 >= n {
                f32::NEG_INFINITY
            } else {
                self.masking_map[i + 1]
            };
            if db < left || db < right {
                continue;
            }
            let (a, b) = self.masking_ids[i];
            let mut p = AttributedPeak::new(i, db);
            p.push_id(a);
            p.push_id(b);
            if count < MAX_MASK_PEAKS {
                self.peaks_scratch[count] = p;
                count += 1;
            } else {
                // Keep strongest peaks when over cap.
                let mut min_i = 0;
                let mut min_s = self.peaks_scratch[0].score;
                for (k, pk) in self.peaks_scratch.iter().enumerate().take(MAX_MASK_PEAKS) {
                    if pk.score < min_s {
                        min_s = pk.score;
                        min_i = k;
                    }
                }
                if db > min_s {
                    self.peaks_scratch[min_i] = p;
                }
            }
        }
        self.peaks_n = count.min(MAX_MASK_PEAKS);
        self.peaks_scratch[..self.peaks_n].sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    fn peaks_above_floor(&self) -> &[AttributedPeak] {
        &self.peaks_scratch[..self.peaks_n]
    }
}

// ─── Peak tracker ─────────────────────────────────────────────────────────────

struct PeakTracker {
    persistence: Vec<u32>,
    last_prominence: Vec<f32>,
    resonance_score: Vec<f32>,
    /// Fixed raw peak list for the current hop (after harmonic suppress).
    peaks_scratch: [(usize, f32); MAX_RAW_PEAKS],
    peaks_n: usize,
    /// Bin flags for O(1) peak membership in [`Self::update_from_scratch`].
    peak_flag: [bool; SPECTRUM_BINS],
    /// Fixed resonance report (cap [`MAX_RES_PEAKS`]).
    res_scratch: [(usize, f32); MAX_RES_PEAKS],
    res_n: usize,
}

impl PeakTracker {
    fn new() -> Self {
        Self {
            persistence: vec![0u32; SPECTRUM_BINS],
            last_prominence: vec![0.0; SPECTRUM_BINS],
            resonance_score: vec![0.0; SPECTRUM_BINS],
            peaks_scratch: [(0, 0.0); MAX_RAW_PEAKS],
            peaks_n: 0,
            peak_flag: [false; SPECTRUM_BINS],
            res_scratch: [(0, 0.0); MAX_RES_PEAKS],
            res_n: 0,
        }
    }

    /// Find peaks → suppress harmonics → update persistence → fill resonance.
    /// Fixed buffers only — no heap on the hop path.
    fn detect(&mut self, spectrum: &[f32], t: &SensitivityThresholds, sample_rate: f32) {
        self.find_peaks_into(spectrum, t, sample_rate);
        suppress_harmonics_in_place(spectrum, &mut self.peaks_scratch, &mut self.peaks_n);
        self.update_from_scratch();
        self.fill_resonance(t);
    }

    fn res_peaks(&self) -> &[(usize, f32)] {
        &self.res_scratch[..self.res_n]
    }

    /// Local maximum + four gates, each rejecting a distinct false-positive
    /// mode the raw 2-neighbor prominence check let through:
    /// - floor: rejects peaks sitting in the noise floor (no real signal there)
    /// - contrast: prominence against a wide local baseline (±8 bins) instead
    ///   of just the immediate 2 neighbors, less sensitive to single-bin ripple
    /// - flatness: rejects broadband/noisy content (cymbals, hats) that has
    ///   lots of small local maxima but isn't a narrowband resonance — the
    ///   main fix for the high-frequency false-positive bias, since bright
    ///   material triggers many raw local maxima that a flat dB threshold
    ///   alone can't tell apart from an actual tonal peak.
    /// - Q (bandwidth): rejects broad humps (formants, EQ buckets, room-mode
    ///   clusters) — contrast+flatness alone can't tell a wide bump from a
    ///   sharp resonance, only the -3dB bandwidth can.
    fn find_peaks_into(&mut self, spectrum: &[f32], t: &SensitivityThresholds, sample_rate: f32) {
        const BASELINE_WINDOW: usize = 8;
        const FLATNESS_WINDOW: usize = 4;
        const MAX_BW_SEARCH: usize = 24;

        let n = spectrum.len();
        let bin_hz = sample_rate / (n as f32 * 2.0);
        self.peaks_n = 0;
        for k in 1..n.saturating_sub(1) {
            let left = spectrum[k - 1];
            let center = spectrum[k];
            let right = spectrum[k + 1];
            if !(center > left && center > right) {
                continue;
            }
            if center < t.floor_db {
                continue;
            }

            let lo = k.saturating_sub(BASELINE_WINDOW);
            let hi = (k + BASELINE_WINDOW).min(n - 1);
            let baseline = spectrum[lo..=hi].iter().sum::<f32>() / (hi - lo + 1) as f32;
            let contrast = center - baseline;
            if contrast < t.contrast_min_db {
                continue;
            }

            let flo = k.saturating_sub(FLATNESS_WINDOW);
            let fhi = (k + FLATNESS_WINDOW).min(n - 1);
            let window = &spectrum[flo..=fhi];
            let power_sum: f32 = window.iter().map(|&db| 10f32.powf(db / 10.0)).sum();
            let log_sum: f32 = window
                .iter()
                .map(|&db| 10f32.powf(db / 10.0).max(1e-12).ln())
                .sum();
            let count = window.len() as f32;
            let arith_mean = power_sum / count;
            let geo_mean = (log_sum / count).exp();
            let flatness = if arith_mean > 1e-12 {
                geo_mean / arith_mean
            } else {
                1.0
            };
            if flatness > t.flatness_max {
                continue;
            }

            let bw_lo_bound = k.saturating_sub(MAX_BW_SEARCH);
            let mut lo_edge = k;
            while lo_edge > bw_lo_bound && spectrum[lo_edge - 1] > center - 3.0 {
                lo_edge -= 1;
            }
            let bw_hi_bound = (k + MAX_BW_SEARCH).min(n - 1);
            let mut hi_edge = k;
            while hi_edge < bw_hi_bound && spectrum[hi_edge + 1] > center - 3.0 {
                hi_edge += 1;
            }
            let bandwidth_hz = (hi_edge - lo_edge).max(1) as f32 * bin_hz;
            let q = (k as f32 * bin_hz) / bandwidth_hz;
            if q < t.min_q {
                continue;
            }

            if self.peaks_n < MAX_RAW_PEAKS {
                self.peaks_scratch[self.peaks_n] = (k, contrast);
                self.peaks_n += 1;
            } else {
                // Prefer higher contrast when over cap.
                let mut min_i = 0;
                let mut min_c = self.peaks_scratch[0].1;
                for (i, p) in self.peaks_scratch.iter().enumerate() {
                    if p.1 < min_c {
                        min_c = p.1;
                        min_i = i;
                    }
                }
                if contrast > min_c {
                    self.peaks_scratch[min_i] = (k, contrast);
                }
            }
        }
    }

    fn update_from_scratch(&mut self) {
        self.peak_flag.fill(false);
        for &(k, prom) in self.peaks_scratch[..self.peaks_n].iter() {
            if k < SPECTRUM_BINS {
                self.peak_flag[k] = true;
                self.last_prominence[k] = prom;
            }
        }

        const PERSIST_CAP: u32 = 40;
        for k in 0..SPECTRUM_BINS {
            if self.peak_flag[k] {
                self.persistence[k] = (self.persistence[k] + 1).min(PERSIST_CAP);
            } else {
                self.persistence[k] = self.persistence[k].saturating_sub(1);
            }
            let target =
                self.last_prominence[k] * (self.persistence[k] as f32 / PERSIST_CAP as f32);
            let coef = if target > self.resonance_score[k] {
                0.6
            } else {
                0.04
            };
            self.resonance_score[k] =
                (self.resonance_score[k] * (1.0 - coef) + target * coef).max(0.0);
        }
    }

    fn fill_resonance(&mut self, t: &SensitivityThresholds) {
        // Reuse peaks_scratch as temp full candidate list, then keep top N.
        self.peaks_n = 0;
        for k in 1..SPECTRUM_BINS.saturating_sub(1) {
            if self.resonance_score[k] > t.score_min && self.persistence[k] > t.persistence_min {
                let score = self.resonance_score[k];
                if self.peaks_n < MAX_RAW_PEAKS {
                    self.peaks_scratch[self.peaks_n] = (k, score);
                    self.peaks_n += 1;
                } else {
                    let mut min_i = 0;
                    let mut min_s = self.peaks_scratch[0].1;
                    for (i, p) in self.peaks_scratch.iter().enumerate() {
                        if p.1 < min_s {
                            min_s = p.1;
                            min_i = i;
                        }
                    }
                    if score > min_s {
                        self.peaks_scratch[min_i] = (k, score);
                    }
                }
            }
        }
        self.peaks_scratch[..self.peaks_n]
            .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        self.res_n = self.peaks_n.min(MAX_RES_PEAKS);
        self.res_scratch[..self.res_n].copy_from_slice(&self.peaks_scratch[..self.res_n]);
    }
}

// ─── Params ──────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct LucentParams {
    #[param(
        id = 1,
        name = "Analyze Mode",
        default = 0,
        range = "discrete(0, 2)",
        group = "Lucent"
    )]
    pub analyze_mode: IntParam,
    #[param(id = 2, name = "Resonance", default = 1, group = "Lucent")]
    pub resonance_active: BoolParam,
    #[param(id = 3, name = "Masking", default = 1, group = "Lucent")]
    pub masking_active: BoolParam,
    #[param(id = 4, name = "Bypass", default = 0, group = "Lucent")]
    pub bypass_active: BoolParam,
    /// How deep the resonance/masking detectors dig: 0% = shallow (only
    /// strong, sustained findings), 100% = deep (surfaces weaker, shorter
    /// ones too). 50% reproduces the previously hand-tuned thresholds.
    #[param(
        id = 5,
        name = "Sensitivity",
        default = 50.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Lucent"
    )]
    pub sensitivity: FloatParam,
    #[persist]
    pub name: RwLock<String>,
    /// Live name for the background SHM heartbeat thread — shared with the
    /// editor via AURA's `Arc<LucentParams>` so renames apply when
    /// transport is stopped.
    #[skip]
    pub name_bg: Arc<RwLock<String>>,
    #[skip]
    pub shared: Arc<LucentShared>,
}

impl LucentParams {
    /// Real value display for `unit = "%"` params: our plain values are
    /// already the percent number (e.g. `50.0` means `50%`), not a
    /// 0.0-1.0 fraction. `aura_params::format_param_value`'s built-in
    /// Percent case multiplies by 100 assuming the latter, so it would
    /// show `5000%` for a real 50% value without this override.
    fn fmt_pct(&self, value: f64) -> String {
        format!("{value:.1}%")
    }
}

// ─── Plugin ───────────────────────────────────────────────────────────────────

pub struct Lucent;

pub struct LucentDspState {
    pub(crate) fft_fwd: Arc<dyn RealToComplex<f32>>,
    pub(crate) fft_input: Vec<f32>,
    pub(crate) fft_write_pos: usize,
    pub(crate) fft_hann: Vec<f32>,
    pub(crate) fft_windowed: Vec<f32>,
    pub(crate) fft_output: Vec<Complex<f32>>,
    pub(crate) peak_tracker: PeakTracker,
    pub(crate) relay_peak_tracker: PeakTracker,
    pub(crate) masking_analyzer: MaskingAnalyzer,
    /// Scratch for group-level power-sum (avoids per-FFT heap alloc on RT).
    pub(crate) relay_sum_buf: Vec<f32>,
    /// Pre-filled `MAX_SLOTS` feeds for `read_active_into` — live count in
    /// `relay_scratch_n`. Never `clear()`ed (would drop String capacity).
    pub(crate) relay_scratch: Vec<RelayFeed>,
    pub(crate) relay_scratch_n: usize,
    /// Fixed group-resonance attribution (SHM slot ids, no strings).
    pub(crate) contrib_scratch: [AttributedPeak; MAX_RES_PEAKS],
    pub(crate) contrib_n: usize,
    pub(crate) sample_rate: f32,
    pub(crate) peak_hold_value: f32,
    pub(crate) peak_hold_l_value: f32,
    pub(crate) peak_hold_r_value: f32,
    pub(crate) claimed_lucent_slot: Option<u8>,
    /// Slot last baked into `cached_display_name` (recompute on claim/change).
    pub(crate) display_name_slot: Option<u8>,
    pub(crate) cached_name: String,
    pub(crate) cached_display_name: String,
    pub(crate) liveness: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub(crate) instance_key: usize,
    /// Envelope follower driving the goniometer's visual auto-gain — same
    /// pattern as Equilibrium/Meridian, so all three plugins' vectorscopes
    /// fill the same visual range regardless of the signal's actual level
    /// instead of Lucent's showing a tiny raw-amplitude dot cluster.
    pub(crate) scope_vis_envelope: f32,
}

impl LucentDspState {
    pub(crate) fn build_fft() -> (Arc<dyn RealToComplex<f32>>, Vec<Complex<f32>>) {
        let fft_size = SPECTRUM_BINS * 2;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft_fwd = planner.plan_fft_forward(fft_size);
        let fft_output = fft_fwd.make_output_vec();
        (fft_fwd, fft_output)
    }

    pub(crate) fn ensure_consumer_slot(&mut self, params: &LucentParams, now_ms: u64) {
        if self.claimed_lucent_slot.is_some() {
            return;
        }
        let adopted = params.shared.shm.slot.load(Ordering::Acquire);
        if adopted >= 0 {
            self.claimed_lucent_slot = Some(adopted as u8);
        } else if let Some(hub) = relay_hub() {
            self.claimed_lucent_slot = hub.claim_consumer_slot(now_ms);
        }
        params.shared.shm.slot.store(
            self.claimed_lucent_slot.map(|s| s as i32).unwrap_or(-1),
            Ordering::Release,
        );
    }

    pub(crate) fn publish_consumer_name(&mut self, params: &LucentParams, now_ms: u64) {
        let mut name_changed = false;
        if let Ok(name) = params.name.try_read()
            && *name != self.cached_name
        {
            self.cached_name.clear();
            self.cached_name.push_str(&name);
            if let Ok(mut bg) = params.name_bg.try_write() {
                bg.clear();
                bg.push_str(&name);
            }
            name_changed = true;
        }
        let slot_changed = self.display_name_slot != self.claimed_lucent_slot;
        if name_changed || slot_changed {
            self.display_name_slot = self.claimed_lucent_slot;
            self.cached_display_name = self
                .claimed_lucent_slot
                .map(|slot| display_name(&self.cached_name, slot))
                .unwrap_or_else(|| self.cached_name.clone());
        }
        if let Some(slot) = self.claimed_lucent_slot
            && let Some(hub) = relay_hub()
        {
            hub.write_consumer_name(slot, &self.cached_display_name, now_ms);
        }
    }

    pub(crate) fn spawn_consumer_heartbeat(&mut self, params: &LucentParams) {
        if let Some(alive) = self.liveness.take() {
            alive.store(false, Ordering::Release);
        }
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        self.liveness = Some(alive.clone());
        let shared = params.shared.clone();
        let name_bg = params.name_bg.clone();
        std::thread::spawn(move || {
            while alive.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let slot = shared.shm.slot.load(Ordering::Acquire);
                if slot < 0 {
                    continue;
                }
                if let Some(hub) = relay_hub() {
                    let raw = name_bg.read().ok().map(|n| n.clone()).unwrap_or_default();
                    let name = display_name(&raw, slot as u8);
                    hub.write_consumer_name(slot as u8, &name, now_ms());
                }
            }
        });
    }
}

impl Default for LucentDspState {
    fn default() -> Self {
        let fft_size = SPECTRUM_BINS * 2;
        let (fft_fwd, fft_output) = Self::build_fft();
        Self {
            fft_fwd,
            fft_input: vec![0.0; fft_size],
            fft_write_pos: 0,
            fft_hann: (0..fft_size)
                .map(|i| {
                    let n = fft_size;
                    0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos())
                })
                .collect(),
            fft_windowed: vec![0.0; fft_size],
            fft_output,
            peak_tracker: PeakTracker::new(),
            relay_peak_tracker: PeakTracker::new(),
            masking_analyzer: MaskingAnalyzer::new(44100.0),
            relay_sum_buf: vec![-90.0; SPECTRUM_BINS],
            relay_scratch: (0..MAX_SLOTS)
                .map(|_| {
                    (
                        0u8,
                        String::with_capacity(MAX_NAME_LEN),
                        [-90.0f32; SPECTRUM_BINS],
                    )
                })
                .collect(),
            relay_scratch_n: 0,
            contrib_scratch: [AttributedPeak::default(); MAX_RES_PEAKS],
            contrib_n: 0,
            sample_rate: 44100.0,
            peak_hold_value: -100.0,
            peak_hold_l_value: -100.0,
            peak_hold_r_value: -100.0,
            claimed_lucent_slot: None,
            display_name_slot: None,
            cached_name: String::new(),
            cached_display_name: String::new(),
            liveness: None,
            instance_key: 0,
            scope_vis_envelope: 1e-4,
        }
    }
}

#[inline]
fn gain_to_db(amp: f32) -> f32 {
    if amp < 1e-10 {
        -200.0
    } else {
        20.0 * amp.log10()
    }
}

impl PluginLogic for Lucent {
    type Params = LucentParams;
    type DspState = LucentDspState;

    fn info() -> PluginInfo {
        let mut info = PluginInfo::new(
            "Lucent",
            "LX Audiolabs",
            env!("CARGO_PKG_VERSION"),
            "lucent",
        );
        // Stable ship IDs — must match pre-AURA truce Lucent (hosts key
        // sessions on clap id; com.lx-audiolabs.* breaks existing projects).
        info.clap_id = "be.lxndr.lucent";
        info.vst3_id = "be.lxndr.lucent";
        info.lv2_uri = "https://lx-audiolabs.com/lv2/lucent";
        info.category = PluginCategory::Analyzer;
        info
    }

    fn bus_layouts() -> Vec<BusLayout> {
        vec![BusLayout::stereo()]
    }

    fn init(params: &Self::Params, sample_rate: f64) -> Self::DspState {
        let mut state = LucentDspState::default();
        state.instance_key = params as *const _ as usize;
        // reset() covers the truce init's custom work (consumer slot claim,
        // name publish, heartbeat spawn) and also seeds the sample rate.
        Self::reset(&mut state, params, &AudioConfig::new(sample_rate, 4096));
        state
    }

    fn reset(state: &mut LucentDspState, params: &LucentParams, config: &AudioConfig) {
        let sr = config.sample_rate;
        state.sample_rate = sr as f32;
        let now_ms = now_ms();
        params
            .shared
            .spectrum
            .sample_rate
            .store(sr as f32, Ordering::Release);

        state.ensure_consumer_slot(params, now_ms);
        state.publish_consumer_name(params, now_ms);
        state.spawn_consumer_heartbeat(params);
    }

    fn process(
        state: &mut LucentDspState,
        params: &LucentParams,
        buffer: &mut AudioBuffer<'_, f32>,
        _ctx: &mut ProcessContext,
    ) -> ProcessStatus {
        process::run(state, params, buffer)
    }

    // No state_changed hook in AURA: the truce version re-synced the cached
    // name mirrors after preset/session load. That is already covered by the
    // lazy check in `publish_consumer_name` (compares `params.name` against
    // `state.cached_name` every audio block and re-syncs cached_name /
    // name_bg / cached_display_name on change), so nothing extra is needed.

    fn editor(params: Arc<Self::Params>) -> Option<Box<dyn Editor>> {
        Some(editor::build_editor(params))
    }
}

impl Drop for LucentDspState {
    fn drop(&mut self) {
        if let Some(alive) = self.liveness.take() {
            alive.store(false, Ordering::Release);
        }
        // Note: params.shared is not directly accessible here in 6.1.2 since
        // params lives outside DspState. The shell will clean up the shm_slot
        // via params when the plugin instance is torn down. We still clean up
        // resonance/masking registries by instance_key.
        if let Some(slot) = self.claimed_lucent_slot.take()
            && let Some(hub) = relay_hub()
        {
            hub.release_consumer_slot(slot);
        }
        remove_resonance(self.instance_key);
        remove_masking(self.instance_key);
        remove_relays(self.instance_key);
    }
}

#[cfg(feature = "clap")]
aura::export!(Lucent);

#[cfg(feature = "vst3")]
aura::export_vst3!(Lucent);

#[cfg(feature = "lv2")]
aura::export_lv2!(Lucent);

#[cfg(test)]
mod masking_tests {
    use super::MaskingAnalyzer;
    use aura_dsp::analysis::{SPECTRUM_BINS, spectrum_physical_db, spectrum_tilt_db};

    fn tilted_silent_spectrum(sample_rate: f32) -> Vec<f32> {
        let fft_size = (SPECTRUM_BINS * 2) as f32;
        (0..SPECTRUM_BINS)
            .map(|j| {
                let freq = j as f32 * sample_rate / fft_size;
                (-90.0 + spectrum_tilt_db(freq)).clamp(-90.0, 12.0)
            })
            .collect()
    }

    #[test]
    fn silent_tilted_relays_do_not_mask() {
        let sr = 48_000.0;
        let silent = tilted_silent_spectrum(sr);
        let mut silent_arr = [-90.0f32; SPECTRUM_BINS];
        silent_arr.copy_from_slice(&silent);
        let relays = [
            (0u8, "Relay A".to_string(), silent_arr),
            (1u8, "Relay B".to_string(), silent_arr),
        ];
        let mut analyzer = MaskingAnalyzer::new(sr);
        analyzer.compute_masking(None, &relays, 0, -70.0, sr, 4);
        analyzer.fill_peaks_above_floor(-70.0);
        assert!(
            analyzer.peaks_above_floor().is_empty(),
            "tilted silence must not register as masking"
        );
    }

    #[test]
    fn collision_reports_contributor_ids() {
        use super::CONTRIB_OWN;
        let sr = 48_000.0;
        let mut peak = [-90.0f32; SPECTRUM_BINS];
        peak[100] = 0.0;
        let relays = [(0u8, "Relay A".to_string(), peak)];
        let mut analyzer = MaskingAnalyzer::new(sr);
        analyzer.compute_masking(Some(&peak), &relays, 0, -70.0, sr, 0);
        analyzer.fill_peaks_above_floor(-70.0);
        let peaks = analyzer.peaks_above_floor();
        assert!(
            peaks.iter().any(|p| {
                let ids = p.ids_slice();
                ids.contains(&CONTRIB_OWN) && ids.contains(&0)
            }),
            "collision should id Own + SHM slot 0, got {peaks:?}"
        );
    }

    #[test]
    fn physical_db_undoes_tilted_silence() {
        let sr = 48_000.0;
        let fft_size = (SPECTRUM_BINS * 2) as f32;
        let freq = 983.0 * sr / fft_size;
        let displayed = -90.0 + spectrum_tilt_db(freq);
        let physical = spectrum_physical_db(displayed, freq);
        assert!(
            physical < -80.0,
            "physical level should sit at noise floor, got {physical}"
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::Lucent;

    #[test]
    fn renders_pass_through() {
        let frames = 2400; // ~50 ms at 48 kHz (old truce driver ran 50 ms)
        let inputs = vec![vec![0.5f32; frames], vec![0.5f32; frames]];
        let result = aura_test::process_with_input::<Lucent>(&inputs, frames);
        aura_test::assert_no_nans(&result);
        aura_test::assert_nonzero(&result);
    }

    #[test]
    fn state_round_trips() {
        aura_test::assert_state_round_trip::<Lucent>();
    }
}
