#![allow(unsafe_op_in_unsafe_fn)]

// Equilibrium — Pre-master spectral balancer (AURA).
//
// 5-band LR2 crossover (80/300/2000/6000 Hz) with per-band Gain,
// Stereo Width (M/S), Pan (constant-power), and Solo.
//
// Signal chain:
//   DC HP@8Hz → [LP@35kHz if sr≥88.2k] → 5-band Crossover
//   → per-band: Gain → M/S Width → Pan → Solo
//   → sum → Mono Floor (Side HPF) → Mono/Delta → Gain → Auto Gain → clamp

use std::sync::Arc;
use aura::prelude::*;

use aura_dsp::analysis::*;
use aura_dsp::analysis::product_shared::EquilibriumShared;
use aura_dsp::fx::{AutoLoudMeter, Biquad, LR2Crossover};

mod editor;
mod presets;
mod process;

pub(crate) const BAND_COUNT: usize = 5;
#[allow(dead_code)]
const WINDOW_W: u32 = 990;
#[allow(dead_code)]
const WINDOW_H: u32 = 670;

// ─── Helpers ─────────────────────────────────────────────────────────────────

#[inline]
pub(crate) fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

#[inline]
pub(crate) fn gain_to_db(gain: f32) -> f32 {
    if gain < 1e-9 {
        -90.0
    } else {
        20.0 * gain.log10()
    }
}

pub(crate) const MINUS_INF_DB: f32 = -90.0;

// ─── Params ──────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct EquilibriumParams {
    // 5 Band Gains
    #[param(
        id = 1,
        name = "Sub Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Gain"
    )]
    pub low_gain: FloatParam,
    #[param(
        id = 2,
        name = "Bass Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Gain"
    )]
    pub bass_gain: FloatParam,
    #[param(
        id = 3,
        name = "Mid Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Gain"
    )]
    pub mid_gain: FloatParam,
    #[param(
        id = 4,
        name = "Pres Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Gain"
    )]
    pub high_mid_gain: FloatParam,
    #[param(
        id = 5,
        name = "Air Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Gain"
    )]
    pub high_gain: FloatParam,

    // 5 Band Widths
    #[param(
        id = 6,
        name = "Sub Width",
        default = 100.0,
        range = "linear(0.0, 150.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Width"
    )]
    pub low_width: FloatParam,
    #[param(
        id = 7,
        name = "Bass Width",
        default = 100.0,
        range = "linear(0.0, 150.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Width"
    )]
    pub bass_width: FloatParam,
    #[param(
        id = 8,
        name = "Mid Width",
        default = 100.0,
        range = "linear(0.0, 150.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Width"
    )]
    pub mid_width: FloatParam,
    #[param(
        id = 9,
        name = "Pres Width",
        default = 100.0,
        range = "linear(0.0, 150.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Width"
    )]
    pub high_mid_width: FloatParam,
    #[param(
        id = 10,
        name = "Air Width",
        default = 100.0,
        range = "linear(0.0, 150.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Width"
    )]
    pub high_width: FloatParam,

    // 5 Band Pans (-1.0 L to +1.0 R)
    #[param(
        id = 11,
        name = "Sub Pan",
        default = 0.0,
        range = "linear(-1.0, 1.0)",
        smooth = "linear(20)",
        group = "Pan"
    )]
    pub low_pan: FloatParam,
    #[param(
        id = 12,
        name = "Bass Pan",
        default = 0.0,
        range = "linear(-1.0, 1.0)",
        smooth = "linear(20)",
        group = "Pan"
    )]
    pub bass_pan: FloatParam,
    #[param(
        id = 13,
        name = "Mid Pan",
        default = 0.0,
        range = "linear(-1.0, 1.0)",
        smooth = "linear(20)",
        group = "Pan"
    )]
    pub mid_pan: FloatParam,
    #[param(
        id = 14,
        name = "Pres Pan",
        default = 0.0,
        range = "linear(-1.0, 1.0)",
        smooth = "linear(20)",
        group = "Pan"
    )]
    pub high_mid_pan: FloatParam,
    #[param(
        id = 15,
        name = "Air Pan",
        default = 0.0,
        range = "linear(-1.0, 1.0)",
        smooth = "linear(20)",
        group = "Pan"
    )]
    pub high_pan: FloatParam,

    // Mono Floor frequency (0 = off, 1–300 Hz)
    #[param(
        id = 16,
        name = "Mono Floor",
        default = 0.0,
        range = "linear(0.0, 300.0)",
        unit = "Hz"
    )]
    pub mono_floor: FloatParam,

    // Output manual gain
    #[param(
        id = 17,
        name = "Output Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)"
    )]
    pub output_gain: FloatParam,

    // Solos
    #[param(id = 18, name = "Solo Sub", default = 0)]
    pub solo_low: BoolParam,
    #[param(id = 19, name = "Solo Bass", default = 0)]
    pub solo_bass: BoolParam,
    #[param(id = 20, name = "Solo Mid", default = 0)]
    pub solo_mid: BoolParam,
    #[param(id = 21, name = "Solo Pres", default = 0)]
    pub solo_high_mid: BoolParam,
    #[param(id = 22, name = "Solo Air", default = 0)]
    pub solo_high: BoolParam,

    // Modes
    #[param(id = 23, name = "Mono Sum", default = 0, group = "Monitor")]
    pub mono_active: BoolParam,
    #[param(id = 24, name = "Delta Diff", default = 0, group = "Monitor")]
    pub delta_active: BoolParam,
    #[param(id = 25, name = "Listen Profile", default = 0, group = "Monitor")]
    pub listen_active: BoolParam,
    #[param(id = 26, name = "Auto Loudness", default = 0, group = "Monitor")]
    pub auto_gain_active: BoolParam,
    #[param(id = 27, name = "Bypass", default = 0, group = "Monitor")]
    pub bypass_active: BoolParam,

    // Pre-Master mode
    #[param(id = 28, name = "Pre-Master", default = 0, group = "Monitor")]
    pub pre_master_active: BoolParam,
    #[param(id = 29, name = "Pre-Master Target", default = -3.0, range = "linear(-6.0, -3.0)", unit = "dB")]
    pub pre_master_target_db: FloatParam,

    #[skip]
    pub shared: Arc<EquilibriumShared>,
}

impl EquilibriumParams {
    /// Real value display for `unit = "%"` params: our plain values are
    /// already the percent number (e.g. `100.0` means `100%`), not a
    /// 0.0-1.0 fraction. `truce_params::format_param_value`'s built-in
    /// Percent case multiplies by 100 assuming the latter, so it would
    /// show `10000%` for a real 100% value without this override.
    fn fmt_pct(&self, value: f64) -> String {
        format!("{value:.1}%")
    }
}

// ─── Plugin ───────────────────────────────────────────────────────────────────

pub struct Equilibrium;

pub struct EquilibriumDspState {
    // Filters L
    pub(crate) low_cut_l: Biquad,
    pub(crate) high_cut_l: Biquad,
    pub(crate) xo_bass_mid_l: LR2Crossover,
    pub(crate) xo_low_bass_l: LR2Crossover,
    pub(crate) xo_mid_high_l: LR2Crossover,
    pub(crate) xo_highmid_high_l: LR2Crossover,

    // Filters R
    pub(crate) low_cut_r: Biquad,
    pub(crate) high_cut_r: Biquad,
    pub(crate) xo_bass_mid_r: LR2Crossover,
    pub(crate) xo_low_bass_r: LR2Crossover,
    pub(crate) xo_mid_high_r: LR2Crossover,
    pub(crate) xo_highmid_high_r: LR2Crossover,

    // Mono Floor filter (Side HPF)
    pub(crate) mono_floor_filter: Biquad,

    // Temporal smoothing
    pub(crate) rms_decay_coef: f32,
    pub(crate) correlation_decay_coef: f32,

    // Smoothed states
    pub(crate) smoothed_band_power: [f32; BAND_COUNT],
    pub(crate) listen_band_power_sum: [f64; BAND_COUNT],
    pub(crate) listen_sample_count: u64,
    pub(crate) listen_lo_ema: [f64; BAND_COUNT],
    pub(crate) listen_hi_ema: [f64; BAND_COUNT],
    pub(crate) listen_ref_ema: [f64; BAND_COUNT],
    pub(crate) listen_levels_ema: [f32; BAND_COUNT],
    pub(crate) listen_min_ema: [f32; BAND_COUNT],
    pub(crate) listen_max_ema: [f32; BAND_COUNT],

    // Correlation
    pub(crate) corr_avg_lr: f32,
    pub(crate) corr_avg_l2: f32,
    pub(crate) corr_avg_r2: f32,

    // Peak hold
    pub(crate) peak_hold_value: f32,
    pub(crate) peak_hold_l_value: f32,
    pub(crate) peak_hold_r_value: f32,

    // Stereo balance
    pub(crate) smoothed_power_l: f32,
    pub(crate) smoothed_power_r: f32,

    // Auto Gain
    pub(crate) auto_gain_comp: f32,

    // Pre-Master
    pub(crate) pre_master_gain: f32,
    pub(crate) pre_master_active_prev: bool,
    pub(crate) pre_master_measure_peak: f32,
    pub(crate) pre_master_measure_count: u32,

    // Goniometer
    pub(crate) scope_vis_envelope: f32,

    // AUTO LOUD
    pub(crate) auto_loud_in: AutoLoudMeter,
    pub(crate) auto_loud_out: AutoLoudMeter,

    // SNAP FFT
    pub(crate) snap_fft: SnapFFT,

    // Cached parameters (dirty-flag optimization)
    pub(crate) cached_mono_floor_freq: f32,
    pub(crate) cached_sample_rate: f32,
}

impl Default for EquilibriumDspState {
    fn default() -> Self {
        Self::new()
    }
}

impl EquilibriumDspState {
    fn new() -> Self {
        Self {
            low_cut_l: Biquad::new(),
            high_cut_l: Biquad::new(),
            xo_bass_mid_l: LR2Crossover::new(),
            xo_low_bass_l: LR2Crossover::new(),
            xo_mid_high_l: LR2Crossover::new(),
            xo_highmid_high_l: LR2Crossover::new(),
            low_cut_r: Biquad::new(),
            high_cut_r: Biquad::new(),
            xo_bass_mid_r: LR2Crossover::new(),
            xo_low_bass_r: LR2Crossover::new(),
            xo_mid_high_r: LR2Crossover::new(),
            xo_highmid_high_r: LR2Crossover::new(),
            mono_floor_filter: Biquad::new(),
            rms_decay_coef: 0.001,
            correlation_decay_coef: 0.005,
            smoothed_band_power: [0.0; BAND_COUNT],
            listen_band_power_sum: [0.0; BAND_COUNT],
            listen_sample_count: 0,
            listen_lo_ema: [f64::INFINITY; BAND_COUNT],
            listen_hi_ema: [f64::NEG_INFINITY; BAND_COUNT],
            listen_ref_ema: [0.0; BAND_COUNT],
            listen_levels_ema: [-90.0; BAND_COUNT],
            listen_min_ema: [-90.0; BAND_COUNT],
            listen_max_ema: [-90.0; BAND_COUNT],
            smoothed_power_l: 0.0,
            smoothed_power_r: 0.0,
            corr_avg_lr: 0.0,
            corr_avg_l2: 0.0,
            corr_avg_r2: 0.0,
            peak_hold_value: MINUS_INF_DB,
            peak_hold_l_value: MINUS_INF_DB,
            peak_hold_r_value: MINUS_INF_DB,
            auto_gain_comp: 1.0,
            pre_master_gain: 1.0,
            pre_master_active_prev: false,
            pre_master_measure_peak: 0.0,
            pre_master_measure_count: 0,
            scope_vis_envelope: 1e-4,
            auto_loud_in: AutoLoudMeter::new(44100.0),
            auto_loud_out: AutoLoudMeter::new(44100.0),
            snap_fft: SnapFFT::new(),
            cached_mono_floor_freq: -999.0,
            cached_sample_rate: -999.0,
        }
    }
}

// ─── PluginLogic ─────────────────────────────────────────────────────────────

impl PluginLogic for Equilibrium {
    type Params = EquilibriumParams;
    type DspState = EquilibriumDspState;

    fn info() -> PluginInfo {
        let mut info = PluginInfo::new(
            "Equilibrium",
            "LX Audiolabs",
            env!("CARGO_PKG_VERSION"),
            "equilibrium",
        );
        // Stable ship IDs — must match pre-AURA truce Equilibrium (Bitwig keys
        // sessions on clap id; com.lx-audiolabs.* breaks existing projects).
        info.clap_id = "be.lxndr.equilibrium";
        info.vst3_id = "be.lxndr.equilibrium";
        info.lv2_uri = "https://lx-audiolabs.com/lv2/equilibrium";
        info.category = PluginCategory::Effect;
        info
    }

    fn bus_layouts() -> Vec<BusLayout> {
        vec![BusLayout::stereo()]
    }

    fn init(params: &Self::Params, sample_rate: f64) -> Self::DspState {
        let mut state = EquilibriumDspState::default();
        Self::reset(&mut state, params, &AudioConfig::new(sample_rate, 4096));
        state
    }

    fn reset(state: &mut EquilibriumDspState, params: &EquilibriumParams, config: &AudioConfig) {
        let sr = config.sample_rate as f32;
        state.cached_sample_rate = sr;

        // Recreate Auto-Loud meters at host sample rate
        state.auto_loud_in = AutoLoudMeter::new(sr);
        state.auto_loud_out = AutoLoudMeter::new(sr);

        // DC/infrasonic protection HP @ 8 Hz
        state.low_cut_l.set_butterworth_hp(2.0, sr);
        state.low_cut_r.set_butterworth_hp(2.0, sr);

        // LP @ 35 kHz only at ≥ 88.2 kHz
        if sr >= 88_200.0 {
            state.high_cut_l.set_butterworth_lp(35000.0, sr);
            state.high_cut_r.set_butterworth_lp(35000.0, sr);
        }

        // Crossover frequencies
        for (xo_l, xo_r, fc) in [
            (&mut state.xo_bass_mid_l, &mut state.xo_bass_mid_r, 300.0),
            (&mut state.xo_low_bass_l, &mut state.xo_low_bass_r, 80.0),
            (&mut state.xo_mid_high_l, &mut state.xo_mid_high_r, 2000.0),
            (
                &mut state.xo_highmid_high_l,
                &mut state.xo_highmid_high_r,
                6000.0,
            ),
        ] {
            xo_l.set_cutoff(fc, sr);
            xo_r.set_cutoff(fc, sr);
        }

        // Mono floor if initially active
        let mm_init = params.mono_floor.raw_target() as f32;
        if mm_init > 1.0 {
            state.mono_floor_filter.set_butterworth_hp(mm_init, sr);
            state.cached_mono_floor_freq = mm_init;
        }

        state.rms_decay_coef = 1.0 - (-1.0 / (0.5 * sr)).exp();
        state.correlation_decay_coef = 1.0 - (-1.0 / (0.1 * sr)).exp();

        // Reset all filter states
        state.low_cut_l.reset();
        state.low_cut_r.reset();
        state.high_cut_l.reset();
        state.high_cut_r.reset();
        state.xo_bass_mid_l.reset();
        state.xo_bass_mid_r.reset();
        state.xo_low_bass_l.reset();
        state.xo_low_bass_r.reset();
        state.xo_mid_high_l.reset();
        state.xo_mid_high_r.reset();
        state.xo_highmid_high_l.reset();
        state.xo_highmid_high_r.reset();
        state.mono_floor_filter.reset();

        params
            .shared
            .sample_rate
            .store(sr, std::sync::atomic::Ordering::Release);
    }

    fn process(
        state: &mut EquilibriumDspState,
        params: &EquilibriumParams,
        buffer: &mut AudioBuffer<'_, f32>,
        _ctx: &mut ProcessContext,
    ) -> ProcessStatus {
        process::run(state, params, buffer)
    }

    fn editor(params: Arc<Self::Params>) -> Option<Box<dyn Editor>> {
        Some(editor::build_editor(params))
    }
}

#[cfg(feature = "clap")]
aura::export!(Equilibrium);

#[cfg(feature = "vst3")]
aura::export_vst3!(Equilibrium);

#[cfg(feature = "lv2")]
aura::export_lv2!(Equilibrium);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips() {
        aura_test::assert_state_round_trip::<Equilibrium>();
    }
}
