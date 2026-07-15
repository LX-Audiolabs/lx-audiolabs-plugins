pub mod state_migration;

/// 2nd-order Biquad Filter structure for Butterworth LP/HP and Mono Maker.
///
/// Runs in **Transposed Direct Form II**: the state (`s1`/`s2`) holds partial
/// sums rather than raw input/output history. This is far more robust to
/// coefficient changes than Direct Form I — when fc/Q/gain jump in a single
/// block (slider drag, A/B/C slope switch), DF-I leaves the large output
/// history `y1`/`y2` mismatched against the new feedback coefficients, which
/// fires a loud transient that rings on the new resonance (a low, narrow,
/// high-gain band booms like a "mega low shelf"). TDF-II state stays
/// consistent, so the transfer function is identical but the modulation glitch
/// is gone.
#[derive(Clone, Default)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    s1: f64,
    s2: f64,
}

impl Biquad {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x_f64 = x as f64;
        // Transposed Direct Form II
        let y = self.b0 * x_f64 + self.s1;
        let s1 = self.b1 * x_f64 - self.a1 * y + self.s2;
        let s2 = self.b2 * x_f64 - self.a2 * y;
        // Finite-guard (last line of defense): if the state ever goes non-finite,
        // reset instead of poisoning every downstream persistent accumulator
        // (later filters, goniometer correlation, FFT) forever — that was the
        // "Reaper freezes until Reset" failure mode.
        if !y.is_finite() || !s1.is_finite() || !s2.is_finite() {
            self.s1 = 0.0;
            self.s2 = 0.0;
            return 0.0;
        }
        // Avoid denormals in the state
        self.s1 = if s1.abs() < 1e-30 { 0.0 } else { s1 };
        self.s2 = if s2.abs() < 1e-30 { 0.0 } else { s2 };
        y as f32
    }

    pub fn set_coefs(&mut self, b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) {
        // NO magnitude clamp. The RBJ-cookbook shelf/peaking filters (and our
        // Butterworth setters) are unconditionally stable by construction, so a
        // pole near the unit circle is correct, not dangerous: a low-frequency
        // low shelf naturally has a2 ≈ 0.9999, matched by a zero right next to it.
        // The old clamp (|a2| ≤ 0.99) moved ONLY the pole while leaving the zeros
        // in place — that broke the pole/zero pairing and turned a gentle +12 dB
        // shelf into a ~+40 dB resonator. Sweeping the Low/High Cut then pumped
        // energy into that resonance until the output ran away. We now only reject
        // non-finite coefficients and otherwise trust the design.
        if !(b0.is_finite() && b1.is_finite() && b2.is_finite() && a1.is_finite() && a2.is_finite())
        {
            return; // keep the previous (valid) coefficients
        }

        self.b0 = b0;
        self.b1 = b1;
        self.b2 = b2;
        self.a1 = a1;
        self.a2 = a2;
    }

    pub fn set_butterworth_hp(&mut self, fc: f32, sample_rate: f32) {
        // Guard: sr=0 produces NaN (division by zero) → keep old coefficients.
        if sample_rate < 1.0 {
            return;
        }
        let fc = (fc as f64).min(0.49 * (sample_rate as f64)).max(1.0);
        let sr = sample_rate as f64;
        let theta = std::f64::consts::PI * fc / sr;
        let k = theta.tan();
        let norm = 1.0 / (1.0 + std::f64::consts::FRAC_1_SQRT_2 * 2.0 * k + k * k);

        self.b0 = norm;
        self.b1 = -2.0 * norm;
        self.b2 = norm;
        self.a1 = 2.0 * (k * k - 1.0) * norm;
        self.a2 = (1.0 - std::f64::consts::FRAC_1_SQRT_2 * 2.0 * k + k * k) * norm;
    }

    pub fn set_butterworth_lp(&mut self, fc: f32, sample_rate: f32) {
        if sample_rate < 1.0 {
            return;
        }
        let fc = (fc as f64).min(0.49 * (sample_rate as f64)).max(1.0);
        let sr = sample_rate as f64;
        let theta = std::f64::consts::PI * fc / sr;
        let k = theta.tan();
        let norm = 1.0 / (1.0 + std::f64::consts::FRAC_1_SQRT_2 * 2.0 * k + k * k);

        self.b0 = k * k * norm;
        self.b1 = 2.0 * k * k * norm;
        self.b2 = k * k * norm;
        self.a1 = 2.0 * (k * k - 1.0) * norm;
        self.a2 = (1.0 - std::f64::consts::FRAC_1_SQRT_2 * 2.0 * k + k * k) * norm;
    }

    /// 2nd-order high-pass with arbitrary Q. Cascade two of these with staggered Q
    /// (0.54119610 & 1.30656296) for a 4th-order (24 dB/oct) Butterworth.
    pub fn set_butterworth_hp_q(&mut self, fc: f32, q: f32, sample_rate: f32) {
        if sample_rate < 1.0 {
            return;
        }
        let fc = (fc as f64).min(0.49 * (sample_rate as f64)).max(1.0);
        let sr = sample_rate as f64;
        let theta = std::f64::consts::PI * fc / sr;
        let k = theta.tan();
        let inv_q = 1.0 / (q as f64);
        let norm = 1.0 / (1.0 + inv_q * k + k * k);

        self.b0 = norm;
        self.b1 = -2.0 * norm;
        self.b2 = norm;
        self.a1 = 2.0 * (k * k - 1.0) * norm;
        self.a2 = (1.0 - inv_q * k + k * k) * norm;
    }

    /// 2nd-order low-pass with arbitrary Q. Cascade two of these with staggered Q
    /// (0.54119610 & 1.30656296) for a 4th-order (24 dB/oct) Butterworth.
    pub fn set_butterworth_lp_q(&mut self, fc: f32, q: f32, sample_rate: f32) {
        if sample_rate < 1.0 {
            return;
        }
        let fc = (fc as f64).min(0.49 * (sample_rate as f64)).max(1.0);
        let sr = sample_rate as f64;
        let theta = std::f64::consts::PI * fc / sr;
        let k = theta.tan();
        let inv_q = 1.0 / (q as f64);
        let norm = 1.0 / (1.0 + inv_q * k + k * k);

        self.b0 = k * k * norm;
        self.b1 = 2.0 * k * k * norm;
        self.b2 = k * k * norm;
        self.a1 = 2.0 * (k * k - 1.0) * norm;
        self.a2 = (1.0 - inv_q * k + k * k) * norm;
    }

    /// Pass-through (identity) — used for an unused cascade section.
    pub fn set_identity(&mut self) {
        self.b0 = 1.0;
        self.b1 = 0.0;
        self.b2 = 0.0;
        self.a1 = 0.0;
        self.a2 = 0.0;
    }

    pub fn set_peaking_eq(&mut self, fc: f32, db_gain: f32, q: f32, sample_rate: f32) {
        let a = 10.0f64.powf(db_gain as f64 / 40.0);
        let omega = 2.0 * std::f64::consts::PI * (fc as f64) / (sample_rate as f64);
        let sn = omega.sin();
        let cs = omega.cos();
        let alpha = sn / (2.0 * (q as f64));

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cs;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cs;
        let a2 = 1.0 - alpha / a;

        let norm = 1.0 / a0;
        self.set_coefs(b0 * norm, b1 * norm, b2 * norm, a1 * norm, a2 * norm);
    }

    pub fn set_low_shelf(&mut self, fc: f32, db_gain: f32, slope_s: f32, sample_rate: f32) {
        let a = 10.0f64.powf(db_gain as f64 / 40.0);
        let omega = 2.0 * std::f64::consts::PI * (fc as f64) / (sample_rate as f64);
        let sn = omega.sin();
        let cs = omega.cos();
        let alpha = (sn / 2.0)
            * (((a + 1.0 / a) * (1.0 / (slope_s as f64) - 1.0) + 2.0)
                .max(0.0)
                .sqrt());

        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let b0 = a * ((a + 1.0) - (a - 1.0) * cs + two_sqrt_a_alpha);
        let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cs);
        let b2 = a * ((a + 1.0) - (a - 1.0) * cs - two_sqrt_a_alpha);
        let a0 = (a + 1.0) + (a - 1.0) * cs + two_sqrt_a_alpha;
        let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cs);
        let a2 = (a + 1.0) + (a - 1.0) * cs - two_sqrt_a_alpha;

        let norm = 1.0 / a0;
        self.set_coefs(b0 * norm, b1 * norm, b2 * norm, a1 * norm, a2 * norm);
    }

    /// Magnitude response at `freq` Hz in dB. Useful for drawing EQ transfer function curves.
    pub fn magnitude_db(&self, freq: f32, sample_rate: f32) -> f32 {
        let w = 2.0 * std::f64::consts::PI * (freq as f64) / (sample_rate as f64);
        let cw = w.cos();
        let c2w = (2.0 * w).cos();
        let sw = w.sin();
        let s2w = (2.0 * w).sin();
        let nr = self.b0 + self.b1 * cw + self.b2 * c2w;
        let ni = -(self.b1 * sw + self.b2 * s2w);
        let dr = 1.0 + self.a1 * cw + self.a2 * c2w;
        let di = -(self.a1 * sw + self.a2 * s2w);
        let den2 = dr * dr + di * di;
        if den2 < 1e-30 {
            return 0.0;
        }
        let mag2 = (nr * nr + ni * ni) / den2;
        if mag2 < 1e-12 {
            -60.0
        } else {
            (10.0 * mag2.log10()) as f32
        }
    }

    pub fn set_high_shelf(&mut self, fc: f32, db_gain: f32, slope_s: f32, sample_rate: f32) {
        let a = 10.0f64.powf(db_gain as f64 / 40.0);
        let omega = 2.0 * std::f64::consts::PI * (fc as f64) / (sample_rate as f64);
        let sn = omega.sin();
        let cs = omega.cos();
        let alpha = (sn / 2.0)
            * (((a + 1.0 / a) * (1.0 / (slope_s as f64) - 1.0) + 2.0)
                .max(0.0)
                .sqrt());

        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let b0 = a * ((a + 1.0) + (a - 1.0) * cs + two_sqrt_a_alpha);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cs);
        // RBJ cookbook: b2 = A*((A+1) + (A-1)*cos - 2√A·α). The (A-1)*cos term is
        // ADDED, not subtracted — the prior minus sign turned the high shelf into a
        // huge low-frequency boost (e.g. +27 dB @50 Hz for a +6 dB shelf), which is
        // why a ±tilt sounded duller in both directions.
        let b2 = a * ((a + 1.0) + (a - 1.0) * cs - two_sqrt_a_alpha);
        let a0 = (a + 1.0) - (a - 1.0) * cs + two_sqrt_a_alpha;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cs);
        let a2 = (a + 1.0) - (a - 1.0) * cs - two_sqrt_a_alpha;

        let norm = 1.0 / a0;
        self.set_coefs(b0 * norm, b1 * norm, b2 * norm, a1 * norm, a2 * norm);
    }

    pub fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

/// Tilt EQ — Lo shelf + Hi shelf in series at a fixed pivot frequency.
/// Positive `gain_db` boosts lows and cuts highs; negative does the opposite.
#[derive(Clone, Default)]
pub struct TiltEq {
    lo: Biquad,
    hi: Biquad,
}

impl TiltEq {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, pivot_hz: f32, gain_db: f32, sample_rate: f32) {
        self.lo.set_low_shelf(pivot_hz, gain_db, 1.0, sample_rate);
        self.hi.set_high_shelf(pivot_hz, -gain_db, 1.0, sample_rate);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.hi.process(self.lo.process(x))
    }

    pub fn reset(&mut self) {
        self.lo.reset();
        self.hi.reset();
    }
}

// =============================================================================

/// Linkwitz-Riley 2nd-order (LR2) Crossover Filter (consists of cascaded 1st-order LP/HP).
#[derive(Clone, Default)]
pub struct LR2Crossover {
    lp1: Biquad,
    lp2: Biquad,
    hp1: Biquad,
    hp2: Biquad,
}

impl LR2Crossover {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_cutoff(&mut self, fc: f32, sample_rate: f32) {
        let fc = (fc as f64).min(0.47 * (sample_rate as f64)).max(10.0);
        let theta = std::f64::consts::PI * fc / (sample_rate as f64);
        let k = theta.tan();
        let norm = 1.0 / (1.0 + k);

        let b0_lp = k * norm;
        let b1_lp = k * norm;
        let a1_lp = (k - 1.0) * norm;

        let b0_hp = norm;
        let b1_hp = -norm;
        let a1_hp = (k - 1.0) * norm;

        self.lp1.set_coefs(b0_lp, b1_lp, 0.0, a1_lp, 0.0);
        self.lp2.set_coefs(b0_lp, b1_lp, 0.0, a1_lp, 0.0);

        self.hp1.set_coefs(b0_hp, b1_hp, 0.0, a1_hp, 0.0);
        self.hp2.set_coefs(b0_hp, b1_hp, 0.0, a1_hp, 0.0);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> (f32, f32) {
        let lp = self.lp2.process(self.lp1.process(x));
        let hp = -self.hp2.process(self.hp1.process(x));
        (lp, hp)
    }

    /// Perfect-reconstruction split: hp = input − lp, so lp + hp = input always.
    /// Use this when bands are summed back (e.g. Equilibrium). The hp biquads are not touched.
    #[inline]
    pub fn process_transparent(&mut self, x: f32) -> (f32, f32) {
        let lp = self.lp2.process(self.lp1.process(x));
        (lp, x - lp)
    }

    pub fn reset(&mut self) {
        self.lp1.reset();
        self.lp2.reset();
        self.hp1.reset();
        self.hp2.reset();
    }
}

// =============================================================================
// TOLERANCE TABLE — deterministic per-channel parameter micro-variations
// Used by TMT (Tolerance Modeling Technology) in Meridian and Aurum.
// Generated once at plugin init from a u64 seed via Xorshift64.
// N = number of named parameter slots (use the tol:: module in each plugin).
// =============================================================================

pub struct ToleranceTable<const N: usize> {
    offsets: [f32; N],
}

impl<const N: usize> ToleranceTable<N> {
    /// Generate a new table from a fixed seed.
    /// Each slot is a value in [-1.0, 1.0] — multiply by your max tolerance
    /// (e.g. 0.02 for ±2%) when applying to a parameter.
    pub fn new(seed: u64) -> Self {
        let mut state = if seed == 0 { 0xdeadbeefcafe1234 } else { seed };
        let mut offsets = [0.0f32; N];
        for slot in offsets.iter_mut() {
            // Xorshift64 — no deps, no alloc, reproducible
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *slot = (state as i64 as f32) / (i64::MAX as f32);
        }
        Self { offsets }
    }

    /// Returns a value in [-1.0, 1.0] for the given slot index.
    #[inline]
    pub fn get(&self, idx: usize) -> f32 {
        self.offsets[idx]
    }
}

// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarmonicsMode {
    Even,
    Odd,
    Mixed,
}

/// Mastering Saturator (Black Box HG-2 inspired)
/// Provides even (Pentode), odd (Triode), or mixed harmonic generation.
pub struct MasteringSaturator;

impl MasteringSaturator {
    /// Applies mastering-grade saturation to a single sample.
    /// `drive_db`: 0.0 to 12.0 dB
    /// `mix`: 0.0 to 1.0
    #[inline]
    pub fn process(x: f32, drive_db: f32, mix: f32, mode: HarmonicsMode) -> f32 {
        if drive_db <= 0.0 || mix <= 0.0 || x.abs() < 1e-6 {
            return x;
        }

        let drive_linear = 10f32.powf(drive_db / 20.0);
        let x_driven = x * drive_linear;

        let saturated = match mode {
            HarmonicsMode::Even => {
                // Asymmetric curve for even harmonics (Pentode style).
                // We use a subtle quadratic bias before a soft clip to generate 2nd order harmonics.
                let bias = 0.1 * (drive_linear - 1.0).max(0.0);
                ((x_driven + bias).tanh() - bias.tanh()) / drive_linear
            }
            HarmonicsMode::Odd => {
                // Symmetric curve for odd harmonics (Triode style).
                // Pure tanh creates 3rd, 5th, etc.
                x_driven.tanh() / drive_linear
            }
            HarmonicsMode::Mixed => {
                // Series processing: Even -> Odd
                let bias = 0.05 * (drive_linear - 1.0).max(0.0);
                let even = (x_driven + bias).tanh() - bias.tanh();
                even.tanh() / drive_linear
            }
        };

        // Parallel mix
        x * (1.0 - mix) + saturated * mix
    }
}

// =============================================================================

/// Mastering Clipper (bx_clipper inspired)
/// A peak shaver with mathematically perfect soft-knee bounding.
pub struct MasteringClipper;

impl MasteringClipper {
    /// Applies clipping to a single sample.
    /// `ceiling_linear`: The absolute maximum output level (e.g., 0.89 for -1.0 dBFS).
    /// `softness`: 0.0 (Hard Clip) to 1.0 (Very Soft Knee).
    #[inline]
    pub fn process(x: f32, ceiling_linear: f32, softness: f32) -> f32 {
        let abs_x = x.abs();

        if softness <= 0.0 {
            // Hard clip
            return x.clamp(-ceiling_linear, ceiling_linear);
        }

        // Soft clip using a quadratic knee.
        // Threshold T is the point where the knee starts.
        let t = ceiling_linear * (1.0 - softness);
        let w = ceiling_linear - t; // Width of the knee

        if abs_x <= t {
            // Linear region
            x
        } else if abs_x >= t + 2.0 * w {
            // Fully clipped region
            x.signum() * ceiling_linear
        } else {
            // Knee region
            let over = abs_x - t;
            let y = t + over - (over * over) / (4.0 * w);
            x.signum() * y
        }
    }
}

// =============================================================================

/// A feedforward stereo compressor with envelope ballistics and soft knee
#[derive(Clone, Default)]
pub struct Compressor {
    envelope: f32,
    sample_rate: f32,
}

impl Compressor {
    pub fn new() -> Self {
        Self {
            envelope: 0.0,
            sample_rate: 44100.0,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        l: f32,
        r: f32,
        threshold_db: f32,
        mix_percent: f32,
        attack_ms: f32,
        release_ms: f32,
        ratio: f32,
        knee_db: f32,
        gain_reduction_db_out: &mut f32,
    ) -> (f32, f32) {
        let signal_level = (l.abs() + r.abs()) * 0.5;

        // Envelope tracking
        let att_coef = (-1.0 / (attack_ms.max(0.1) * 0.001 * self.sample_rate)).exp();
        let rel_coef = (-1.0 / (release_ms.max(1.0) * 0.001 * self.sample_rate)).exp();

        let env_in = signal_level;
        self.envelope = if env_in > self.envelope {
            att_coef * self.envelope + (1.0 - att_coef) * env_in
        } else {
            rel_coef * self.envelope + (1.0 - rel_coef) * env_in
        };

        let env_db = if self.envelope < 1e-5 {
            -100.0
        } else {
            20.0 * self.envelope.log10()
        };

        // Gain reduction calculation (with soft knee)
        let mut gr_db = 0.0;
        if env_db > threshold_db - knee_db * 0.5 {
            if env_db < threshold_db + knee_db * 0.5 {
                let x = env_db - threshold_db + knee_db * 0.5;
                let knee_gr = x * x / (2.0 * knee_db);
                gr_db = knee_gr * (1.0 - 1.0 / ratio);
            } else {
                gr_db = (env_db - threshold_db) * (1.0 - 1.0 / ratio);
            }
        }

        *gain_reduction_db_out = gr_db;

        let comp_gain = 10.0f32.powf(-gr_db / 20.0);
        let mix = mix_percent / 100.0;

        // Report effective gain reduction AFTER wet/dry mix so the meter
        // reflects what actually reaches the output (mix=0 → 0 dB GR).
        let effective_linear_gain = 1.0 - mix + comp_gain * mix;
        *gain_reduction_db_out = if effective_linear_gain < 1.0 {
            -20.0 * effective_linear_gain.max(1e-10).log10()
        } else {
            0.0
        };

        let out_l = l * (1.0 - mix + comp_gain * mix);
        let out_r = r * (1.0 - mix + comp_gain * mix);

        (out_l, out_r)
    }
}

// =============================================================================

#[derive(Clone, Default)]
pub struct TwoBandCompressor {
    pub low_comp: Compressor,
    pub high_comp: Compressor,
    pub xo_l: LR2Crossover,
    pub xo_r: LR2Crossover,
    sample_rate: f32,
}

impl TwoBandCompressor {
    pub fn new() -> Self {
        Self {
            low_comp: Compressor::new(),
            high_comp: Compressor::new(),
            xo_l: LR2Crossover::new(),
            xo_r: LR2Crossover::new(),
            sample_rate: 44100.0,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr;
        self.low_comp.set_sample_rate(sr);
        self.high_comp.set_sample_rate(sr);
    }

    pub fn set_split_freq(&mut self, freq: f32) {
        self.xo_l.set_cutoff(freq, self.sample_rate);
        self.xo_r.set_cutoff(freq, self.sample_rate);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        l: f32,
        r: f32,
        t_lo: f32,
        t_hi: f32,
        ratio: f32,
        att: f32,
        rel: f32,
        mix: f32,
        gr_lo_out: &mut f32,
        gr_hi_out: &mut f32,
    ) -> (f32, f32) {
        let (lo_l, hi_l) = self.xo_l.process(l);
        let (lo_r, hi_r) = self.xo_r.process(r);

        let (comp_lo_l, comp_lo_r) = self
            .low_comp
            .process(lo_l, lo_r, t_lo, 100.0, att, rel, ratio, 4.0, gr_lo_out);
        let (comp_hi_l, comp_hi_r) = self
            .high_comp
            .process(hi_l, hi_r, t_hi, 100.0, att, rel, ratio, 4.0, gr_hi_out);

        let wet_l = comp_lo_l + comp_hi_l;
        let wet_r = comp_lo_r + comp_hi_r;

        let mix_factor = mix / 100.0;
        (
            l * (1.0 - mix_factor) + wet_l * mix_factor,
            r * (1.0 - mix_factor) + wet_r * mix_factor,
        )
    }
}

// =============================================================================

#[derive(Clone, Default)]
pub struct MsEq {
    pub mid_bands: [Biquad; 4],
    pub side_bands: [Biquad; 4],
    sample_rate: f32,
}

impl MsEq {
    pub fn new() -> Self {
        Self {
            mid_bands: [Biquad::new(), Biquad::new(), Biquad::new(), Biquad::new()],
            side_bands: [Biquad::new(), Biquad::new(), Biquad::new(), Biquad::new()],
            sample_rate: 44100.0,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr;
    }

    pub fn process(&mut self, mid: f32, side: f32) -> (f32, f32) {
        let mut m = mid;
        for b in &mut self.mid_bands {
            m = b.process(m);
        }

        let mut s = side;
        for b in &mut self.side_bands {
            s = b.process(s);
        }

        (m, s)
    }
}

// =============================================================================

#[derive(Clone, Default)]
pub struct SweeteningEq {
    pub hpf: Biquad,
    pub lpf: Biquad,
    pub lo_shelf: Biquad,
    pub hi_shelf: Biquad,
    sample_rate: f32,
}

impl SweeteningEq {
    pub fn new() -> Self {
        Self {
            hpf: Biquad::new(),
            lpf: Biquad::new(),
            lo_shelf: Biquad::new(),
            hi_shelf: Biquad::new(),
            sample_rate: 44100.0,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr;
    }

    pub fn process(&mut self, x: f32) -> f32 {
        let mut y = x;
        y = self.hpf.process(y);
        y = self.lpf.process(y);
        y = self.lo_shelf.process(y);
        y = self.hi_shelf.process(y);
        y
    }
}

// =============================================================================
// Auto Loud Meter — LUFS-based (EBU R128 / ITU-R BS.1770-4)
//
// Uses ebur128 Mode::S (short-term, 3s sliding window, ungated) with built-in
// K-weighting. LUFS measures perceived loudness — it correctly weights bass
// (K-filter) and is far less affected by peak-shaving from saturators than a
// raw peak meter. Ideal for gain-matching when dynamics processors (compressor,
// saturation) are in the signal chain.
// =============================================================================

use ebur128::{EbuR128, Mode};

/// Auto-Loud true-peak ceiling in dBTP. The make-up gain is clamped so the
/// measured inter-sample peak never crosses this level. −1.0 dBTP is the
/// conventional safe ceiling, leaving headroom for downstream lossy codecs.
pub const DBTP_CEILING: f32 = -1.0;

pub struct AutoLoudMeter {
    analyzer: EbuR128,
    fed_samples: u64,
}

impl AutoLoudMeter {
    /// `sample_rate` is f32 to match `nih_plug::util::Psafe`-style call sites.
    pub fn new(sample_rate: f32) -> Self {
        // Mode::S for short-term LUFS + Mode::TRUE_PEAK for ITU-R BS.1770-4
        // inter-sample (4x oversampled) true-peak metering. The true-peak read
        // backs the Auto-Loud safety clamp so a static make-up gain can never
        // push inter-sample peaks past the ceiling.
        let mut analyzer = EbuR128::new(2, sample_rate as u32, Mode::S | Mode::TRUE_PEAK)
            .expect("ebur128: failed to create AutoLoudMeter");
        // Enough headroom for a 5-second measurement window.
        let _ = analyzer.set_max_window(6000);
        Self {
            analyzer,
            fed_samples: 0,
        }
    }

    /// Feed one buffer of planar stereo audio.
    #[inline]
    pub fn feed(&mut self, left: &[f32], right: &[f32]) {
        let _ = self.analyzer.add_frames_planar_f32(&[left, right]);
        self.fed_samples += left.len().min(right.len()) as u64;
    }

    /// Reset internal state for a new measurement cycle.
    pub fn reset(&mut self) {
        self.analyzer.reset();
        self.fed_samples = 0;
    }

    /// Short-term LUFS (3 s sliding window, ungated, K-weighted).
    /// Returns a conservative -70 dB floor when not enough data is available.
    pub fn loudness_db(&self) -> f32 {
        self.analyzer.loudness_shortterm().unwrap_or(-70.0) as f32
    }

    /// Maximum inter-sample true peak over both channels since the last reset,
    /// in dBTP (ITU-R BS.1770-4, 4x oversampled). Returns a -100 dB floor when
    /// no true-peak data is available. Use this for the Auto-Loud safety clamp:
    /// because Auto-Loud applies a *static* linear make-up gain, scaling the
    /// output by `(ceiling_dbtp - this)` guarantees no inter-sample peak crosses
    /// the ceiling — a gapless dBTP guarantee for the measured window.
    pub fn true_peak_db(&self) -> f32 {
        let tp = self
            .analyzer
            .true_peak(0)
            .unwrap_or(0.0)
            .max(self.analyzer.true_peak(1).unwrap_or(0.0)) as f32;
        if tp < 1e-10 {
            -100.0
        } else {
            20.0 * tp.log10()
        }
    }

    pub fn sample_count(&self) -> u64 {
        self.fed_samples
    }
}

// =============================================================================
// Mastering Meter — integrated LUFS + true-peak readout (no auto-gain)
// =============================================================================

/// Passive EBU R128 meter for mastering delivery checks.
/// Integrated (gated) loudness, LRA, and ITU true-peak hold since last [`reset`].
pub struct MasteringMeter {
    analyzer: EbuR128,
    fed_samples: u64,
    true_peak_hold: f32,
}

impl MasteringMeter {
    pub fn new(sample_rate: f32) -> Self {
        let mut analyzer = EbuR128::new(
            2,
            sample_rate as u32,
            Mode::I | Mode::LRA | Mode::TRUE_PEAK,
        )
        .expect("ebur128: failed to create MasteringMeter");
        let _ = analyzer.set_max_window(120_000);
        Self {
            analyzer,
            fed_samples: 0,
            true_peak_hold: -100.0,
        }
    }

    #[inline]
    pub fn feed(&mut self, left: &[f32], right: &[f32]) {
        let n = left.len().min(right.len());
        if n == 0 {
            return;
        }
        let _ = self.analyzer.add_frames_planar_f32(&[left, right]);
        self.fed_samples += n as u64;
        let tp = self.true_peak_instant_db();
        if tp > self.true_peak_hold {
            self.true_peak_hold = tp;
        }
    }

    pub fn reset(&mut self) {
        self.analyzer.reset();
        self.fed_samples = 0;
        self.true_peak_hold = -100.0;
    }

    /// Gated integrated loudness (LUFS) — export reference.
    pub fn loudness_integrated_db(&self) -> f32 {
        self.analyzer.loudness_global().unwrap_or(-70.0) as f32
    }

    /// True-peak hold (dBTP) since last reset.
    pub fn true_peak_db(&self) -> f32 {
        self.true_peak_hold
    }

    /// EBU R128 loudness range (LRA) in LU — needs enough programme audio.
    pub fn loudness_range_lu(&self) -> Option<f32> {
        self.analyzer
            .loudness_range()
            .ok()
            .map(|v| v as f32)
    }

    fn true_peak_instant_db(&self) -> f32 {
        let tp = self
            .analyzer
            .true_peak(0)
            .unwrap_or(0.0)
            .max(self.analyzer.true_peak(1).unwrap_or(0.0)) as f32;
        if tp < 1e-10 {
            -100.0
        } else {
            20.0 * tp.log10()
        }
    }

    pub fn sample_count(&self) -> u64 {
        self.fed_samples
    }
}

// =============================================================================
// Simple RMS Loudness Meter — for Auto Loud
// =============================================================================

/// Minimal RMS energy accumulator. No K-weighting, no gating, no external crate
/// complexity. Just sum of squared samples → dB. Used by Auto Loud to match
/// what the user sees on a peak meter.
pub struct RmsMeter {
    sum_sq: f64,
    count: u64,
}

impl Default for RmsMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl RmsMeter {
    pub fn new() -> Self {
        Self {
            sum_sq: 0.0,
            count: 0,
        }
    }

    /// Feed one buffer of planar stereo audio.
    #[inline]
    pub fn feed(&mut self, left: &[f32], right: &[f32]) {
        let n = left.len().min(right.len());
        let mut s = 0.0f64;
        for i in 0..n {
            let l = left[i] as f64;
            let r = right[i] as f64;
            s += l * l + r * r;
        }
        self.sum_sq += s;
        self.count += n as u64;
    }

    pub fn reset(&mut self) {
        self.sum_sq = 0.0;
        self.count = 0;
    }

    /// Per-channel RMS level in dB.
    pub fn rms_db(&self) -> f32 {
        if self.count == 0 {
            return -100.0;
        }
        let mean_sq = self.sum_sq / (self.count as f64 * 2.0);
        if mean_sq < 1e-20 {
            -100.0
        } else {
            10.0 * (mean_sq as f32).log10()
        }
    }

    pub fn sample_count(&self) -> u64 {
        self.count
    }
}

// =============================================================================
// Simple Peak Meter — for Auto Loud (peak-matching)
// =============================================================================

/// Tracks the maximum absolute sample over the fed window. Mirrors `RmsMeter`'s
/// interface so Auto Loud can swap metrics cleanly. Matches what the user sees
/// on the plugin's peak meter.
pub struct PeakMeter {
    peak: f32,
    count: u64,
}

impl Default for PeakMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl PeakMeter {
    pub fn new() -> Self {
        Self {
            peak: 0.0,
            count: 0,
        }
    }

    /// Feed one buffer of planar stereo audio.
    #[inline]
    pub fn feed(&mut self, left: &[f32], right: &[f32]) {
        let n = left.len().min(right.len());
        let mut p = self.peak;
        for i in 0..n {
            let a = left[i].abs();
            let b = right[i].abs();
            if a > p {
                p = a;
            }
            if b > p {
                p = b;
            }
        }
        self.peak = p;
        self.count += n as u64;
    }

    pub fn reset(&mut self) {
        self.peak = 0.0;
        self.count = 0;
    }

    /// Peak level in dBFS.
    pub fn peak_db(&self) -> f32 {
        if self.peak < 1e-10 {
            -100.0
        } else {
            20.0 * self.peak.log10()
        }
    }

    pub fn sample_count(&self) -> u64 {
        self.count
    }
}

// =============================================================================
// M/S MULTIBAND LIMITER
// Three independent limiters: Mid-Lo, Mid-Hi (split via LR2Crossover), Side.
// Each band uses peak-envelope detection with ballistic attack/release.
// Instantaneous gain-snap on attack, exponential recovery on release.
// =============================================================================

#[derive(Clone)]
struct BandLimiter {
    envelope: f32,
    gain: f32,
}

impl BandLimiter {
    fn new() -> Self {
        Self {
            envelope: 0.0,
            gain: 1.0,
        }
    }

    #[inline]
    fn process(
        &mut self,
        x: f32,
        threshold: f32,
        att_coeff: f32,
        rel_coeff: f32,
        makeup: f32,
        classic_mode: bool,
    ) -> f32 {
        let level = x.abs();

        if classic_mode {
            // Smooth RMS-style envelope — gentler, more transparent
            self.envelope = if level > self.envelope {
                att_coeff * self.envelope + (1.0 - att_coeff) * level
            } else {
                rel_coeff * self.envelope + (1.0 - rel_coeff) * level
            };
        } else {
            // Peak envelope with fast attack — Modern mode
            self.envelope = if level > self.envelope {
                level // instantaneous peak catch
            } else {
                rel_coeff * self.envelope + (1.0 - rel_coeff) * level
            };
        }

        let required = if self.envelope > threshold && threshold > 1e-10 {
            threshold / self.envelope
        } else {
            1.0_f32
        };

        // Instantaneous attack (snap down), smoothed release (drift up)
        self.gain = self.gain.min(required);
        self.gain = (self.gain * rel_coeff + (1.0 - rel_coeff)).min(1.0);

        x * self.gain * makeup
    }

    fn reset(&mut self) {
        self.envelope = 0.0;
        self.gain = 1.0;
    }
}

#[derive(Clone)]
pub struct MsBandLimiter {
    xo_mid: LR2Crossover,
    lim_mid_lo: BandLimiter,
    lim_mid_hi: BandLimiter,
    lim_side: BandLimiter,
    sample_rate: f32,
}

impl Default for MsBandLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl MsBandLimiter {
    pub fn new() -> Self {
        Self {
            xo_mid: LR2Crossover::new(),
            lim_mid_lo: BandLimiter::new(),
            lim_mid_hi: BandLimiter::new(),
            lim_side: BandLimiter::new(),
            sample_rate: 44100.0,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr;
    }

    pub fn set_crossover(&mut self, freq: f32) {
        self.xo_mid.set_cutoff(freq, self.sample_rate);
    }

    pub fn reset(&mut self) {
        self.lim_mid_lo.reset();
        self.lim_mid_hi.reset();
        self.lim_side.reset();
    }

    /// Process one M/S sample pair through all three band limiters.
    ///
    /// Returns `(mid_out, side_out, gr_mid_lo_db, gr_mid_hi_db, gr_side_db)`.
    /// All threshold/gain params are in **linear** domain — convert dB before calling.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn process_ms(
        &mut self,
        mid: f32,
        side: f32,
        thresh_mid_lo: f32,
        thresh_mid_hi: f32,
        thresh_side: f32,
        att_mid_lo: f32,
        att_mid_hi: f32,
        att_side: f32,
        rel_mid_lo: f32,
        rel_mid_hi: f32,
        rel_side: f32,
        makeup_mid_lo: f32,
        makeup_mid_hi: f32,
        makeup_side: f32,
        global_makeup: f32,
        classic_mode: bool,
    ) -> (f32, f32) {
        // Split mid into lo / hi via Linkwitz-Riley crossover
        let (mid_lo, mid_hi) = self.xo_mid.process(mid);

        let mid_lo_out = self.lim_mid_lo.process(
            mid_lo,
            thresh_mid_lo,
            att_mid_lo,
            rel_mid_lo,
            makeup_mid_lo,
            classic_mode,
        );
        let mid_hi_out = self.lim_mid_hi.process(
            mid_hi,
            thresh_mid_hi,
            att_mid_hi,
            rel_mid_hi,
            makeup_mid_hi,
            classic_mode,
        );
        let side_out = self.lim_side.process(
            side,
            thresh_side,
            att_side,
            rel_side,
            makeup_side,
            classic_mode,
        );

        (
            (mid_lo_out + mid_hi_out) * global_makeup,
            side_out * global_makeup,
        )
    }

    /// Current gain reduction per band in dB (0.0 = no reduction, negative = active).
    pub fn gr_db(&self) -> (f32, f32, f32) {
        let to_db = |g: f32| if g <= 0.0 { -90.0 } else { 20.0 * g.log10() };
        (
            to_db(self.lim_mid_lo.gain),
            to_db(self.lim_mid_hi.gain),
            to_db(self.lim_side.gain),
        )
    }
}

// =============================================================================
// CPU floating-point helper: set FTZ/DAZ inside a block, restore MXCSR on exit.
// =============================================================================

/// RAII guard that enables Flush-To-Zero and Denormals-Are-Zero on x86_64
/// for the duration of the current scope and restores the previous MXCSR
/// value when dropped. On non-x86_64 targets it is a zero-cost no-op.
pub struct FtzDazGuard {
    #[cfg(target_arch = "x86_64")]
    old_csr: u32,
}

impl FtzDazGuard {
    /// Capture the current MXCSR, set FTZ/DAZ, and return the guard.
    #[allow(deprecated)]
    pub fn new() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            let old_csr = unsafe { std::arch::x86_64::_mm_getcsr() };
            unsafe { std::arch::x86_64::_mm_setcsr(old_csr | 0x8040) };
            Self { old_csr }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            Self {}
        }
    }
}

impl Default for FtzDazGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for FtzDazGuard {
    #[allow(deprecated)]
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            std::arch::x86_64::_mm_setcsr(self.old_csr);
        }
    }
}
