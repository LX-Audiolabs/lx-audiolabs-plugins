#![allow(unsafe_op_in_unsafe_fn)]

// Aether — Headphone monitoring corrector (truce port).
//
// MONITORING ONLY: place in Reaper Monitor-FX or on a separate monitor track,
// never in the print/mastering chain (crossfeed alters the audio).
//
// Signal chain (per plugin-aether.md):
//     Crossfeed  ->  Harman 5-band EQ (identical L/R)  ->  Gain
//
// The Harman EQ is a plain per-channel-identical linear EQ (no M/S, no L/R diff),
// so it commutes with the crossfeed — order is conceptual, not sonic.

use shared_analysis::SharedState;
use shared_dsp::{Biquad, FtzDazGuard, state_migration};
use std::sync::Arc;
use truce::prelude::*;
use truce_core::editor::Editor;
use truce_core::state::StateLoadError;

mod editor;

const NUM_BANDS: usize = 5;
const CF_DELAY_MAX: usize = 512; // ponytail: used in AetherDspState::Default via vec![0.0; CF_DELAY_MAX] — keep for readability
// Window size is defined in editor.rs for the Slint UI.

// ─── Params ──────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct AetherParams {
    #[param(
        name = "EQ1 Freq",
        default = 105.0,
        range = "log(20.0, 20000.0)",
        unit = "Hz",
        smooth = "linear(20)"
    )]
    pub eq1_freq: FloatParam,
    #[param(
        name = "EQ1 Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)"
    )]
    pub eq1_gain: FloatParam,
    #[param(
        name = "EQ1 Q",
        default = 0.7,
        range = "log(0.3, 8.0)",
        smooth = "linear(20)"
    )]
    pub eq1_q: FloatParam,

    #[param(
        name = "EQ2 Freq",
        default = 300.0,
        range = "log(20.0, 20000.0)",
        unit = "Hz",
        smooth = "linear(20)"
    )]
    pub eq2_freq: FloatParam,
    #[param(
        name = "EQ2 Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)"
    )]
    pub eq2_gain: FloatParam,
    #[param(
        name = "EQ2 Q",
        default = 1.0,
        range = "log(0.3, 8.0)",
        smooth = "linear(20)"
    )]
    pub eq2_q: FloatParam,

    #[param(
        name = "EQ3 Freq",
        default = 1200.0,
        range = "log(20.0, 20000.0)",
        unit = "Hz",
        smooth = "linear(20)"
    )]
    pub eq3_freq: FloatParam,
    #[param(
        name = "EQ3 Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)"
    )]
    pub eq3_gain: FloatParam,
    #[param(
        name = "EQ3 Q",
        default = 1.0,
        range = "log(0.3, 8.0)",
        smooth = "linear(20)"
    )]
    pub eq3_q: FloatParam,

    #[param(
        name = "EQ4 Freq",
        default = 4000.0,
        range = "log(20.0, 20000.0)",
        unit = "Hz",
        smooth = "linear(20)"
    )]
    pub eq4_freq: FloatParam,
    #[param(
        name = "EQ4 Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)"
    )]
    pub eq4_gain: FloatParam,
    #[param(
        name = "EQ4 Q",
        default = 1.0,
        range = "log(0.3, 8.0)",
        smooth = "linear(20)"
    )]
    pub eq4_q: FloatParam,

    #[param(
        name = "EQ5 Freq",
        default = 10000.0,
        range = "log(20.0, 20000.0)",
        unit = "Hz",
        smooth = "linear(20)"
    )]
    pub eq5_freq: FloatParam,
    #[param(
        name = "EQ5 Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)"
    )]
    pub eq5_gain: FloatParam,
    #[param(
        name = "EQ5 Q",
        default = 0.7,
        range = "log(0.3, 8.0)",
        smooth = "linear(20)"
    )]
    pub eq5_q: FloatParam,

    #[param(name = "EQ1 Type", default = 1, range = "discrete(0, 3)")]
    pub eq1_type: IntParam,
    #[param(name = "EQ2 Type", default = 2, range = "discrete(0, 3)")]
    pub eq2_type: IntParam,
    #[param(name = "EQ3 Type", default = 2, range = "discrete(0, 3)")]
    pub eq3_type: IntParam,
    #[param(name = "EQ4 Type", default = 2, range = "discrete(0, 3)")]
    pub eq4_type: IntParam,
    #[param(name = "EQ5 Type", default = 3, range = "discrete(0, 3)")]
    pub eq5_type: IntParam,

    #[param(
        name = "Blend",
        default = 100.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Aether"
    )]
    pub blend: FloatParam,

    #[param(
        name = "Crossfeed Angle",
        default = 60.0,
        range = "linear(30.0, 75.0)",
        unit = "deg",
        smooth = "linear(20)",
        group = "Aether"
    )]
    pub cf_angle: FloatParam,
    #[param(
        name = "Crossfeed Amount",
        default = 0.0,
        range = "linear(0.0, 100.0)",
        unit = "%",
        format = "fmt_pct",
        smooth = "linear(20)",
        group = "Aether"
    )]
    pub cf_amount: FloatParam,
    #[param(
        name = "Crossfeed Realism",
        default = 0,
        range = "discrete(0, 2)",
        group = "Aether"
    )]
    pub cf_realism: IntParam,

    #[param(
        name = "Gain",
        default = 0.0,
        range = "linear(-12.0, 12.0)",
        unit = "dB",
        smooth = "linear(20)",
        group = "Aether"
    )]
    pub gain: FloatParam,

    #[param(name = "Bypass", default = 0)]
    pub bypass: BoolParam,

    #[skip]
    pub shared: Arc<SharedState>,
}

impl AetherParams {
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

pub struct Aether;

#[derive(Default)]
pub struct AetherDspState {
    sample_rate: f32,
    eq_l: [Biquad; NUM_BANDS],
    eq_r: [Biquad; NUM_BANDS],
    cf_lp_l: f32,
    cf_lp_r: f32,
    cf_delay_l: Vec<f32>,
    cf_delay_r: Vec<f32>,
    cf_delay_pos: usize,
}

impl AetherDspState {
    fn update_eq_coeffs(&mut self, params: &AetherParams) {
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

    fn bus_layouts() -> Vec<BusLayout> {
        vec![BusLayout::stereo()]
    }

    fn init(_params: &AetherParams, _cx: &InitContext) -> AetherDspState {
        AetherDspState {
            cf_delay_l: vec![0.0; CF_DELAY_MAX],
            cf_delay_r: vec![0.0; CF_DELAY_MAX],
            ..Default::default()
        }
    }

    fn reset(state: &mut AetherDspState, params: &AetherParams, config: &AudioConfig) {
        let sr = config.sample_rate;
        state.sample_rate = sr as f32;
        params
            .shared
            .sample_rate
            .store(sr as f32, std::sync::atomic::Ordering::Release);
        for b in state.eq_l.iter_mut().chain(state.eq_r.iter_mut()) {
            b.reset();
        }
        state.cf_lp_l = 0.0;
        state.cf_lp_r = 0.0;
        state.cf_delay_l.resize(CF_DELAY_MAX, 0.0);
        state.cf_delay_r.resize(CF_DELAY_MAX, 0.0);
        state.cf_delay_l.fill(0.0);
        state.cf_delay_r.fill(0.0);
        state.cf_delay_pos = 0;
        state.update_eq_coeffs(params);
    }

    fn process(
        state: &mut AetherDspState,
        params: &AetherParams,
        buffer: &mut AudioBuffer,
        _events: &EventList,
        _ctx: &mut ProcessContext,
    ) -> ProcessStatus {
        let _ftz = FtzDazGuard::new();

        if buffer.num_input_channels() < 2 {
            return ProcessStatus::Normal;
        }
        let num_samples = buffer.num_samples();

        let mut in_peak = 0.0f32;
        for i in 0..num_samples {
            in_peak = in_peak
                .max(buffer.input(0)[i].abs())
                .max(buffer.input(1)[i].abs());
        }
        let in_db = if in_peak < 1e-9 {
            -90.0
        } else {
            20.0 * in_peak.log10()
        };
        params
            .shared
            .input_peak
            .store(in_db, std::sync::atomic::Ordering::Release);

        if params.bypass.value() {
            for ch in 0..buffer.channels() {
                let (inp, out) = buffer.io(ch);
                out.copy_from_slice(inp);
            }
            return ProcessStatus::Normal;
        }

        state.update_eq_coeffs(params);
        let blend = params.blend.raw_target() as f32 / 100.0;
        let (itd_ms, cut_mul, feed_mul) = match params.cf_realism.value_i32() {
            1 => (0.32, 0.85, 1.05),
            2 => (0.45, 0.70, 1.15),
            _ => (0.22, 1.00, 1.00),
        };
        let cf_mix =
            ((params.cf_amount.raw_target() as f32 / 100.0) * 0.5 * feed_mul).min(0.75);
        let cf_norm = ((params.cf_angle.raw_target() as f32 - 30.0) / 45.0).clamp(0.0, 1.0);
        let cf_fc = (700.0 + cf_norm * 1300.0) * cut_mul;
        let cf_a = 1.0 - (-2.0 * std::f32::consts::PI * cf_fc / state.sample_rate).exp();
        let delay_samples =
            ((itd_ms * 0.001 * state.sample_rate).round() as usize).min(state.cf_delay_l.len() - 1);

        for i in 0..num_samples {
            let in_l = buffer.input(0)[i];
            let in_r = buffer.input(1)[i];

            let mut eq_l = in_l;
            let mut eq_r = in_r;
            for b in 0..NUM_BANDS {
                eq_l = state.eq_l[b].process(eq_l);
                eq_r = state.eq_r[b].process(eq_r);
            }
            let h_l = in_l + (eq_l - in_l) * blend;
            let h_r = in_r + (eq_r - in_r) * blend;

            let wp = state.cf_delay_pos;
            state.cf_delay_l[wp] = h_l;
            state.cf_delay_r[wp] = h_r;
            let rp = (wp + state.cf_delay_l.len() - delay_samples) % state.cf_delay_l.len();
            let del_l = state.cf_delay_l[rp];
            let del_r = state.cf_delay_r[rp];
            state.cf_delay_pos = (wp + 1) % state.cf_delay_l.len();

            state.cf_lp_l += cf_a * (del_r - state.cf_lp_l);
            state.cf_lp_r += cf_a * (del_l - state.cf_lp_r);
            let cf_l = h_l + state.cf_lp_l * cf_mix;
            let cf_r = h_r + state.cf_lp_r * cf_mix;

            let gain_smoothed = params.gain.value();
            let g = 10.0_f32.powf(gain_smoothed / 20.0);
            buffer.output(0)[i] = cf_l * g;
            buffer.output(1)[i] = cf_r * g;
        }

        ProcessStatus::Normal
    }

    fn snapshot_into(_state: &AetherDspState, _buf: &mut Vec<u8>) -> bool {
        false
    }
    fn load_state(_state: &mut AetherDspState, data: &[u8]) -> Result<(), StateLoadError> {
        if let Some(_params) = state_migration::try_parse_niceplug_state(data) {
            // Note: load_state in 6.x receives state, not self. The param values
            // must be set via the params reference passed by the host, but here we
            // only have &AetherParams which is immutable. In practice, Truce's
            // load_state is called with params already mutable by the runtime.
            // For NicePlug migration, we skip — the state bytes are envelope-only
            // in modern hosts; this path is for legacy session recovery.
        }
        Ok(())
    }
    fn state_changed(_state: &mut AetherDspState, _params: &AetherParams) {}

    fn editor(params: Arc<Self::Params>) -> Box<dyn Editor> {
        editor::build_editor(params)
    }
}

truce::plugin! { logic: Aether, params: AetherParams }

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
}
