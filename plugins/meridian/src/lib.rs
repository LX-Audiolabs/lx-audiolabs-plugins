#![allow(unsafe_op_in_unsafe_fn)]

// Meridian — Track and group shaper (AURA).
//
// 5-band EQ with slope control, soft-knee compressor, exciter, tube warmth,
// tilt EQ, stereo width/pan, and Auto Loud LUFS metering.
//
// Signal chain:
//   HPF/LPF → 5-band Series EQ → Tilt → Exciter → Compressor →
//   Warmth → Inflate → Pan → Stereo Width → Mono/Delta → Gain → clamp

use realfft::RealFftPlanner;
use std::sync::Arc;
use aura::prelude::*;

use aura_dsp::analysis::{SPECTRUM_BINS, SnapFFT};
use lx_analysis::product_shared::MeridianShared;
use aura_dsp::fx::{AutoLoudMeter, Biquad, Compressor, LR2Crossover, TiltEq};

mod editor;
mod presets;
mod process;

// Window size is defined in editor.rs for the Slint UI.

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

/// Soft clipping — odd harmonics (Exciter).
#[inline]
pub(crate) fn soft_clip(x: f32) -> f32 {
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
pub(crate) fn tube_warm(x: f32) -> f32 {
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
pub(crate) fn inflate_shape(x: f32, curve: f32) -> f32 {
    let t = (curve + 50.0) / 100.0; // -50..+50 -> 0..1
    let drive = 1.0 + t * t * 5.0; // quadratic: 0=1 (clean), 0.5=2.25 (gentle), 1=6 (fat/aggressive)
    let gain = 1.0 + t; // 1..2, small-signal gain matching Oxford's a1 range
    (x * drive).tanh() * gain / drive
}

// ─── Params ──────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct MeridianParams {
    // HPF / LPF
    #[param(id = 1, name = "Low Cut",
        default = 2.0,
        range = "log(2.0, 2000.0)",
        unit = "Hz",
        group = "Filter"
    )]
    pub hpf_freq: FloatParam,
    #[param(id = 2, name = "High Cut",
        default = 35000.0,
        range = "log(200.0, 35000.0)",
        unit = "Hz",
        group = "Filter"
    )]
    pub lpf_freq: FloatParam,
    #[param(id = 3, name = "Cut Slope",
        default = 0,
        range = "discrete(0, 1)",
        group = "Filter"
    )]
    pub cut_slope: IntParam,

    // Bass EQ shelf
    #[param(id = 4, name = "Lo Shelf Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Lo Shelf"
    )]
    pub bass_gain: FloatParam,
    #[param(id = 5, name = "Lo Shelf Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Lo Shelf"
    )]
    pub bass_slope: IntParam,

    // Lo-Mid EQ
    #[param(id = 6, name = "Lo-Mid Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Lo-Mid"
    )]
    pub lo_mid_gain: FloatParam,
    #[param(id = 7, name = "Lo-Mid Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Lo-Mid"
    )]
    pub lo_mid_slope: IntParam,

    // Mid EQ
    #[param(id = 8, name = "Mid Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Mid"
    )]
    pub mid_gain: FloatParam,
    #[param(id = 9, name = "Mid Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Mid"
    )]
    pub mid_slope: IntParam,

    // High EQ
    #[param(id = 10, name = "Hi-Mid Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Hi-Mid"
    )]
    pub high_gain: FloatParam,
    #[param(id = 11, name = "Hi-Mid Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Hi-Mid"
    )]
    pub high_slope: IntParam,

    // Excite (high shelf)
    #[param(id = 12, name = "Hi Shelf Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "EQ/Hi Shelf"
    )]
    pub excite_gain: FloatParam,
    #[param(id = 13, name = "Hi Shelf Slope",
        default = 1,
        range = "discrete(0, 2)",
        group = "EQ/Hi Shelf"
    )]
    pub excite_slope: IntParam,

    // EQ band frequencies
    #[param(id = 14, name = "Lo Shelf Freq",
        default = 80.0,
        range = "log(40.0, 200.0)",
        unit = "Hz",
        group = "EQ/Lo Shelf"
    )]
    pub eq_freq_1: FloatParam,
    #[param(id = 15, name = "Lo-Mid Freq",
        default = 300.0,
        range = "log(150.0, 800.0)",
        unit = "Hz",
        group = "EQ/Lo-Mid"
    )]
    pub eq_freq_2: FloatParam,
    #[param(id = 16, name = "Mid Freq",
        default = 1000.0,
        range = "log(500.0, 3000.0)",
        unit = "Hz",
        group = "EQ/Mid"
    )]
    pub eq_freq_3: FloatParam,
    #[param(id = 17, name = "Hi-Mid Freq",
        default = 4000.0,
        range = "log(2000.0, 10000.0)",
        unit = "Hz",
        group = "EQ/Hi-Mid"
    )]
    pub eq_freq_4: FloatParam,
    #[param(id = 18, name = "Hi Shelf Freq",
        default = 12000.0,
        range = "log(6000.0, 20000.0)",
        unit = "Hz",
        group = "EQ/Hi Shelf"
    )]
    pub eq_freq_5: FloatParam,

    // Tilt EQ
    #[param(id = 19, name = "Tilt",
        default = 0.0,
        range = "linear(-2.0, 2.0)",
        unit = "dB",
        group = "Tilt"
    )]
    pub tilt_gain: FloatParam,

    // Warmth (tube saturation)
    #[param(id = 20, name = "Warmth Drive",
        default = 0.0,
        range = "linear(0.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Saturator"
    )]
    pub warmth_drive: FloatParam,
    #[param(id = 21, name = "Warmth Mix",
        default = 0.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Saturator"
    )]
    pub warmth_mix: FloatParam,

    // Exciter (HF saturation)
    #[param(id = 22, name = "Excite Amount",
        default = 0.0,
        range = "linear(0.0, 30.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Exciter"
    )]
    pub excite_amount: FloatParam,
    #[param(id = 23, name = "Excite Blend",
        default = 0.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Exciter"
    )]
    pub excite_blend: FloatParam,
    #[param(id = 24, name = "Excite Freq",
        default = 8000.0,
        range = "log(6000.0, 12000.0)",
        unit = "Hz",
        group = "Exciter"
    )]
    pub excite_freq: FloatParam,

    // Compressor
    #[param(id = 25, name = "Comp Threshold",
        default = 0.0,
        range = "linear(-30.0, 0.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_threshold: FloatParam,
    #[param(id = 26, name = "Comp Mix",
        default = 0.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_mix: FloatParam,
    #[param(id = 27, name = "Comp Attack",
        default = 15.0,
        range = "linear(5.0, 50.0)",
        unit = "ms",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_attack: FloatParam,
    #[param(id = 28, name = "Comp Release",
        default = 120.0,
        range = "linear(50.0, 300.0)",
        unit = "ms",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_release: FloatParam,
    #[param(id = 29, name = "Comp Ratio",
        default = 2.0,
        range = "linear(1.5, 4.0)",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_character: FloatParam,
    #[param(id = 30, name = "Comp Makeup",
        default = 0.0,
        range = "linear(0.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Compressor"
    )]
    pub comp_makeup: FloatParam,

    // Inflate (Oxford-Inflator-inspired loudness/density waveshaper)
    #[param(id = 31, name = "Inflate Effect",
        default = 0.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Inflate"
    )]
    pub inflate_effect: FloatParam,
    #[param(id = 32, name = "Inflate Curve",
        default = 0.0,
        range = "linear(-50.0, 50.0)",
        smooth = "linear(20)",
        group = "Inflate"
    )]
    pub inflate_curve: FloatParam,
    #[param(id = 33, name = "Inflate Band Split", default = 0, group = "Inflate")]
    pub inflate_band_split: BoolParam,
    #[param(id = 34, name = "Inflate Clip", default = 0, group = "Inflate")]
    pub inflate_clip: BoolParam,

    // Stereo Width
    #[param(id = 35, name = "Stereo Width",
        default = 100.0,
        range = "linear(0.0, 200.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Stereo/Routing"
    )]
    pub stereo_width: FloatParam,
    // Pan
    #[param(id = 36, name = "Pan",
        default = 0.0,
        range = "linear(-1.0, 1.0)",
        smooth = "linear(20)",
        group = "Stereo/Routing"
    )]
    pub pan: FloatParam,
    // Output Gain
    #[param(id = 37, name = "Output Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Stereo/Routing"
    )]
    pub output_gain: FloatParam,

    // States
    #[param(id = 38, name = "Mono Sum", default = 0, group = "Stereo/Routing")]
    pub mono_active: BoolParam,
    #[param(id = 39, name = "Delta Diff", default = 0, group = "Stereo/Routing")]
    pub delta_active: BoolParam,
    #[param(id = 40, name = "Bypass", default = 0, group = "Stereo/Routing")]
    pub bypass_active: BoolParam,

    #[skip]
    pub shared: Arc<MeridianShared>,
}

impl MeridianParams {
    /// Real value display for `unit = "%"` params: our plain values are
    /// already the percent number (e.g. `100.0` means `100%`), not a
    /// 0.0-1.0 fraction. `aura_params::format_param_value`'s built-in
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
    pub(crate) hpf_l: Biquad,
    pub(crate) hpf_r: Biquad,
    pub(crate) lpf_l: Biquad,
    pub(crate) lpf_r: Biquad,
    pub(crate) hpf2_l: Biquad,
    pub(crate) hpf2_r: Biquad,
    pub(crate) lpf2_l: Biquad,
    pub(crate) lpf2_r: Biquad,

    // EQ bands
    pub(crate) bass_l: Biquad,
    pub(crate) bass_r: Biquad,
    pub(crate) lo_mid_l: Biquad,
    pub(crate) lo_mid_r: Biquad,
    pub(crate) mid_l: Biquad,
    pub(crate) mid_r: Biquad,
    pub(crate) high_l: Biquad,
    pub(crate) high_r: Biquad,
    pub(crate) excite_l: Biquad,
    pub(crate) excite_r: Biquad,

    pub(crate) tilt_l: TiltEq,
    pub(crate) tilt_r: TiltEq,

    pub(crate) excite_hp_l: Biquad,
    pub(crate) excite_hp_r: Biquad,

    pub(crate) compressor: Compressor,

    // Inflate band-split (LF/MF/HF, Linkwitz-Riley, sums flat)
    pub(crate) xo_inflate_lo_l: LR2Crossover,
    pub(crate) xo_inflate_lo_r: LR2Crossover,
    pub(crate) xo_inflate_hi_l: LR2Crossover,
    pub(crate) xo_inflate_hi_r: LR2Crossover,

    // Crossover analysis (for GUI visualizer)
    pub(crate) xo_bass_mid_l: LR2Crossover,
    pub(crate) xo_bass_mid_r: LR2Crossover,
    pub(crate) xo_low_bass_l: LR2Crossover,
    pub(crate) xo_low_bass_r: LR2Crossover,
    pub(crate) xo_mid_high_l: LR2Crossover,
    pub(crate) xo_mid_high_r: LR2Crossover,
    pub(crate) xo_highmid_high_l: LR2Crossover,
    pub(crate) xo_highmid_high_r: LR2Crossover,

    // Smoothed states
    pub(crate) correlation_decay_coef: f32,
    pub(crate) smoothed_band_power: [f32; 5],
    pub(crate) corr_avg_lr: f32,
    pub(crate) corr_avg_l2: f32,
    pub(crate) corr_avg_r2: f32,
    pub(crate) peak_hold_value: f32,
    pub(crate) peak_hold_l_value: f32,
    pub(crate) peak_hold_r_value: f32,

    // AUTO LOUD
    pub(crate) auto_loud_in: AutoLoudMeter,
    pub(crate) auto_loud_pre_sat: AutoLoudMeter,
    pub(crate) auto_loud_out: AutoLoudMeter,
    pub(crate) pre_sat_buf_l: Vec<f32>,
    pub(crate) pre_sat_buf_r: Vec<f32>,

    // Goniometer
    pub(crate) scope_vis_envelope: f32,

    // FFT
    pub(crate) fft_planner: RealFftPlanner<f32>,
    pub(crate) fft_input: Vec<f32>,
    pub(crate) fft_write_pos: usize,
    pub(crate) fft_hann: Vec<f32>,
    pub(crate) fft_windowed: Vec<f32>,
    pub(crate) fft_output_cache: Vec<realfft::num_complex::Complex<f32>>,

    // SNAP
    pub(crate) snap_fft: SnapFFT,

    // Dirty-flag caches
    pub(crate) cached_hpf_freq: f32,
    pub(crate) cached_lpf_freq: f32,
    pub(crate) cached_cut_slope: i64,
    pub(crate) cached_bass_gain: f32,
    pub(crate) cached_bass_slope: i64,
    pub(crate) cached_lo_mid_gain: f32,
    pub(crate) cached_lo_mid_slope: i64,
    pub(crate) cached_mid_gain: f32,
    pub(crate) cached_mid_slope: i64,
    pub(crate) cached_high_gain: f32,
    pub(crate) cached_high_slope: i64,
    pub(crate) cached_excite_gain: f32,
    pub(crate) cached_excite_slope: i64,
    pub(crate) cached_eq_freq_1: f32,
    pub(crate) cached_eq_freq_2: f32,
    pub(crate) cached_eq_freq_3: f32,
    pub(crate) cached_eq_freq_4: f32,
    pub(crate) cached_eq_freq_5: f32,
    pub(crate) cached_tilt_gain: f32,
    pub(crate) cached_excite_freq: f32,
    pub(crate) cached_sample_rate: f32,
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

    fn info() -> PluginInfo {
        let mut info = PluginInfo::new(
            "Meridian",
            "LX Audiolabs",
            env!("CARGO_PKG_VERSION"),
            "meridian",
        );
        // Stable ship IDs — must match pre-AURA truce Meridian (Bitwig keys sessions
        // on clap id; com.lx-audiolabs.* breaks existing projects + device cache).
        info.clap_id = "be.lxndr.meridian";
        info.vst3_id = "be.lxndr.meridian";
        info.lv2_uri = "https://lx-audiolabs.com/lv2/meridian";
        info.category = PluginCategory::Effect;
        info
    }

    fn bus_layouts() -> Vec<BusLayout> {
        vec![BusLayout::stereo()]
    }

    fn init(params: &Self::Params, sample_rate: f64) -> Self::DspState {
        let mut state = MeridianDspState::default();
        Self::reset(&mut state, params, &AudioConfig::new(sample_rate, 4096));
        state
    }

    fn reset(state: &mut MeridianDspState, params: &MeridianParams, config: &AudioConfig) {
        let sr = config.sample_rate as f32;

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
            .spectrum.sample_rate
            .store(sr, std::sync::atomic::Ordering::Release);
    }

    fn process(
        state: &mut MeridianDspState,
        params: &MeridianParams,
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
aura::export!(Meridian);

#[cfg(feature = "vst3")]
aura::export_vst3!(Meridian);

#[cfg(feature = "lv2")]
aura::export_lv2!(Meridian);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips() {
        aura_test::assert_state_round_trip::<Meridian>();
    }
}
