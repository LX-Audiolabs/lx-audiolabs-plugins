#![allow(unsafe_op_in_unsafe_fn)]

// Meridian — Track and group shaper (truce port).
//
// 5-band EQ with slope control, soft-knee compressor, exciter, tube warmth,
// tilt EQ, stereo width/pan, and Auto Loud LUFS metering.
//
// Signal chain:
//   HPF/LPF → 5-band Series EQ → Tilt → Exciter → Compressor →
//   Warmth → Inflate → Pan → Stereo Width → Mono/Delta → Gain → clamp

use realfft::RealFftPlanner;
use std::f32::consts::FRAC_PI_4;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use truce::prelude::*;
use truce_core::editor::Editor;
use truce_core::state::StateLoadError;

use shared_analysis::{SCOPE_BUFFER_LEN, SPECTRUM_BINS, SharedState, SnapFFT, SnapMode};
use shared_dsp::{
    AutoLoudMeter, Biquad, Compressor, DBTP_CEILING, FtzDazGuard, LR2Crossover, TiltEq,
};

mod editor;

// Window size is defined in editor.rs for the Slint UI.

// ─── Helpers ─────────────────────────────────────────────────────────────────

#[inline]
fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

#[inline]
fn gain_to_db(gain: f32) -> f32 {
    if gain < 1e-9 {
        -90.0
    } else {
        20.0 * gain.log10()
    }
}

/// Soft clipping — odd harmonics (Exciter).
#[inline]
fn soft_clip(x: f32) -> f32 {
    let abs_x = x.abs();
    if abs_x <= 1.0 {
        x - (x * x * x) / 3.0
    } else if x > 0.0 {
        2.0 / 3.0
    } else {
        -2.0 / 3.0
    }
}

/// Tube-style saturation — DC bias shifts operating point for even harmonics (Warmth).
#[inline]
fn tube_warm(x: f32) -> f32 {
    const BIAS: f32 = 0.1;
    (x + BIAS).tanh() - BIAS.tanh()
}

/// Approximated Oxford-Inflator-style loudness/density waveshaper (Inflate).
/// Not a Sonnox algorithm clone — the "probability density shifting" process is
/// patented/undocumented. `curve` -50..+50: negative = subtle/tight, 0 = balanced,
/// positive = fat/loud. Drive-varying tanh, always finite for finite input.
/// Fix 2026-07-03: normalize by `drive` (like `tube_warm`), not `tanh(drive)` —
/// the old `/tanh(drive)` normalization forced unity gain at x=1 (full scale) but
/// blew up the small-signal gain (slope at x=0) to `drive/tanh(drive)`, i.e. up to
/// 6.0x at CURVE=+50 and 2.3x already at CURVE=0 ("balanced"). Quiet/mid-level
/// program material got boosted and colored far more than intended — harsh even at
/// neutral. Community reverse-engineering of the real Oxford Inflator (small-signal
/// gain coefficient a1 = 1 + (curve+50)/100, i.e. 1.0..2.0 linear with curve) puts
/// the target gain range at 1.0..2.0. `/drive` normalization gives unity slope at
/// x=0 for the tanh term; multiplying by `gain` (1..2) reproduces that range while
/// keeping curvature/drive (1..6) untouched from the 2026-07-02(b) fix.
#[inline]
fn inflate_shape(x: f32, curve: f32) -> f32 {
    let t = (curve + 50.0) / 100.0; // -50..+50 -> 0..1
    let drive = 1.0 + t * t * 5.0; // quadratic: 0=1 (clean), 0.5=2.25 (gentle), 1=6 (fat/aggressive)
    let gain = 1.0 + t; // 1..2, small-signal gain matching Oxford's a1 range
    (x * drive).tanh() * gain / drive
}

// ─── Params ──────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct MeridianParams {
    // HPF / LPF
    #[param(
        name = "Low Cut",
        default = 2.0,
        range = "log(2.0, 2000.0)",
        unit = "Hz",
        group = "Filter"
    )]
    pub hpf_freq: FloatParam,
    #[param(
        name = "High Cut",
        default = 35000.0,
        range = "log(200.0, 35000.0)",
        unit = "Hz",
        group = "Filter"
    )]
    pub lpf_freq: FloatParam,
    #[param(
        name = "Cut Slope",
        default = 0,
        range = "discrete(0, 1)",
        group = "Filter"
    )]
    pub cut_slope: IntParam,

    // Bass EQ shelf
    #[param(
        name = "Lo Shelf Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Lo Shelf"
    )]
    pub bass_gain: FloatParam,
    #[param(
        name = "Lo Shelf Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Lo Shelf"
    )]
    pub bass_slope: IntParam,

    // Lo-Mid EQ
    #[param(
        name = "Lo-Mid Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Lo-Mid"
    )]
    pub lo_mid_gain: FloatParam,
    #[param(
        name = "Lo-Mid Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Lo-Mid"
    )]
    pub lo_mid_slope: IntParam,

    // Mid EQ
    #[param(
        name = "Mid Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Mid"
    )]
    pub mid_gain: FloatParam,
    #[param(
        name = "Mid Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Mid"
    )]
    pub mid_slope: IntParam,

    // High EQ
    #[param(
        name = "Hi-Mid Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Hi-Mid"
    )]
    pub high_gain: FloatParam,
    #[param(
        name = "Hi-Mid Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Hi-Mid"
    )]
    pub high_slope: IntParam,

    // Excite (high shelf)
    #[param(
        name = "Hi Shelf Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Hi Shelf"
    )]
    pub excite_gain: FloatParam,
    #[param(
        name = "Hi Shelf Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Hi Shelf"
    )]
    pub excite_slope: IntParam,

    // EQ band frequencies
    #[param(
        name = "Lo Shelf Freq",
        default = 80.0,
        range = "log(40.0, 200.0)",
        unit = "Hz",
        group = "EQ/Lo Shelf"
    )]
    pub eq_freq_1: FloatParam,
    #[param(
        name = "Lo-Mid Freq",
        default = 300.0,
        range = "log(150.0, 800.0)",
        unit = "Hz",
        group = "EQ/Lo-Mid"
    )]
    pub eq_freq_2: FloatParam,
    #[param(
        name = "Mid Freq",
        default = 1000.0,
        range = "log(500.0, 3000.0)",
        unit = "Hz",
        group = "EQ/Mid"
    )]
    pub eq_freq_3: FloatParam,
    #[param(
        name = "Hi-Mid Freq",
        default = 4000.0,
        range = "log(2000.0, 10000.0)",
        unit = "Hz",
        group = "EQ/Hi-Mid"
    )]
    pub eq_freq_4: FloatParam,
    #[param(
        name = "Hi Shelf Freq",
        default = 12000.0,
        range = "log(6000.0, 20000.0)",
        unit = "Hz",
        group = "EQ/Hi Shelf"
    )]
    pub eq_freq_5: FloatParam,

    // Tilt EQ
    #[param(
        name = "Tilt",
        default = 0.0,
        range = "linear(-1.5, 1.5)",
        unit = "dB",
        group = "Tilt"
    )]
    pub tilt_gain: FloatParam,

    // Warmth (tube saturation)
    #[param(
        name = "Warmth Drive",
        default = 0.0,
        range = "linear(0.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Saturator"
    )]
    pub warmth_drive: FloatParam,
    #[param(
        name = "Warmth Mix",
        default = 0.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Saturator"
    )]
    pub warmth_mix: FloatParam,

    // Exciter (HF saturation)
    #[param(
        name = "Excite Amount",
        default = 0.0,
        range = "linear(0.0, 30.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Exciter"
    )]
    pub excite_amount: FloatParam,
    #[param(
        name = "Excite Blend",
        default = 0.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Exciter"
    )]
    pub excite_blend: FloatParam,
    #[param(
        name = "Excite Freq",
        default = 8000.0,
        range = "log(6000.0, 12000.0)",
        unit = "Hz",
        group = "Exciter"
    )]
    pub excite_freq: FloatParam,

    // Compressor
    #[param(
        name = "Comp Threshold",
        default = 0.0,
        range = "linear(-30.0, 0.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_threshold: FloatParam,
    #[param(
        name = "Comp Mix",
        default = 0.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_mix: FloatParam,
    #[param(
        name = "Comp Attack",
        default = 15.0,
        range = "linear(5.0, 50.0)",
        unit = "ms",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_attack: FloatParam,
    #[param(
        name = "Comp Release",
        default = 120.0,
        range = "linear(50.0, 300.0)",
        unit = "ms",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_release: FloatParam,
    #[param(
        name = "Comp Ratio",
        default = 2.0,
        range = "linear(1.5, 4.0)",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_character: FloatParam,
    #[param(
        name = "Comp Makeup",
        default = 0.0,
        range = "linear(0.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_makeup: FloatParam,

    // Inflate (Oxford-Inflator-inspired loudness/density waveshaper)
    #[param(
        name = "Inflate Effect",
        default = 0.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Inflate"
    )]
    pub inflate_effect: FloatParam,
    #[param(
        name = "Inflate Curve",
        default = 0.0,
        range = "linear(-50.0, 50.0)",
        smooth = "linear(20)",
        group = "Inflate"
    )]
    pub inflate_curve: FloatParam,
    #[param(name = "Inflate Band Split", default = 0, group = "Inflate")]
    pub inflate_band_split: BoolParam,
    #[param(name = "Inflate Clip", default = 0, group = "Inflate")]
    pub inflate_clip: BoolParam,

    // Stereo Width
    #[param(
        name = "Stereo Width",
        default = 100.0,
        range = "linear(0.0, 200.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Stereo/Routing"
    )]
    pub stereo_width: FloatParam,
    // Pan
    #[param(
        name = "Pan",
        default = 0.0,
        range = "linear(-1.0, 1.0)",
        smooth = "linear(20)",
        group = "Stereo/Routing"
    )]
    pub pan: FloatParam,
    // Output Gain
    #[param(
        name = "Output Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Stereo/Routing"
    )]
    pub output_gain: FloatParam,

    // States
    #[param(name = "Mono Sum", default = 0, group = "Stereo/Routing")]
    pub mono_active: BoolParam,
    #[param(name = "Delta Diff", default = 0, group = "Stereo/Routing")]
    pub delta_active: BoolParam,
    #[param(name = "Bypass", default = 0, group = "Stereo/Routing")]
    pub bypass_active: BoolParam,

    #[skip]
    pub shared: Arc<SharedState>,
}

impl MeridianParams {
    /// Real value display for `unit = "%"` params: our plain values are
    /// already the percent number (e.g. `100.0` means `100%`), not a
    /// 0.0-1.0 fraction. `truce_params::format_param_value`'s built-in
    /// Percent case multiplies by 100 assuming the latter, so it would
    /// show `10000%` for a real 100% value without this override.
    fn fmt_pct(&self, value: f64) -> String {
        format!("{value:.1}%")
    }
}

// ─── Plugin ──────────────────────────────────────────────────────────────────

pub struct Meridian;

pub struct MeridianDspState {

    // HPF/LPF
    hpf_l: Biquad,
    hpf_r: Biquad,
    lpf_l: Biquad,
    lpf_r: Biquad,
    hpf2_l: Biquad,
    hpf2_r: Biquad,
    lpf2_l: Biquad,
    lpf2_r: Biquad,

    // EQ bands
    bass_l: Biquad,
    bass_r: Biquad,
    lo_mid_l: Biquad,
    lo_mid_r: Biquad,
    mid_l: Biquad,
    mid_r: Biquad,
    high_l: Biquad,
    high_r: Biquad,
    excite_l: Biquad,
    excite_r: Biquad,

    tilt_l: TiltEq,
    tilt_r: TiltEq,

    excite_hp_l: Biquad,
    excite_hp_r: Biquad,

    compressor: Compressor,

    // Inflate band-split (LF/MF/HF, Linkwitz-Riley, sums flat)
    xo_inflate_lo_l: LR2Crossover,
    xo_inflate_lo_r: LR2Crossover,
    xo_inflate_hi_l: LR2Crossover,
    xo_inflate_hi_r: LR2Crossover,

    // Crossover analysis (for GUI visualizer)
    xo_bass_mid_l: LR2Crossover,
    xo_bass_mid_r: LR2Crossover,
    xo_low_bass_l: LR2Crossover,
    xo_low_bass_r: LR2Crossover,
    xo_mid_high_l: LR2Crossover,
    xo_mid_high_r: LR2Crossover,
    xo_highmid_high_l: LR2Crossover,
    xo_highmid_high_r: LR2Crossover,

    // Smoothed states
    correlation_decay_coef: f32,
    smoothed_band_power: [f32; 5],
    corr_avg_lr: f32,
    corr_avg_l2: f32,
    corr_avg_r2: f32,
    peak_hold_value: f32,
    peak_hold_l_value: f32,
    peak_hold_r_value: f32,

    // AUTO LOUD
    auto_loud_in: AutoLoudMeter,
    auto_loud_pre_sat: AutoLoudMeter,
    auto_loud_out: AutoLoudMeter,
    pre_sat_buf_l: Vec<f32>,
    pre_sat_buf_r: Vec<f32>,

    // Goniometer
    scope_vis_envelope: f32,

    // FFT
    fft_planner: RealFftPlanner<f32>,
    fft_input: Vec<f32>,
    fft_write_pos: usize,
    fft_hann: Vec<f32>,
    fft_windowed: Vec<f32>,
    fft_output_cache: Vec<realfft::num_complex::Complex<f32>>,

    // SNAP
    snap_fft: SnapFFT,

    // Dirty-flag caches
    cached_hpf_freq: f32,
    cached_lpf_freq: f32,
    cached_cut_slope: i64,
    cached_bass_gain: f32,
    cached_bass_slope: i64,
    cached_lo_mid_gain: f32,
    cached_lo_mid_slope: i64,
    cached_mid_gain: f32,
    cached_mid_slope: i64,
    cached_high_gain: f32,
    cached_high_slope: i64,
    cached_excite_gain: f32,
    cached_excite_slope: i64,
    cached_eq_freq_1: f32,
    cached_eq_freq_2: f32,
    cached_eq_freq_3: f32,
    cached_eq_freq_4: f32,
    cached_eq_freq_5: f32,
    cached_tilt_gain: f32,
    cached_excite_freq: f32,
    cached_sample_rate: f32,
}

impl Default for MeridianDspState {
    fn default() -> Self {
        let fft_size = SPECTRUM_BINS * 2;
        Self {
            hpf_l: Biquad::new(),
            hpf_r: Biquad::new(),
            lpf_l: Biquad::new(),
            lpf_r: Biquad::new(),
            hpf2_l: Biquad::new(),
            hpf2_r: Biquad::new(),
            lpf2_l: Biquad::new(),
            lpf2_r: Biquad::new(),
            bass_l: Biquad::new(),
            bass_r: Biquad::new(),
            lo_mid_l: Biquad::new(),
            lo_mid_r: Biquad::new(),
            mid_l: Biquad::new(),
            mid_r: Biquad::new(),
            high_l: Biquad::new(),
            high_r: Biquad::new(),
            excite_l: Biquad::new(),
            excite_r: Biquad::new(),
            tilt_l: TiltEq::new(),
            tilt_r: TiltEq::new(),
            excite_hp_l: Biquad::new(),
            excite_hp_r: Biquad::new(),
            compressor: Compressor::new(),
            xo_inflate_lo_l: LR2Crossover::new(),
            xo_inflate_lo_r: LR2Crossover::new(),
            xo_inflate_hi_l: LR2Crossover::new(),
            xo_inflate_hi_r: LR2Crossover::new(),
            xo_bass_mid_l: LR2Crossover::new(),
            xo_bass_mid_r: LR2Crossover::new(),
            xo_low_bass_l: LR2Crossover::new(),
            xo_low_bass_r: LR2Crossover::new(),
            xo_mid_high_l: LR2Crossover::new(),
            xo_mid_high_r: LR2Crossover::new(),
            xo_highmid_high_l: LR2Crossover::new(),
            xo_highmid_high_r: LR2Crossover::new(),
            correlation_decay_coef: 0.005,
            smoothed_band_power: [0.0; 5],
            corr_avg_lr: 0.0,
            corr_avg_l2: 0.0,
            corr_avg_r2: 0.0,
            peak_hold_value: -90.0,
            peak_hold_l_value: -90.0,
            peak_hold_r_value: -90.0,
            auto_loud_in: AutoLoudMeter::new(44100.0),
            auto_loud_pre_sat: AutoLoudMeter::new(44100.0),
            auto_loud_out: AutoLoudMeter::new(44100.0),
            pre_sat_buf_l: Vec::new(),
            pre_sat_buf_r: Vec::new(),
            scope_vis_envelope: 1e-4,
            fft_planner: RealFftPlanner::new(),
            fft_input: vec![0.0; fft_size],
            fft_write_pos: 0,
            fft_hann: (0..fft_size)
                .map(|i| {
                    let n = fft_size;
                    0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos())
                })
                .collect(),
            fft_windowed: vec![0.0; fft_size],
            fft_output_cache: {
                let mut planner = RealFftPlanner::new();
                let fwd = planner.plan_fft_forward(fft_size);
                fwd.make_output_vec()
            },
            snap_fft: SnapFFT::new(),
            // -999 sentinels: must not match param defaults or dirty-flag coef
            // updates skip and Biquad::default() (zero coefs) silences output.
            cached_hpf_freq: -999.0,
            cached_lpf_freq: -999.0,
            cached_cut_slope: -999,
            cached_bass_gain: -999.0,
            cached_bass_slope: -999,
            cached_lo_mid_gain: -999.0,
            cached_lo_mid_slope: -999,
            cached_mid_gain: -999.0,
            cached_mid_slope: -999,
            cached_high_gain: -999.0,
            cached_high_slope: -999,
            cached_excite_gain: -999.0,
            cached_excite_slope: -999,
            cached_eq_freq_1: -999.0,
            cached_eq_freq_2: -999.0,
            cached_eq_freq_3: -999.0,
            cached_eq_freq_4: -999.0,
            cached_eq_freq_5: -999.0,
            cached_tilt_gain: -999.0,
            cached_excite_freq: -999.0,
            cached_sample_rate: -999.0,
        }
    }
}




// ─── PluginLogic ─────────────────────────────────────────────────────────────

impl PluginLogic for Meridian {
    type Params = MeridianParams;
    type DspState = MeridianDspState;

    fn bus_layouts() -> Vec<BusLayout> {
        vec![BusLayout::stereo()]
    }

    fn reset(state: &mut MeridianDspState, params: &MeridianParams, config: &AudioConfig) {
        let sr = config.sample_rate as f32;
        let sr = sr as f32;

        state.compressor.set_sample_rate(sr);

        // ponytail: seed all biquads at reset — 6.1.2 migration zeroed dirty-flag
        // caches so filters matching defaults (esp. tilt @ 0 dB) never got coefs.
        let hpf_f = params.hpf_freq.raw_target() as f32;
        let lpf_f = params.lpf_freq.raw_target() as f32;
        let cut_slope_val = params.cut_slope.value();
        const Q1: f32 = 0.541_196_1;
        const Q2: f32 = 1.306_563;
        if cut_slope_val >= 1 {
            state.hpf_l.set_butterworth_hp_q(hpf_f, Q1, sr);
            state.hpf_r.set_butterworth_hp_q(hpf_f, Q1, sr);
            state.hpf2_l.set_butterworth_hp_q(hpf_f, Q2, sr);
            state.hpf2_r.set_butterworth_hp_q(hpf_f, Q2, sr);
            state.lpf_l.set_butterworth_lp_q(lpf_f, Q1, sr);
            state.lpf_r.set_butterworth_lp_q(lpf_f, Q1, sr);
            state.lpf2_l.set_butterworth_lp_q(lpf_f, Q2, sr);
            state.lpf2_r.set_butterworth_lp_q(lpf_f, Q2, sr);
        } else {
            state.hpf_l.set_butterworth_hp(hpf_f, sr);
            state.hpf_r.set_butterworth_hp(hpf_f, sr);
            state.lpf_l.set_butterworth_lp(lpf_f, sr);
            state.lpf_r.set_butterworth_lp(lpf_f, sr);
            state.hpf2_l.set_identity();
            state.hpf2_r.set_identity();
            state.lpf2_l.set_identity();
            state.lpf2_r.set_identity();
        }

        let slope_val = |slope_idx: i64| -> f32 {
            match slope_idx {
                0 => 0.5,
                1 => 1.0,
                _ => 2.0,
            }
        };
        let q_val = |slope_idx: i64| -> f32 {
            match slope_idx {
                0 => 0.4,
                1 => 0.7,
                _ => 1.5,
            }
        };

        let eq_f1 = params.eq_freq_1.raw_target() as f32;
        let eq_f2 = params.eq_freq_2.raw_target() as f32;
        let eq_f3 = params.eq_freq_3.raw_target() as f32;
        let eq_f4 = params.eq_freq_4.raw_target() as f32;
        let eq_f5 = params.eq_freq_5.raw_target() as f32;
        let bass_gain = params.bass_gain.raw_target() as f32;
        let lo_mid_gain = params.lo_mid_gain.raw_target() as f32;
        let mid_gain = params.mid_gain.raw_target() as f32;
        let high_gain = params.high_gain.raw_target() as f32;
        let excite_gain = params.excite_gain.raw_target() as f32;
        let tilt_db = params.tilt_gain.raw_target() as f32;
        let excite_freq = params.excite_freq.raw_target() as f32;

        state.bass_l
            .set_low_shelf(eq_f1, bass_gain, slope_val(params.bass_slope.value()), sr);
        state.bass_r
            .set_low_shelf(eq_f1, bass_gain, slope_val(params.bass_slope.value()), sr);
        state.lo_mid_l.set_peaking_eq(
            eq_f2,
            lo_mid_gain,
            q_val(params.lo_mid_slope.value()),
            sr,
        );
        state.lo_mid_r.set_peaking_eq(
            eq_f2,
            lo_mid_gain,
            q_val(params.lo_mid_slope.value()),
            sr,
        );
        state.mid_l
            .set_peaking_eq(eq_f3, mid_gain, q_val(params.mid_slope.value()), sr);
        state.mid_r
            .set_peaking_eq(eq_f3, mid_gain, q_val(params.mid_slope.value()), sr);
        state.high_l.set_peaking_eq(
            eq_f4,
            high_gain,
            q_val(params.high_slope.value()),
            sr,
        );
        state.high_r.set_peaking_eq(
            eq_f4,
            high_gain,
            q_val(params.high_slope.value()),
            sr,
        );
        state.excite_l.set_high_shelf(
            eq_f5,
            excite_gain,
            slope_val(params.excite_slope.value()),
            sr,
        );
        state.excite_r.set_high_shelf(
            eq_f5,
            excite_gain,
            slope_val(params.excite_slope.value()),
            sr,
        );
        state.tilt_l.set(1000.0, tilt_db, sr);
        state.tilt_r.set(1000.0, tilt_db, sr);
        state.excite_hp_l.set_butterworth_hp(excite_freq, sr);
        state.excite_hp_r.set_butterworth_hp(excite_freq, sr);

        state.xo_inflate_lo_l.set_cutoff(300.0, sr);
        state.xo_inflate_lo_r.set_cutoff(300.0, sr);
        state.xo_inflate_hi_l.set_cutoff(3000.0, sr);
        state.xo_inflate_hi_r.set_cutoff(3000.0, sr);

        // Recreate Auto-Loud meters at host sample rate
        state.auto_loud_in = AutoLoudMeter::new(sr);
        state.auto_loud_pre_sat = AutoLoudMeter::new(sr);
        state.auto_loud_out = AutoLoudMeter::new(sr);

        // Crossover frequencies (constant for GUI visualizer)
        for (xo_l, xo_r, fc) in [
            (&mut state.xo_bass_mid_l, &mut state.xo_bass_mid_r, 400.0),
            (&mut state.xo_low_bass_l, &mut state.xo_low_bass_r, 100.0),
            (&mut state.xo_mid_high_l, &mut state.xo_mid_high_r, 1500.0),
            (
                &mut state.xo_highmid_high_l,
                &mut state.xo_highmid_high_r,
                8000.0,
            ),
        ] {
            xo_l.set_cutoff(fc, sr);
            xo_r.set_cutoff(fc, sr);
        }

        state.correlation_decay_coef = 1.0 - (-1.0 / (0.1 * sr)).exp();
        state.cached_sample_rate = sr;

        params
            .shared
            .sample_rate
            .store(sr, std::sync::atomic::Ordering::Release);
    }

    fn process(
        state: &mut MeridianDspState,
        params: &MeridianParams,
        buffer: &mut AudioBuffer,
        _events: &EventList,
        _ctx: &mut ProcessContext,
    ) -> ProcessStatus {
        let _ftz = FtzDazGuard::new();

        if buffer.num_input_channels() < 2 {
            return ProcessStatus::Normal;
        }

        let bypass = params.bypass_active.value();

        // Reset analysis
        if params
            .shared
            .reset_analysis
            .swap(false, Ordering::Acquire)
        {
            state.fft_input.fill(0.0);
            state.fft_write_pos = 0;
            if let Ok(mut avg) = params.shared.spectrum_avg.try_lock() {
                avg.fill(-90.0);
            }
            if let Ok(mut bins) = params.shared.spectrum_bins.try_lock() {
                bins.fill(-90.0);
            }
            state.hpf_l.reset();
            state.hpf_r.reset();
            state.lpf_l.reset();
            state.lpf_r.reset();
            state.hpf2_l.reset();
            state.hpf2_r.reset();
            state.lpf2_l.reset();
            state.lpf2_r.reset();
            state.bass_l.reset();
            state.bass_r.reset();
            state.lo_mid_l.reset();
            state.lo_mid_r.reset();
            state.mid_l.reset();
            state.mid_r.reset();
            state.high_l.reset();
            state.high_r.reset();
            state.excite_l.reset();
            state.excite_r.reset();
            state.tilt_l.reset();
            state.tilt_r.reset();
            state.excite_hp_l.reset();
            state.excite_hp_r.reset();
            state.xo_inflate_lo_l.reset();
            state.xo_inflate_lo_r.reset();
            state.xo_inflate_hi_l.reset();
            state.xo_inflate_hi_r.reset();
            state.xo_bass_mid_l.reset();
            state.xo_bass_mid_r.reset();
            state.xo_low_bass_l.reset();
            state.xo_low_bass_r.reset();
            state.xo_mid_high_l.reset();
            state.xo_mid_high_r.reset();
            state.xo_highmid_high_l.reset();
            state.xo_highmid_high_r.reset();
        }

        let sample_rate = params.shared.sample_rate.load(Ordering::Acquire);
        state.compressor.set_sample_rate(sample_rate);

        // Dirty-flag coefficient update
        let hpf_f = params.hpf_freq.raw_target() as f32;
        let lpf_f = params.lpf_freq.raw_target() as f32;
        let cut_slope_val = params.cut_slope.value();
        let bass_gain_val = params.bass_gain.raw_target() as f32;
        let bass_slope_val = params.bass_slope.value();
        let lo_mid_gain_val = params.lo_mid_gain.raw_target() as f32;
        let lo_mid_slope_val = params.lo_mid_slope.value();
        let mid_gain_val = params.mid_gain.raw_target() as f32;
        let mid_slope_val = params.mid_slope.value();
        let high_gain_val = params.high_gain.raw_target() as f32;
        let high_slope_val = params.high_slope.value();
        let excite_gain_val = params.excite_gain.raw_target() as f32;
        let excite_slope_val = params.excite_slope.value();
        let eq_f1 = params.eq_freq_1.raw_target() as f32;
        let eq_f2 = params.eq_freq_2.raw_target() as f32;
        let eq_f3 = params.eq_freq_3.raw_target() as f32;
        let eq_f4 = params.eq_freq_4.raw_target() as f32;
        let eq_f5 = params.eq_freq_5.raw_target() as f32;
        let tilt_db = params.tilt_gain.raw_target() as f32;
        let excite_freq = params.excite_freq.raw_target() as f32;

        let slope_val = |slope_idx: i64| -> f32 {
            match slope_idx {
                0 => 0.5,
                1 => 1.0,
                _ => 2.0,
            }
        };
        let q_val = |slope_idx: i64| -> f32 {
            match slope_idx {
                0 => 0.4,
                1 => 0.7,
                _ => 1.5,
            }
        };

        let coef_dirty = sample_rate != state.cached_sample_rate;
        state.cached_sample_rate = sample_rate;

        if hpf_f != state.cached_hpf_freq
            || lpf_f != state.cached_lpf_freq
            || cut_slope_val != state.cached_cut_slope
            || coef_dirty
        {
            // Safety: reset filter state on >4 octave jump (right-click reset fix)
            let hpf_jump = if state.cached_hpf_freq > 1.0 && hpf_f > 1.0 {
                (hpf_f / state.cached_hpf_freq).max(state.cached_hpf_freq / hpf_f)
            } else if hpf_f != state.cached_hpf_freq {
                4.1
            } else {
                1.0
            };
            let lpf_jump = if state.cached_lpf_freq > 1.0 && lpf_f > 1.0 {
                (lpf_f / state.cached_lpf_freq).max(state.cached_lpf_freq / lpf_f)
            } else if lpf_f != state.cached_lpf_freq {
                4.1
            } else {
                1.0
            };
            if hpf_jump > 4.0 {
                state.hpf_l.reset();
                state.hpf_r.reset();
                state.hpf2_l.reset();
                state.hpf2_r.reset();
            }
            if lpf_jump > 4.0 {
                state.lpf_l.reset();
                state.lpf_r.reset();
                state.lpf2_l.reset();
                state.lpf2_r.reset();
            }
            state.cached_hpf_freq = hpf_f;
            state.cached_lpf_freq = lpf_f;
            state.cached_cut_slope = cut_slope_val;
            const Q1: f32 = 0.541_196_1;
            const Q2: f32 = 1.306_563;
            if cut_slope_val >= 1 {
                state.hpf_l.set_butterworth_hp_q(hpf_f, Q1, sample_rate);
                state.hpf_r.set_butterworth_hp_q(hpf_f, Q1, sample_rate);
                state.hpf2_l.set_butterworth_hp_q(hpf_f, Q2, sample_rate);
                state.hpf2_r.set_butterworth_hp_q(hpf_f, Q2, sample_rate);
                state.lpf_l.set_butterworth_lp_q(lpf_f, Q1, sample_rate);
                state.lpf_r.set_butterworth_lp_q(lpf_f, Q1, sample_rate);
                state.lpf2_l.set_butterworth_lp_q(lpf_f, Q2, sample_rate);
                state.lpf2_r.set_butterworth_lp_q(lpf_f, Q2, sample_rate);
            } else {
                state.hpf_l.set_butterworth_hp(hpf_f, sample_rate);
                state.hpf_r.set_butterworth_hp(hpf_f, sample_rate);
                state.lpf_l.set_butterworth_lp(lpf_f, sample_rate);
                state.lpf_r.set_butterworth_lp(lpf_f, sample_rate);
                state.hpf2_l.set_identity();
                state.hpf2_r.set_identity();
                state.lpf2_l.set_identity();
                state.lpf2_r.set_identity();
            }
        }

        if bass_gain_val != state.cached_bass_gain
            || bass_slope_val != state.cached_bass_slope
            || eq_f1 != state.cached_eq_freq_1
            || coef_dirty
        {
            state.cached_bass_gain = bass_gain_val;
            state.cached_bass_slope = bass_slope_val;
            state.cached_eq_freq_1 = eq_f1;
            let bass_slope = slope_val(bass_slope_val);
            state.bass_l
                .set_low_shelf(eq_f1, bass_gain_val, bass_slope, sample_rate);
            state.bass_r
                .set_low_shelf(eq_f1, bass_gain_val, bass_slope, sample_rate);
        }

        if lo_mid_gain_val != state.cached_lo_mid_gain
            || lo_mid_slope_val != state.cached_lo_mid_slope
            || eq_f2 != state.cached_eq_freq_2
            || coef_dirty
        {
            state.cached_lo_mid_gain = lo_mid_gain_val;
            state.cached_lo_mid_slope = lo_mid_slope_val;
            state.cached_eq_freq_2 = eq_f2;
            let lo_mid_q = q_val(lo_mid_slope_val);
            state.lo_mid_l
                .set_peaking_eq(eq_f2, lo_mid_gain_val, lo_mid_q, sample_rate);
            state.lo_mid_r
                .set_peaking_eq(eq_f2, lo_mid_gain_val, lo_mid_q, sample_rate);
        }

        if mid_gain_val != state.cached_mid_gain
            || mid_slope_val != state.cached_mid_slope
            || eq_f3 != state.cached_eq_freq_3
            || coef_dirty
        {
            state.cached_mid_gain = mid_gain_val;
            state.cached_mid_slope = mid_slope_val;
            state.cached_eq_freq_3 = eq_f3;
            let mid_q = q_val(mid_slope_val);
            state.mid_l
                .set_peaking_eq(eq_f3, mid_gain_val, mid_q, sample_rate);
            state.mid_r
                .set_peaking_eq(eq_f3, mid_gain_val, mid_q, sample_rate);
        }

        if high_gain_val != state.cached_high_gain
            || high_slope_val != state.cached_high_slope
            || eq_f4 != state.cached_eq_freq_4
            || coef_dirty
        {
            state.cached_high_gain = high_gain_val;
            state.cached_high_slope = high_slope_val;
            state.cached_eq_freq_4 = eq_f4;
            let high_q = q_val(high_slope_val);
            state.high_l
                .set_peaking_eq(eq_f4, high_gain_val, high_q, sample_rate);
            state.high_r
                .set_peaking_eq(eq_f4, high_gain_val, high_q, sample_rate);
        }

        if excite_gain_val != state.cached_excite_gain
            || excite_slope_val != state.cached_excite_slope
            || eq_f5 != state.cached_eq_freq_5
            || coef_dirty
        {
            state.cached_excite_gain = excite_gain_val;
            state.cached_excite_slope = excite_slope_val;
            state.cached_eq_freq_5 = eq_f5;
            let excite_slope = slope_val(excite_slope_val);
            state.excite_l
                .set_high_shelf(eq_f5, excite_gain_val, excite_slope, sample_rate);
            state.excite_r
                .set_high_shelf(eq_f5, excite_gain_val, excite_slope, sample_rate);
        }

        if tilt_db != state.cached_tilt_gain || coef_dirty {
            state.cached_tilt_gain = tilt_db;
            state.tilt_l.set(1000.0, tilt_db, sample_rate);
            state.tilt_r.set(1000.0, tilt_db, sample_rate);
        }

        if excite_freq != state.cached_excite_freq || coef_dirty {
            state.cached_excite_freq = excite_freq;
            state.excite_hp_l
                .set_butterworth_hp(excite_freq, sample_rate);
            state.excite_hp_r
                .set_butterworth_hp(excite_freq, sample_rate);
        }

        // Reset peak
        if params.shared.reset_peak.swap(false, Ordering::Release) {
            state.peak_hold_value = -90.0;
            state.peak_hold_l_value = -90.0;
            state.peak_hold_r_value = -90.0;
        }

        // Smoothed parameter values (per-block reads via value(), truce pattern)
        let warmth_drive_db = params.warmth_drive.value();
        let warmth_mix_pct = params.warmth_mix.value();
        let excite_amt = params.excite_amount.value();
        let excite_blend = params.excite_blend.value();
        let comp_t = params.comp_threshold.value();
        let comp_m = params.comp_mix.value();
        let comp_att = params.comp_attack.value();
        let comp_rel = params.comp_release.value();
        let ratio = params.comp_character.value();
        let knee = (1.0 - (ratio - 1.5) / 2.5) * 6.0;
        let comp_makeup_gain = db_to_gain(params.comp_makeup.value());

        let inflate_effect = params.inflate_effect.value() / 100.0;
        let inflate_curve = params.inflate_curve.value();
        let inflate_band_split = params.inflate_band_split.value();
        let inflate_clip = params.inflate_clip.value();

        let width = params.stereo_width.value() / 100.0;
        let pan = params.pan.value();
        let out_gain = db_to_gain(params.output_gain.value());

        let mut snap_phase = params.shared.snap_phase.load(Ordering::Acquire);
        let mono = match snap_phase {
            2 => true,
            3 => false,
            _ => params.mono_active.value(),
        };
        let delta = match snap_phase {
            3 => true,
            _ => params.delta_active.value(),
        };

        let mut max_out_peak = 0.0f32;
        let mut max_out_peak_l = 0.0f32;
        let mut max_out_peak_r = 0.0f32;
        let mut count_samples: usize = 0;

        let mut block_band_power = [0.0f32; 5];

        if buffer.num_samples() == 0 {
            return ProcessStatus::Normal;
        }
        let num_samples = buffer.num_samples();

        let mut gr_db = 0.0f32;
        let mut max_gr_db = 0.0f32;
        let is_measuring = params
            .shared
            .auto_loud_measuring
            .load(Ordering::Acquire);

        // Feed input LUFS
        if is_measuring {
            state.auto_loud_in.feed(buffer.input(0), buffer.input(1));
            state.pre_sat_buf_l.clear();
            state.pre_sat_buf_r.clear();
        }

        // Raw pointer setup for output
        let (out0_ptr, out1_ptr): (*mut f32, *mut f32);
        {
            let (_, out0) = buffer.io(0);
            out0_ptr = out0.as_mut_ptr();
        }
        {
            let out1_slice = buffer.output(1);
            out1_ptr = out1_slice.as_mut_ptr();
        }
        #[allow(unsafe_code)]
        let (out0, out1): (&mut [f32], &mut [f32]) = unsafe {
            (
                std::slice::from_raw_parts_mut(out0_ptr, num_samples),
                std::slice::from_raw_parts_mut(out1_ptr, num_samples),
            )
        };

        for i in 0..num_samples {
            count_samples += 1;
            let in_l = buffer.input(0)[i];
            let in_r = buffer.input(1)[i];

            // HPF & LPF
            let mut x_l = state.lpf2_l.process(
                state.lpf_l
                    .process(state.hpf2_l.process(state.hpf_l.process(in_l))),
            );
            let mut x_r = state.lpf2_r.process(
                state.lpf_r
                    .process(state.hpf2_r.process(state.hpf_r.process(in_r))),
            );

            // Series EQ
            x_l = state.excite_l.process(
                state.high_l.process(
                    state.mid_l
                        .process(state.lo_mid_l.process(state.bass_l.process(x_l))),
                ),
            );
            x_r = state.excite_r.process(
                state.high_r.process(
                    state.mid_r
                        .process(state.lo_mid_r.process(state.bass_r.process(x_r))),
                ),
            );

            // Tilt
            x_l = state.tilt_l.process(x_l);
            x_r = state.tilt_r.process(x_r);

            // Exciter
            if excite_amt > 0.0 || excite_blend > 0.0 {
                let high_l = state.excite_hp_l.process(x_l);
                let high_r = state.excite_hp_r.process(x_r);
                let drive = 1.0 + (excite_amt / 30.0) * 59.0;
                let sat_high_l = soft_clip(high_l * drive);
                let sat_high_r = soft_clip(high_r * drive);
                let blend = excite_blend / 100.0;
                x_l += (sat_high_l - high_l) * blend;
                x_r += (sat_high_r - high_r) * blend;
            }

            // Compressor
            let (mut comp_l, mut comp_r) = state.compressor.process(
                x_l, x_r, comp_t, comp_m, comp_att, comp_rel, ratio, knee, &mut gr_db,
            );
            max_gr_db = max_gr_db.max(gr_db);
            comp_l *= comp_makeup_gain;
            comp_r *= comp_makeup_gain;

            // Pre-sat LUFS
            if is_measuring {
                state.pre_sat_buf_l.push(comp_l);
                state.pre_sat_buf_r.push(comp_r);
            }

            // Warmth
            if warmth_drive_db > 0.0 || warmth_mix_pct > 0.0 {
                let drive = db_to_gain(warmth_drive_db);
                let wet_l = tube_warm(comp_l * drive) / drive;
                let wet_r = tube_warm(comp_r * drive) / drive;
                let mix = warmth_mix_pct / 100.0;
                comp_l = comp_l * (1.0 - mix) + wet_l * mix;
                comp_r = comp_r * (1.0 - mix) + wet_r * mix;
            }

            // Inflate (Oxford-Inflator-inspired loudness/density waveshaper)
            if inflate_effect > 0.0 {
                let shape_one = |v: f32| -> f32 {
                    let v = if inflate_clip {
                        v.clamp(-1.0, 1.0)
                    } else {
                        v.clamp(-2.0, 2.0)
                    };
                    inflate_shape(v, inflate_curve)
                };
                let (wet_l, wet_r) = if inflate_band_split {
                    let (lo_l, hi_l) = state.xo_inflate_lo_l.process(comp_l);
                    let (mid_l, top_l) = state.xo_inflate_hi_l.process(hi_l);
                    let (lo_r, hi_r) = state.xo_inflate_lo_r.process(comp_r);
                    let (mid_r, top_r) = state.xo_inflate_hi_r.process(hi_r);
                    (
                        shape_one(lo_l) + shape_one(mid_l) + shape_one(top_l),
                        shape_one(lo_r) + shape_one(mid_r) + shape_one(top_r),
                    )
                } else {
                    (shape_one(comp_l), shape_one(comp_r))
                };
                comp_l = comp_l * (1.0 - inflate_effect) + wet_l * inflate_effect;
                comp_r = comp_r * (1.0 - inflate_effect) + wet_r * inflate_effect;
            }

            // Pan
            let pan_val = pan.clamp(-1.0, 1.0);
            let pan_angle = (pan_val + 1.0) * FRAC_PI_4;
            let raw_l = pan_angle.cos();
            let raw_r = pan_angle.sin();
            let max_raw = raw_l.max(raw_r);
            let pan_norm = if max_raw > 0.001 { 1.0 / max_raw } else { 1.0 };
            let pan_l = raw_l * pan_norm;
            let pan_r = raw_r * pan_norm;
            let mut out_l = comp_l * pan_l;
            let mut out_r = comp_r * pan_r;

            // Stereo Width
            let w = width.clamp(0.0, 2.0);
            let a = 0.5 * (1.0 + w);
            let b = 0.5 * (1.0 - w);
            let width_l = out_l * a + out_r * b;
            let width_r = out_r * a + out_l * b;
            let width_norm = 1.0 / (1.0 + (w - 1.0).max(0.0) * 0.20);
            out_l = width_l * width_norm;
            out_r = width_r * width_norm;

            // Mono
            if mono {
                let m = (out_l + out_r) * 0.5;
                out_l = m;
                out_r = m;
            }

            let mut processed_l = out_l;
            let mut processed_r = out_r;

            // Delta
            if delta {
                processed_l = out_l - in_l;
                processed_r = out_r - in_r;
            }

            processed_l *= out_gain;
            processed_r *= out_gain;

            // Safety clamp
            processed_l = processed_l.clamp(-1.0, 1.0);
            processed_r = processed_r.clamp(-1.0, 1.0);

            // Crossover analysis for visualizer
            let (low_group_l, high_group_l) = state.xo_bass_mid_l.process(processed_l);
            let (band1_l, band2_l) = state.xo_low_bass_l.process(low_group_l);
            let (mid_group_l, super_high_group_l) = state.xo_mid_high_l.process(high_group_l);
            let (band3_l, band4_l) = (mid_group_l, super_high_group_l);
            let (band4_l_split, band5_l) = state.xo_highmid_high_l.process(band4_l);
            let band4_l = band4_l_split;

            let (low_group_r, high_group_r) = state.xo_bass_mid_r.process(processed_r);
            let (band1_r, band2_r) = state.xo_low_bass_r.process(low_group_r);
            let (mid_group_r, super_high_group_r) = state.xo_mid_high_r.process(high_group_r);
            let (band3_r, band4_r) = (mid_group_r, super_high_group_r);
            let (band4_r_split, band5_r) = state.xo_highmid_high_r.process(band4_r);
            let band4_r = band4_r_split;

            let bands_l = [band1_l, band2_l, band3_l, band4_l, band5_l];
            let bands_r = [band1_r, band2_r, band3_r, band4_r, band5_r];
            for b in 0..5 {
                let band_power = (bands_l[b] * bands_l[b] + bands_r[b] * bands_r[b]) * 0.5;
                block_band_power[b] += band_power;
            }

            let (meter_l, meter_r) = if bypass {
                out0[i] = in_l;
                out1[i] = in_r;
                (in_l, in_r)
            } else {
                max_out_peak = max_out_peak.max(processed_l.abs()).max(processed_r.abs());
                max_out_peak_l = max_out_peak_l.max(processed_l.abs());
                max_out_peak_r = max_out_peak_r.max(processed_r.abs());
                out0[i] = processed_l;
                out1[i] = processed_r;
                (processed_l, processed_r)
            };

            let corr_lr = meter_l * meter_r;
            let corr_l2 = meter_l * meter_l;
            let corr_r2 = meter_r * meter_r;
            state.corr_avg_lr = (1.0 - state.correlation_decay_coef) * state.corr_avg_lr
                + state.correlation_decay_coef * corr_lr;
            state.corr_avg_l2 = (1.0 - state.correlation_decay_coef) * state.corr_avg_l2
                + state.correlation_decay_coef * corr_l2;
            state.corr_avg_r2 = (1.0 - state.correlation_decay_coef) * state.corr_avg_r2
                + state.correlation_decay_coef * corr_r2;
        }

        // Gain reduction
        params
            .shared
            .gain_reduction
            .store(max_gr_db, Ordering::Release);

        // Smoothed band levels
        let sample_weight = 1.0 / count_samples as f32;
        let buf_coef = 1.0 - (-(num_samples as f32) / (0.1 * sample_rate)).exp();
        for (b, &band_power) in block_band_power.iter().enumerate() {
            let average_band_power = band_power * sample_weight;
            state.smoothed_band_power[b] =
                (1.0 - buf_coef) * state.smoothed_band_power[b] + buf_coef * average_band_power;
            let band_db = gain_to_db(state.smoothed_band_power[b].sqrt());
            params.shared.band_levels[b].store(band_db, Ordering::Release);
        }

        // Correlation
        let denom = (state.corr_avg_l2 * state.corr_avg_r2).sqrt();
        let corr = if denom > 1e-6 {
            state.corr_avg_lr / denom
        } else {
            1.0
        };
        params
            .shared
            .phase_correlation
            .store(corr, Ordering::Release);

        // Peak meters
        let peak_db = gain_to_db(max_out_peak);
        let peak_l_db = gain_to_db(max_out_peak_l);
        let peak_r_db = gain_to_db(max_out_peak_r);
        params
            .shared
            .output_peak
            .store(peak_db, Ordering::Release);
        params
            .shared
            .output_peak_l
            .store(peak_l_db, Ordering::Release);
        params
            .shared
            .output_peak_r
            .store(peak_r_db, Ordering::Release);
        state.peak_hold_value = state.peak_hold_value.max(peak_db);
        state.peak_hold_l_value = state.peak_hold_l_value.max(peak_l_db);
        state.peak_hold_r_value = state.peak_hold_r_value.max(peak_r_db);
        params
            .shared
            .peak_hold
            .store(state.peak_hold_value, Ordering::Release);
        params
            .shared
            .peak_hold_l
            .store(state.peak_hold_l_value, Ordering::Release);
        params
            .shared
            .peak_hold_r
            .store(state.peak_hold_r_value, Ordering::Release);

        // Balance
        let rms_l = state.corr_avg_l2.sqrt();
        let rms_r = state.corr_avg_r2.sqrt();
        let sum_lr = rms_l + rms_r;
        let balance = if sum_lr > 1e-6 {
            (rms_l - rms_r) / sum_lr
        } else {
            0.0
        };
        params.shared.balance.store(balance, Ordering::Release);

        // FFT Spectrum
        {
            let n = num_samples;
            let fft_size = state.fft_input.len();
            for i in 0..n {
                state.fft_input[state.fft_write_pos] = (out0[i] + out1[i]) * 0.5;
                state.fft_write_pos += 1;
                if state.fft_write_pos >= fft_size {
                    let half = fft_size / 2;
                    for j in 0..half {
                        state.fft_input[j] = state.fft_input[j + half];
                    }
                    state.fft_write_pos = half;

                    for i in 0..fft_size {
                        state.fft_windowed[i] = state.fft_input[i] * state.fft_hann[i];
                    }
                    let fft = state.fft_planner.plan_fft_forward(fft_size);
                    fft.process(&mut state.fft_windowed, &mut state.fft_output_cache)
                        .ok();
                }
            }

            // Compute and write spectrum after each buffer
            if let Ok(mut spectrum_frame) = params.shared.spectrum_bins.try_lock() {
                shared_analysis::compute_spectrum_bins(
                    &state.fft_output_cache,
                    &mut spectrum_frame,
                    fft_size,
                    sample_rate,
                );
            }

            // Update spectrum_avg (EMA) from spectrum_bins
            if let Ok(mut avg) = params.shared.spectrum_avg.try_lock()
                && let Ok(bins) = params.shared.spectrum_bins.try_lock()
            {
                let n_bins = SPECTRUM_BINS;
                // Energy-gating: only update EMA if signal above -80 dB
                let frame_energy = bins.iter().map(|x| x * x).sum::<f32>() / n_bins as f32;
                let energy_db = 10.0 * frame_energy.log10().max(-40.0);
                let gate = energy_db > -80.0;

                if !gate {
                    for sample in state.fft_input.iter_mut() {
                        *sample = 0.0;
                    }
                }

                for k in 0..n_bins {
                    let freq = k as f32 * sample_rate / fft_size as f32;
                    let log_norm = ((freq.max(20.0).ln() - 20.0_f32.ln())
                        / (20000.0_f32.ln() - 20.0_f32.ln()))
                    .clamp(0.0, 1.0);
                    let alpha = 0.02 + (0.10 - 0.02) * log_norm;
                    let input = if gate { bins[k] } else { 0.0 };
                    avg[k] = avg[k] * (1.0 - alpha) + input * alpha;
                }
            }
        }

        // SNAP FFT
        if snap_phase > 0 {
            for i in 0..num_samples {
                let sample = match snap_phase {
                    1 | 2 => (out0[i] + out1[i]) * 0.5,
                    3 => {
                        let out_mono = (out0[i] + out1[i]) * 0.5;
                        let in_mono = (buffer.input(0)[i] + buffer.input(1)[i]) * 0.5;
                        out_mono - in_mono
                    }
                    _ => 0.0,
                };
                if state.snap_fft.push_sample(sample) {
                    let frame = state.snap_fft.compute_fft(sample_rate);
                    let threshold = if snap_phase == 2 || snap_phase == 3 {
                        30
                    } else {
                        60
                    };
                    if state.snap_fft.accumulate_snap(&frame, snap_phase, threshold) {
                        let mode = match snap_phase {
                            1 => SnapMode::Stereo,
                            2 => SnapMode::Mono,
                            _ => SnapMode::Delta,
                        };
                        let snapshot = state.snap_fft.read_snapshot(mode);
                        if let Ok(mut buf) = match mode {
                            SnapMode::Stereo => params.shared.snap_stereo_snap.try_lock(),
                            SnapMode::Mono => params.shared.snap_mono_snap.try_lock(),
                            SnapMode::Delta => params.shared.snap_delta_snap.try_lock(),
                        } {
                            buf.copy_from_slice(&snapshot);
                        }
                        let next_phase = if snap_phase < 3 { snap_phase + 1 } else { 0 };
                        if next_phase == 0 {
                            params
                                .shared
                                .snap_active
                                .store(false, Ordering::Release);
                        } else {
                            state.snap_fft.reset_snapshots();
                        }
                        params
                            .shared
                            .snap_phase
                            .store(next_phase, Ordering::Release);
                        snap_phase = next_phase;
                    }
                }
            }
        }

        // AUTO LOUD
        if params.shared.auto_loud_trigger.load(Ordering::Acquire) {
            params
                .shared
                .auto_loud_trigger
                .store(false, Ordering::Release);
            params
                .shared
                .auto_loud_measuring
                .store(true, Ordering::Release);
            state.auto_loud_in.reset();
            state.auto_loud_pre_sat.reset();
            state.auto_loud_out.reset();
        }
        if is_measuring {
            if !state.pre_sat_buf_l.is_empty() {
                state.auto_loud_pre_sat
                    .feed(&state.pre_sat_buf_l, &state.pre_sat_buf_r);
            }
            state.auto_loud_out.feed(out0, out1);
            let target_samples = (5.0 * sample_rate as f64) as u64;
            if state.auto_loud_out.sample_count() >= target_samples {
                let in_lufs = state.auto_loud_in.loudness_db();
                let _pre_lufs = state.auto_loud_pre_sat.loudness_db();
                let out_lufs = state.auto_loud_out.loudness_db();
                let out_tp = state.auto_loud_out.true_peak_db();
                let lufs_offset = in_lufs - out_lufs;
                let peak_limit = DBTP_CEILING - out_tp;
                let offset_clamped = lufs_offset.clamp(-24.0, peak_limit);
                params
                    .shared
                    .auto_loud_gain_offset
                    .store(offset_clamped, Ordering::Release);
                params
                    .shared
                    .auto_loud_measuring
                    .store(false, Ordering::Release);
            }
        }

        // Goniometer scope buffer
        {
            let start_pos = params.shared.scope_write_pos.load(Ordering::Acquire);
            if let Ok(mut scope) = params.shared.scope_samples.try_lock() {
                let buf_len = SCOPE_BUFFER_LEN;
                let n = num_samples.min(buf_len);
                let block_peak = (0..n)
                    .map(|i| out0[i].abs().max(out1[i].abs()))
                    .fold(0.0f32, f32::max)
                    .max(1e-9);
                let att = 1.0 - (-(n as f32) / (0.005 * sample_rate)).exp();
                let rel = 1.0 - (-(n as f32) / (0.300 * sample_rate)).exp();
                if block_peak > state.scope_vis_envelope {
                    state.scope_vis_envelope += att * (block_peak - state.scope_vis_envelope);
                } else {
                    state.scope_vis_envelope += rel * (block_peak - state.scope_vis_envelope);
                }
                let vis_gain = if state.scope_vis_envelope > 1e-5 {
                    (0.9 / state.scope_vis_envelope).min(20.0)
                } else {
                    0.0
                };
                for i in 0..n {
                    let pos = (start_pos + i) % buf_len;
                    scope[pos] = [out0[i] * vis_gain, out1[i] * vis_gain];
                }
                params
                    .shared
                    .scope_write_pos
                    .store((start_pos + n) % buf_len, Ordering::Release);
            }
        }

        ProcessStatus::Normal
    }

    fn snapshot_into(_state: &MeridianDspState, _buf: &mut Vec<u8>) -> bool {
        false
    }
    fn save_state(_state: &MeridianDspState) -> Vec<u8> {
        Vec::new()
    }
    fn load_state(_state: &mut MeridianDspState, _data: &[u8]) -> Result<(), StateLoadError> {
        // NicePlug legacy migration removed in 6.1.2: load_state no longer
        // has mutable access to params. Use migrate_state(ForeignState) if
        // legacy session recovery is needed later.
        Ok(())
    }
    fn state_changed(_state: &mut MeridianDspState, _params: &MeridianParams) {}

    fn editor(params: Arc<Self::Params>) -> Box<dyn Editor> {
        editor::build_editor(params)
    }
}

truce::plugin! { logic: Meridian, params: MeridianParams }

#[cfg(test)]
mod tests {
    use crate::Plugin;

    #[test]
    fn state_round_trips() {
        truce_test::assert_state_round_trip::<Plugin>();
    }
}
