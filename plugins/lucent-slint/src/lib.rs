#![allow(unsafe_op_in_unsafe_fn)]

use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use truce::prelude::*;
use truce_core::{editor::Editor, state::StateLoadError};

use lx_analysis::{
    spectrum_physical_db, SPECTRUM_BINS, SharedState, SnapFFT, relay_hub, SPECTRUM_TILT_RAW_GATE_DB,
};

/// Claim a consumer slot (if needed) and refresh the Lucent display name in SHM.
/// Safe from the editor tick — relay discovery must not depend on analyze mode
/// or transport running.
pub(crate) fn editor_ensure_consumer(params: &LucentParams, shared: &SharedState) {
    let now_ms = lx_analysis::shm::now_ms();
    let mut slot = shared.shm_slot.load(Ordering::Acquire);
    if slot < 0
        && let Some(hub) = relay_hub()
        && let Some(claimed) = hub.claim_consumer_slot(now_ms)
    {
        slot = claimed as i32;
        shared.shm_slot.store(slot, Ordering::Release);
    }
    if slot < 0 {
        return;
    }
    let raw = params
        .name
        .try_read()
        .map(|n| n.clone())
        .unwrap_or_default();
    let my_name = lx_analysis::shm::display_name(&raw, slot as u8);
    if let Some(hub) = relay_hub() {
        hub.write_consumer_name(slot as u8, &my_name, now_ms);
    }
}

/// Resonance findings for one Lucent instance: `own` = peaks found in this
/// instance's own bus signal, `relay` = peaks found in the power-summed
/// spectrum of the Relay tracks it's listening to (group-level resonance
/// that can emerge from the sum even if no single track shows it).
#[derive(Default, Clone)]
pub struct ResonanceLists {
    pub own: Vec<(usize, f32)>,
    /// (bin, resonance score, contributor track names) — contributors are the
    /// Relay tracks whose own spectrum is above `CONTRIB_FLOOR_DB` at that bin,
    /// i.e. which tracks are actually feeding this group-level peak.
    pub relay: Vec<(usize, f32, Vec<String>)>,
}

/// Magnitude floor (dB) above which a Relay track counts as a contributor to
/// a group resonance peak. Same value as `MaskingAnalyzer`'s `FLOOR`, kept as
/// its own constant here since the two aren't the same computation.
const CONTRIB_FLOOR_DB: f32 = -70.0;

/// For each (bin, score) group-level peak, find which named Relay spectra
/// are actually above the floor at that bin.
pub(crate) fn attribute_contributors(
    peaks: &[(usize, f32)],
    relay_spectra: &[(String, Vec<f32>)],
) -> Vec<(usize, f32, Vec<String>)> {
    peaks
        .iter()
        .map(|(bin, score)| {
            let contributors = relay_spectra
                .iter()
                .filter(|(_, spec)| spec.get(*bin).copied().unwrap_or(-90.0) > CONTRIB_FLOOR_DB)
                .map(|(name, _)| name.clone())
                .collect();
            (*bin, *score, contributors)
        })
        .collect()
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

pub fn publish_resonance(key: usize, lists: ResonanceLists) {
    if let Ok(mut m) = resonance_registry().lock() {
        m.insert(key, lists);
    }
}

pub fn read_resonance(key: usize) -> ResonanceLists {
    resonance_registry()
        .lock()
        .ok()
        .and_then(|m| m.get(&key).cloned())
        .unwrap_or_default()
}

pub fn remove_resonance(key: usize) {
    if let Ok(mut m) = resonance_registry().lock() {
        m.remove(&key);
    }
}

/// Same instance-keyed pattern as `ResonanceRegistry`, for the top masking
/// collisions (bin, dB, contributor track names) of each Lucent instance.
type MaskingRegistry = Arc<Mutex<HashMap<usize, Vec<(usize, f32, Vec<String>)>>>>;

fn masking_registry() -> &'static MaskingRegistry {
    static REG: OnceLock<MaskingRegistry> = OnceLock::new();
    REG.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub fn publish_masking(key: usize, peaks: Vec<(usize, f32, Vec<String>)>) {
    if let Ok(mut m) = masking_registry().lock() {
        m.insert(key, peaks);
    }
}

pub fn read_masking(key: usize) -> Vec<(usize, f32, Vec<String>)> {
    masking_registry()
        .lock()
        .ok()
        .and_then(|m| m.get(&key).cloned())
        .unwrap_or_default()
}

pub fn remove_masking(key: usize) {
    if let Ok(mut m) = masking_registry().lock() {
        m.remove(&key);
    }
}

/// Per-bin power-sum (linear domain) of multiple dB spectra, back to dB.
/// Models how the tracks actually combine on a bus — e.g. two -6dB signals
/// at the same frequency sum to ~-3dB, not -6dB (`min`/`max` would say -6dB
/// and miss the additive buildup).
fn power_sum_spectrum(spectra: &[Vec<f32>]) -> Vec<f32> {
    let mut out = vec![-90.0f32; SPECTRUM_BINS];
    if spectra.is_empty() {
        return out;
    }
    for (j, out_slot) in out.iter_mut().enumerate() {
        let sum_lin: f32 = spectra
            .iter()
            .map(|s| 10f32.powf(s.get(j).copied().unwrap_or(-90.0) / 10.0))
            .sum();
        *out_slot = if sum_lin < 1e-9 {
            -90.0
        } else {
            10.0 * sum_lin.log10()
        };
    }
    out
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
fn suppress_harmonics(spectrum: &[f32], peaks: Vec<(usize, f32)>) -> Vec<(usize, f32)> {
    const MAX_HARMONIC: usize = 8;
    const BIN_TOLERANCE: usize = 2;
    const LOUDER_MARGIN_DB: f32 = 3.0;

    peaks
        .iter()
        .copied()
        .filter(|&(k, _)| {
            !peaks.iter().any(|&(k0, _)| {
                k0 < k
                    && spectrum[k] <= spectrum[k0] + LOUDER_MARGIN_DB
                    && (2..=MAX_HARMONIC).any(|n| (k0 * n).abs_diff(k) <= BIN_TOLERANCE)
            })
        })
        .collect()
}

mod editor;
mod process;

#[allow(dead_code)]
const WINDOW_W: u32 = 990;
#[allow(dead_code)]
const WINDOW_H: u32 = 550;

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
    /// this struct reads (FFT overlay bars, `top_peaks`).
    masking_map: Vec<f32>,
    /// This frame's ERB-smoothed collision level, before the persistence
    /// gate. Kept separate so a single loud-but-brief collision doesn't
    /// immediately count as "masking" (see `persistence`).
    raw: Vec<f32>,
    persistence: Vec<u32>,
    scratch: Vec<f32>,
    /// Names of the two tracks whose pairwise collision produced `scratch[j]`
    /// / `masking_map[j]` (empty when no collision above floor at that bin).
    scratch_contributors: Vec<Vec<String>>,
    masking_contributors: Vec<Vec<String>>,
}

impl MaskingAnalyzer {
    fn new(_sample_rate: f32) -> Self {
        Self {
            masking_map: vec![-90.0; SPECTRUM_BINS],
            raw: vec![-90.0; SPECTRUM_BINS],
            persistence: vec![0u32; SPECTRUM_BINS],
            scratch: vec![-90.0; SPECTRUM_BINS],
            scratch_contributors: vec![Vec::new(); SPECTRUM_BINS],
            masking_contributors: vec![Vec::new(); SPECTRUM_BINS],
        }
    }

    /// `relay_named` pairs each Relay spectrum with its track name so a
    /// masking collision can be attributed to the two tracks that caused it.
    /// `persistence_min` is the Sensitivity knob's shared persistence gate
    /// (same field resonance uses) — a collision only counts once it holds
    /// for that many frames, not on a single-frame blip.
    fn compute_masking(
        &mut self,
        own_spectrum: Option<&[f32]>,
        relay_named: &[(String, Vec<f32>)],
        floor_db: f32,
        sample_rate: f32,
        persistence_min: u32,
    ) {
        let n = self.masking_map.len();
        let bin_hz = sample_rate / (n as f32 * 2.0);
        let own_live = own_spectrum
            .map(|s| track_has_masking_signal(s, sample_rate))
            .unwrap_or(false);
        let relay_live: Vec<bool> = relay_named
            .iter()
            .map(|(_, s)| track_has_masking_signal(s, sample_rate))
            .collect();

        for j in 0..n {
            let freq = j as f32 * bin_hz;
            let mut active: [(f32, &str); 17] = [(-90.0f32, ""); 17];
            let mut count = 0usize;

            if let Some(own_spec) = own_spectrum {
                let own = spectrum_physical_db(own_spec.get(j).copied().unwrap_or(-90.0), freq);
                if own_live && own > floor_db {
                    active[count] = (own, "Own");
                    count += 1;
                }
            }
            for ((name, relay), live) in relay_named.iter().zip(relay_live.iter()) {
                if let Some(&v) = relay.get(j) {
                    let phys = spectrum_physical_db(v, freq);
                    if *live && phys > floor_db && count < active.len() {
                        active[count] = (phys, name.as_str());
                        count += 1;
                    }
                }
            }

            let mut best = -90.0f32;
            let mut best_pair = ("", "");
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
            self.scratch_contributors[j] = if best > -90.0 {
                vec![best_pair.0.to_string(), best_pair.1.to_string()]
            } else {
                Vec::new()
            };
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
            self.masking_contributors[j] = self.scratch_contributors[m_idx].clone();
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

    /// Top `n` masking-collision bins (frequency + dB + the two contributing
    /// track names), sorted by severity. Mirrors the selection logic that
    /// used to live in `editor.rs::masking_summary`, moved here so the
    /// contributor names travel with the peak instead of being dropped.
    fn top_peaks(&self, n: usize, floor_db: f32) -> Vec<(usize, f32, Vec<String>)> {
        let mut peaks: Vec<(usize, f32, Vec<String>)> = self
            .masking_map
            .iter()
            .enumerate()
            .filter(|&(_, &db)| db > floor_db)
            .map(|(i, &db)| (i, db, self.masking_contributors[i].clone()))
            .collect();
        peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        peaks.truncate(n);
        peaks
    }
}

// ─── Peak tracker ─────────────────────────────────────────────────────────────

struct PeakTracker {
    persistence: Vec<u32>,
    last_prominence: Vec<f32>,
    resonance_score: Vec<f32>,
}

impl PeakTracker {
    fn new() -> Self {
        Self {
            persistence: vec![0u32; SPECTRUM_BINS],
            last_prominence: vec![0.0; SPECTRUM_BINS],
            resonance_score: vec![0.0; SPECTRUM_BINS],
        }
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
    fn find_peaks(
        &self,
        spectrum: &[f32],
        t: &SensitivityThresholds,
        sample_rate: f32,
    ) -> Vec<(usize, f32)> {
        const BASELINE_WINDOW: usize = 8;
        const FLATNESS_WINDOW: usize = 4;
        const MAX_BW_SEARCH: usize = 24;

        let n = spectrum.len();
        let bin_hz = sample_rate / (n as f32 * 2.0);
        let mut peaks = Vec::new();
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

            peaks.push((k, contrast));
        }
        peaks
    }

    fn update(&mut self, peaks: &[(usize, f32)]) {
        let peak_bins: Vec<usize> = peaks.iter().map(|(k, _)| *k).collect();
        for (k, prom) in peaks {
            self.last_prominence[*k] = *prom;
        }

        const PERSIST_CAP: u32 = 40;
        for k in 0..SPECTRUM_BINS {
            if peak_bins.contains(&k) {
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

    fn resonance_peaks(&self, t: &SensitivityThresholds) -> Vec<(usize, f32)> {
        let mut resonant = Vec::new();
        for k in 1..SPECTRUM_BINS.saturating_sub(1) {
            if self.resonance_score[k] > t.score_min && self.persistence[k] > t.persistence_min {
                resonant.push((k, self.resonance_score[k]));
            }
        }
        resonant.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        resonant.truncate(16);
        resonant
    }
}

// ─── Params ──────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct LucentParams {
    #[param(
        name = "Analyze Mode",
        default = 0,
        range = "discrete(0, 2)",
        group = "Lucent"
    )]
    pub analyze_mode: IntParam,
    #[param(name = "Resonance", default = 1, group = "Lucent")]
    pub resonance_active: BoolParam,
    #[param(name = "Masking", default = 1, group = "Lucent")]
    pub masking_active: BoolParam,
    #[param(name = "Bypass", default = 0, group = "Lucent")]
    pub bypass_active: BoolParam,
    /// How deep the resonance/masking detectors dig: 0% = shallow (only
    /// strong, sustained findings), 100% = deep (surfaces weaker, shorter
    /// ones too). 50% reproduces the previously hand-tuned thresholds.
    #[param(
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
    /// editor via Truce's `Arc<LucentParams>` so renames apply when
    /// transport is stopped.
    #[skip]
    pub name_bg: Arc<RwLock<String>>,
    #[skip]
    pub shared: Arc<SharedState>,
}

impl LucentParams {
    /// Real value display for `unit = "%"` params: our plain values are
    /// already the percent number (e.g. `50.0` means `50%`), not a
    /// 0.0-1.0 fraction. `truce_params::format_param_value`'s built-in
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
    pub(crate) snap_fft: SnapFFT,
    pub(crate) sample_rate: f32,
    pub(crate) peak_hold_value: f32,
    pub(crate) peak_hold_l_value: f32,
    pub(crate) peak_hold_r_value: f32,
    pub(crate) claimed_lucent_slot: Option<u8>,
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
        let adopted = params.shared.shm_slot.load(Ordering::Acquire);
        if adopted >= 0 {
            self.claimed_lucent_slot = Some(adopted as u8);
        } else if let Some(hub) = relay_hub() {
            self.claimed_lucent_slot = hub.claim_consumer_slot(now_ms);
        }
        params.shared.shm_slot.store(
            self.claimed_lucent_slot.map(|s| s as i32).unwrap_or(-1),
            Ordering::Release,
        );
    }

    pub(crate) fn publish_consumer_name(&mut self, params: &LucentParams, now_ms: u64) {
        if let Ok(name) = params.name.try_read() {
            self.cached_name = name.clone();
            if let Ok(mut bg) = params.name_bg.try_write() {
                *bg = name.clone();
            }
        }
        self.cached_display_name = self
            .claimed_lucent_slot
            .map(|slot| lx_analysis::shm::display_name(&self.cached_name, slot))
            .unwrap_or_else(|| self.cached_name.clone());
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
                let slot = shared.shm_slot.load(Ordering::Acquire);
                if slot < 0 {
                    continue;
                }
                if let Some(hub) = relay_hub() {
                    let raw = name_bg.read().ok().map(|n| n.clone()).unwrap_or_default();
                    let name = lx_analysis::shm::display_name(&raw, slot as u8);
                    hub.write_consumer_name(slot as u8, &name, lx_analysis::shm::now_ms());
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
            snap_fft: SnapFFT::new(),
            sample_rate: 44100.0,
            peak_hold_value: -100.0,
            peak_hold_l_value: -100.0,
            peak_hold_r_value: -100.0,
            claimed_lucent_slot: None,
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

    fn bus_layouts() -> Vec<BusLayout> {
        vec![BusLayout::stereo()]
    }

    fn init(params: &Self::Params, _cx: &InitContext) -> Self::DspState {
        let mut state = LucentDspState::default();
        state.instance_key = params as *const _ as usize;
        let now_ms = lx_analysis::shm::now_ms();
        state.ensure_consumer_slot(params, now_ms);
        state.publish_consumer_name(params, now_ms);
        state.spawn_consumer_heartbeat(params);
        state
    }

    fn reset(state: &mut LucentDspState, params: &LucentParams, config: &AudioConfig) {
        let sr = config.sample_rate;
        state.sample_rate = sr as f32;
        let now_ms = lx_analysis::shm::now_ms();
        params
            .shared
            .sample_rate
            .store(sr as f32, Ordering::Release);

        state.ensure_consumer_slot(params, now_ms);
        state.publish_consumer_name(params, now_ms);
        state.spawn_consumer_heartbeat(params);
    }

    fn process(
        state: &mut LucentDspState,
        params: &LucentParams,
        buffer: &mut AudioBuffer,
        _events: &EventList,
        _ctx: &mut ProcessContext,
    ) -> ProcessStatus {
        process::run(state, params, buffer)
    }

    fn snapshot_into(_state: &LucentDspState, _buf: &mut Vec<u8>) -> bool {
        false
    }
    fn load_state(_state: &mut LucentDspState, _data: &[u8]) -> Result<(), StateLoadError> {
        Ok(())
    }

    fn state_changed(state: &mut LucentDspState, params: &LucentParams) {
        // Preset recall / undo / session load — sync cached name from restored params.
        if let Ok(n) = params.name.try_read() {
            state.cached_name = n.clone();
            if let Ok(mut bg) = params.name_bg.try_write() {
                *bg = n.clone();
            }
            state.cached_display_name = state
                .claimed_lucent_slot
                .map(|slot| lx_analysis::shm::display_name(&state.cached_name, slot))
                .unwrap_or_else(|| state.cached_name.clone());
            // ponytail: state_changed may fire before reset, so relay_hub()
            // might not be initialized yet. process() writes the name every
            // audio block anyway, and the background heartbeat thread follows.
        }
    }

    fn editor(params: Arc<Self::Params>) -> Box<dyn Editor> {
        editor::build_editor(params)
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
    }
}

truce::plugin! {
    logic: Lucent,
    params: LucentParams,
}

#[cfg(test)]
mod masking_tests {
    use super::MaskingAnalyzer;
    use lx_analysis::{spectrum_physical_db, spectrum_tilt_db, SPECTRUM_BINS};

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
        let relays = [
            ("Relay A".to_string(), silent.clone()),
            ("Relay B".to_string(), silent),
        ];
        let mut analyzer = MaskingAnalyzer::new(sr);
        analyzer.compute_masking(None, &relays, -70.0, sr, 4);
        assert!(
            analyzer.top_peaks(3, -70.0).is_empty(),
            "tilted silence must not register as masking"
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
    use crate::Plugin;
    use std::time::Duration;

    #[test]
    fn renders_pass_through() {
        use truce_test::{InputSource, assertions, driver};
        let result = driver!(Plugin)
            .duration(Duration::from_millis(50))
            .input(InputSource::Constant(0.5))
            .run();
        assertions::assert_no_nans(&result);
        assertions::assert_nonzero(&result);
    }

    #[test]
    fn state_round_trips() {
        truce_test::assert_state_round_trip::<Plugin>();
    }
}
