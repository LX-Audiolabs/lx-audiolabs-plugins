#![allow(unsafe_op_in_unsafe_fn)]

// Aether — Headphone monitoring corrector (AURA).
//
// MONITORING ONLY: place in Reaper Monitor-FX or on a separate monitor track,
// never in the print/mastering chain (crossfeed alters the audio).
//
// Signal chain (per plugin-aether.md):
//     Crossfeed  ->  Harman 5-band EQ (identical L/R)  ->  Gain
//
// The Harman EQ is a plain per-channel-identical linear EQ (no M/S, no L/R diff),
// so it commutes with the crossfeed — order is conceptual, not sonic.

use aura_dsp::analysis::product_shared::AetherShared;
use aura_dsp::fx::Biquad;
use std::sync::Arc;
use aura::prelude::*;

mod editor;
mod process;
mod presets;

pub(crate) const NUM_BANDS: usize = 5;
pub(crate) const CF_DELAY_MAX: usize = 512; // ponytail: used in AetherDspState::Default via vec![0.0; CF_DELAY_MAX] — keep for readability
// Window size is defined in editor.rs for the Slint UI.

// ─── Params ──────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct AetherParams {
    #[param(id = 1,
        name = "EQ1 Freq",
        default = 105.0,
        range = "log(20.0, 20000.0)",
        unit = "Hz",
        smooth = "linear(20)"
    )]
    pub eq1_freq: FloatParam,
    #[param(id = 2,
        name = "EQ1 Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)"
    )]
    pub eq1_gain: FloatParam,
    #[param(id = 3,
        name = "EQ1 Q",
        default = 0.7,
        range = "log(0.3, 8.0)",
        smooth = "linear(20)"
    )]
    pub eq1_q: FloatParam,

    #[param(id = 4,
        name = "EQ2 Freq",
        default = 300.0,
        range = "log(20.0, 20000.0)",
        unit = "Hz",
        smooth = "linear(20)"
    )]
    pub eq2_freq: FloatParam,
    #[param(id = 5,
        name = "EQ2 Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)"
    )]
    pub eq2_gain: FloatParam,
    #[param(id = 6,
        name = "EQ2 Q",
        default = 1.0,
        range = "log(0.3, 8.0)",
        smooth = "linear(20)"
    )]
    pub eq2_q: FloatParam,

    #[param(id = 7,
        name = "EQ3 Freq",
        default = 1200.0,
        range = "log(20.0, 20000.0)",
        unit = "Hz",
        smooth = "linear(20)"
    )]
    pub eq3_freq: FloatParam,
    #[param(id = 8,
        name = "EQ3 Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)"
    )]
    pub eq3_gain: FloatParam,
    #[param(id = 9,
        name = "EQ3 Q",
        default = 1.0,
        range = "log(0.3, 8.0)",
        smooth = "linear(20)"
    )]
    pub eq3_q: FloatParam,

    #[param(id = 10,
        name = "EQ4 Freq",
        default = 4000.0,
        range = "log(20.0, 20000.0)",
        unit = "Hz",
        smooth = "linear(20)"
    )]
    pub eq4_freq: FloatParam,
    #[param(id = 11,
        name = "EQ4 Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)"
    )]
    pub eq4_gain: FloatParam,
    #[param(id = 12,
        name = "EQ4 Q",
        default = 1.0,
        range = "log(0.3, 8.0)",
        smooth = "linear(20)"
    )]
    pub eq4_q: FloatParam,

    #[param(id = 13,
        name = "EQ5 Freq",
        default = 10000.0,
        range = "log(20.0, 20000.0)",
        unit = "Hz",
        smooth = "linear(20)"
    )]
    pub eq5_freq: FloatParam,
    #[param(id = 14,
        name = "EQ5 Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)"
    )]
    pub eq5_gain: FloatParam,
    #[param(id = 15,
        name = "EQ5 Q",
        default = 0.7,
        range = "log(0.3, 8.0)",
        smooth = "linear(20)"
    )]
    pub eq5_q: FloatParam,

    #[param(id = 16, name = "EQ1 Type", default = 1, range = "discrete(0, 3)")]
    pub eq1_type: IntParam,
    #[param(id = 17, name = "EQ2 Type", default = 2, range = "discrete(0, 3)")]
    pub eq2_type: IntParam,
    #[param(id = 18, name = "EQ3 Type", default = 2, range = "discrete(0, 3)")]
    pub eq3_type: IntParam,
    #[param(id = 19, name = "EQ4 Type", default = 2, range = "discrete(0, 3)")]
    pub eq4_type: IntParam,
    #[param(id = 20, name = "EQ5 Type", default = 3, range = "discrete(0, 3)")]
    pub eq5_type: IntParam,

    #[param(id = 21,
        name = "Blend",
        default = 100.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Aether"
    )]
    pub blend: FloatParam,

    #[param(id = 22,
        name = "Crossfeed Angle",
        default = 60.0,
        range = "linear(30.0, 75.0)",
        unit = "deg",
        smooth = "linear(20)",
        group = "Aether"
    )]
    pub cf_angle: FloatParam,
    #[param(id = 23,
        name = "Crossfeed Amount",
        default = 0.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Aether"
    )]
    pub cf_amount: FloatParam,
    #[param(id = 24,
        name = "Crossfeed Realism",
        default = 0,
        range = "discrete(0, 2)",
        group = "Aether"
    )]
    pub cf_realism: IntParam,

    #[param(id = 25,
        name = "Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Aether"
    )]
    pub gain: FloatParam,

    #[param(id = 26, name = "Bypass", default = 0)]
    pub bypass: BoolParam,

    #[skip]
    pub shared: Arc<AetherShared>,
}

impl AetherParams {
    /// Real value display for `unit = "%"` params: our plain values are
    /// already the percent number (e.g. `100.0` means `100%`), not a
    /// 0.0-1.0 fraction. `aura_params::format_param_value`'s built-in
    /// Percent case multiplies by 100 assuming the latter, so it would
    /// show `10000%` for a real 100% value without this override.
    fn fmt_pct(&self, value: f64) -> String {
        format!("{value:.1}%")
    }
}

// ─── Plugin ───────────────────────────────────────────────────────────────────

pub struct Aether;

pub struct AetherDspState {
    pub(crate) sample_rate: f32,
    pub(crate) eq_l: [Biquad; NUM_BANDS],
    pub(crate) eq_r: [Biquad; NUM_BANDS],
    pub(crate) cf_lp_l: f32,
    pub(crate) cf_lp_r: f32,
    pub(crate) cf_delay_l: Vec<f32>,
    pub(crate) cf_delay_r: Vec<f32>,
    pub(crate) cf_delay_pos: usize,
}

impl Default for AetherDspState {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            eq_l: std::array::from_fn(|_| Biquad::new()),
            eq_r: std::array::from_fn(|_| Biquad::new()),
            cf_lp_l: 0.0,
            cf_lp_r: 0.0,
            // Always allocate delay lines — process must never see empty buffers
            // (len()-1 underflow / % 0 panic if process races reset).
            cf_delay_l: vec![0.0; CF_DELAY_MAX],
            cf_delay_r: vec![0.0; CF_DELAY_MAX],
            cf_delay_pos: 0,
        }
    }
}

impl AetherDspState {
    pub(crate) fn update_eq_coeffs(&mut self, params: &AetherParams) {
        let sr = self.sample_rate;
        let vals: [(f32, f32, f32, i32); NUM_BANDS] = [
            (
                params.eq1_freq.raw_target() as f32,
                params.eq1_gain.raw_target() as f32,
                params.eq1_q.raw_target() as f32,
                params.eq1_type.value_i32(),
            ),
            (
                params.eq2_freq.raw_target() as f32,
                params.eq2_gain.raw_target() as f32,
                params.eq2_q.raw_target() as f32,
                params.eq2_type.value_i32(),
            ),
            (
                params.eq3_freq.raw_target() as f32,
                params.eq3_gain.raw_target() as f32,
                params.eq3_q.raw_target() as f32,
                params.eq3_type.value_i32(),
            ),
            (
                params.eq4_freq.raw_target() as f32,
                params.eq4_gain.raw_target() as f32,
                params.eq4_q.raw_target() as f32,
                params.eq4_type.value_i32(),
            ),
            (
                params.eq5_freq.raw_target() as f32,
                params.eq5_gain.raw_target() as f32,
                params.eq5_q.raw_target() as f32,
                params.eq5_type.value_i32(),
            ),
        ];
        for (i, &(fc, g, q, t)) in vals.iter().enumerate() {
            set_band(&mut self.eq_l[i], t, fc, g, q, sr);
            set_band(&mut self.eq_r[i], t, fc, g, q, sr);
        }
    }
}

pub fn set_band(b: &mut Biquad, type_code: i32, fc: f32, gain: f32, q: f32, sr: f32) {
    match type_code {
        1 => b.set_low_shelf(fc, gain, q.clamp(0.3, 2.0), sr),
        2 => b.set_peaking_eq(fc, gain, q, sr),
        3 => b.set_high_shelf(fc, gain, q.clamp(0.3, 2.0), sr),
        _ => b.set_peaking_eq(1000.0, 0.0, 0.7, sr),
    }
}

pub fn band_type_label(type_code: i32) -> &'static str {
    match type_code {
        1 => "LSC",
        2 => "PK",
        3 => "HSC",
        _ => "OFF",
    }
}

pub fn realism_label(code: i32) -> &'static str {
    match code {
        1 => "LIFELIKE",
        2 => "HYPERREAL",
        _ => "STANDARD",
    }
}

// ─── AutoEQ parser ───────────────────────────────────────────────────────────

pub struct AutoEqFilter {
    pub type_code: i32,
    pub freq: f32,
    pub gain: f32,
    pub q: f32,
}
pub struct AutoEqProfile {
    pub preamp: f32,
    pub filters: Vec<AutoEqFilter>,
}

pub fn parse_autoeq(content: &str) -> AutoEqProfile {
    let mut preamp = 0.0f32;
    let mut filters = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Preamp:") {
            if let Some(v) = rest
                .split_whitespace()
                .next()
                .and_then(|t| t.parse::<f32>().ok())
            {
                preamp = v;
            }
            continue;
        }
        if !line.starts_with("Filter") {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        if !toks.contains(&"ON") {
            continue;
        }
        let type_code = if toks.iter().any(|t| *t == "LSC" || *t == "LS") {
            1
        } else if toks.iter().any(|t| *t == "HSC" || *t == "HS") {
            3
        } else if toks.iter().any(|t| *t == "PK" || *t == "PEQ") {
            2
        } else {
            continue;
        };
        let after = |kw: &str| {
            toks.iter()
                .position(|t| *t == kw)
                .and_then(|i| toks.get(i + 1))
                .and_then(|t| t.parse::<f32>().ok())
        };
        if let (Some(freq), Some(gain), Some(q)) = (after("Fc"), after("Gain"), after("Q")) {
            filters.push(AutoEqFilter {
                type_code,
                freq,
                gain,
                q,
            });
        }
    }
    AutoEqProfile { preamp, filters }
}

// ─── PluginLogic ──────────────────────────────────────────────────────────────

impl PluginLogic for Aether {
    type Params = AetherParams;
    type DspState = AetherDspState;

    fn info() -> PluginInfo {
        let mut info = PluginInfo::new(
            "Aether",
            "LX Audiolabs",
            env!("CARGO_PKG_VERSION"),
            "aether",
        );
        // Stable ship IDs — must match pre-AURA truce Aether (Bitwig keys sessions
        // on clap id; com.lx-audiolabs.* breaks existing projects + device cache).
        info.clap_id = "be.lxndr.aether";
        info.vst3_id = "be.lxndr.aether";
        info.lv2_uri = "https://lx-audiolabs.com/lv2/aether";
        info.category = PluginCategory::Effect;
        info
    }

    fn bus_layouts() -> Vec<BusLayout> {
        vec![BusLayout::stereo()]
    }

    fn init(params: &AetherParams, sample_rate: f64) -> AetherDspState {
        let mut state = AetherDspState::default();
        // Biquad::default() zero-coefs would silence output if a host never
        // calls reset — seed real coefficients at init.
        Self::reset(&mut state, params, &AudioConfig::new(sample_rate, 4096));
        state
    }

    fn reset(state: &mut AetherDspState, params: &AetherParams, config: &AudioConfig) {
        let sr = (config.sample_rate as f32).max(1.0);
        state.sample_rate = sr;
        params
            .shared
            .sample_rate
            .store(sr, std::sync::atomic::Ordering::Release);
        for b in state.eq_l.iter_mut().chain(state.eq_r.iter_mut()) {
            b.reset();
        }
        state.cf_lp_l = 0.0;
        state.cf_lp_r = 0.0;
        if state.cf_delay_l.len() != CF_DELAY_MAX {
            state.cf_delay_l.resize(CF_DELAY_MAX, 0.0);
        }
        if state.cf_delay_r.len() != CF_DELAY_MAX {
            state.cf_delay_r.resize(CF_DELAY_MAX, 0.0);
        }
        state.cf_delay_l.fill(0.0);
        state.cf_delay_r.fill(0.0);
        state.cf_delay_pos = 0;
        state.update_eq_coeffs(params);
    }

    fn process(
        state: &mut AetherDspState,
        params: &AetherParams,
        buffer: &mut AudioBuffer<'_, f32>,
        _ctx: &mut ProcessContext,
    ) -> ProcessStatus {
        process::run(state, params, buffer)
    }

    // AURA handles state via params — truce's snapshot_into / load_state /
    // state_changed (and the lx_dsp::state_migration legacy path) are gone.

    fn editor(params: Arc<Self::Params>) -> Option<Box<dyn Editor>> {
        Some(editor::build_editor(params))
    }
}

#[cfg(feature = "clap")]
aura::export!(Aether);

#[cfg(feature = "vst3")]
aura::export_vst3!(Aether);

#[cfg(feature = "lv2")]
aura::export_lv2!(Aether);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_autoeq() {
        let txt = "Preamp: -7.13 dB\nFilter 1: ON LSC Fc 105.0 Hz Gain 3.3 dB Q 0.70\nFilter 2: ON PK Fc 118.4 Hz Gain -3.3 dB Q 0.45\nFilter 3: OFF PK Fc 200.0 Hz Gain 1.0 dB Q 1.00\nFilter 4: ON HSC Fc 10000.0 Hz Gain 2.0 dB Q 0.70\n";
        let p = parse_autoeq(txt);
        assert!((p.preamp + 7.13).abs() < 1e-3);
        assert_eq!(p.filters.len(), 3);
        assert_eq!(p.filters[0].type_code, 1);
        assert_eq!(p.filters[2].type_code, 3);
    }

    #[test]
    fn state_round_trips() {
        aura_test::assert_state_round_trip::<Aether>();
    }
}
