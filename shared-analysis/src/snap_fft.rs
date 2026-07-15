//! Central FFT + SNAP accumulation infrastructure
//! Used by Meridian, Equilibrium, and other spectrum-aware plugins

use crate::SPECTRUM_BINS;
use std::f32::consts::PI;

#[derive(Clone, Copy, Debug)]
pub enum SnapMode {
    Stereo = 0,
    Mono = 1,
    Delta = 2,
}

/// Pre-allocated FFT + SNAP state for real-time audio thread
pub struct SnapFFT {
    // FFT infrastructure (pre-allocated, no heap in audio thread)
    fft_planner: realfft::RealFftPlanner<f32>,
    pub fft_input: Vec<f32>,
    pub fft_write_pos: usize,
    fft_hann: Vec<f32>,
    fft_windowed: Vec<f32>,
    fft_output_cache: Vec<realfft::num_complex::Complex<f32>>,

    // SNAP accumulators (one per mode)
    snap_accum: [Vec<f32>; 3], // [Stereo, Mono, Delta]
    pub snap_fft_count: u32,
    pub snap_phase_prev: u8,
}

impl SnapFFT {
    /// Create and pre-allocate all buffers
    pub fn new() -> Self {
        let fft_size = SPECTRUM_BINS * 2;
        let n_bins = SPECTRUM_BINS;

        // Hann window
        let fft_hann = (0..fft_size)
            .map(|i| {
                let norm = i as f32 / fft_size as f32;
                let pi2 = 2.0 * PI;
                0.5 * (1.0 - (pi2 * norm).cos())
            })
            .collect::<Vec<_>>();

        Self {
            fft_planner: realfft::RealFftPlanner::new(),
            fft_input: vec![0.0f32; fft_size],
            fft_write_pos: 0,
            fft_hann,
            fft_windowed: vec![0.0f32; fft_size],
            fft_output_cache: vec![realfft::num_complex::Complex::new(0.0f32, 0.0f32); n_bins + 1],
            snap_accum: [
                vec![-90.0f32; SPECTRUM_BINS],
                vec![-90.0f32; SPECTRUM_BINS],
                vec![-90.0f32; SPECTRUM_BINS],
            ],
            snap_fft_count: 0,
            snap_phase_prev: 0,
        }
    }

    /// Process one sample into ring buffer; return true if FFT ready
    pub fn push_sample(&mut self, sample: f32) -> bool {
        self.fft_input[self.fft_write_pos] = sample;
        self.fft_write_pos += 1;

        if self.fft_write_pos >= self.fft_input.len() {
            self.fft_write_pos = 0;
            true // FFT ready
        } else {
            false
        }
    }

    /// Compute FFT and return frame (dB magnitudes).
    /// `sample_rate` must be the actual host sample rate — tilt is computed
    /// per bin so the pink-noise reference stays flat independent of sample rate.
    pub fn compute_fft(&mut self, sample_rate: f32) -> [f32; SPECTRUM_BINS] {
        // Window
        for i in 0..self.fft_input.len() {
            self.fft_windowed[i] = self.fft_input[i] * self.fft_hann[i];
        }

        // FFT
        let fft = self.fft_planner.plan_fft_forward(self.fft_input.len());
        let _ = fft.process(&mut self.fft_windowed, &mut self.fft_output_cache);

        // Convert to dB with per-bin tilt (4.5 dB/octave pink-noise compensation)
        let fft_size = self.fft_input.len() as f32;
        let inv_norm = 2.0 / fft_size;
        let mut frame = [-90.0f32; SPECTRUM_BINS];

        for (k, slot) in frame.iter_mut().enumerate() {
            let mag = self.fft_output_cache[k].norm() * inv_norm;
            let db = if mag > 1e-9 {
                20.0 * mag.log10()
            } else {
                -90.0
            };
            let freq = k as f32 * sample_rate / fft_size;
            let tilt = if freq > 20.0 {
                4.5 * (freq / 1000.0).log2()
            } else {
                0.0
            };
            *slot = (db + tilt).clamp(-90.0, 12.0);
        }

        frame
    }

    /// Accumulate frame into SNAP buffer
    /// Returns true when snapshot is complete (fft_count reaches threshold)
    pub fn accumulate_snap(
        &mut self,
        frame: &[f32; SPECTRUM_BINS],
        snap_phase: u8,
        threshold: u32,
    ) -> bool {
        // Determine which mode to accumulate
        let mode = match snap_phase {
            1 => SnapMode::Stereo,
            2 => SnapMode::Mono,
            3 => SnapMode::Delta,
            _ => return false,
        };

        // Phase change detection
        if snap_phase != self.snap_phase_prev {
            // Reset accumulator for new phase
            for v in self.snap_accum[mode as usize].iter_mut() {
                *v = -90.0;
            }
            self.snap_fft_count = 0;
            self.snap_phase_prev = snap_phase;
        }

        // EMA-style accumulation
        let alpha = 0.1; // ~10 frames to converge
        let accum = &mut self.snap_accum[mode as usize];
        for (k, &f) in frame.iter().enumerate() {
            accum[k] = accum[k] * (1.0 - alpha) + f * alpha;
        }

        self.snap_fft_count += 1;

        // Check if threshold reached
        if self.snap_fft_count >= threshold {
            self.snap_fft_count = 0;
            true
        } else {
            false
        }
    }

    /// Read snapshot for a given mode (thread-safe copy)
    pub fn read_snapshot(&self, mode: SnapMode) -> Vec<f32> {
        self.snap_accum[mode as usize].clone()
    }

    /// Clear all snapshot accumulators
    pub fn reset_snapshots(&mut self) {
        for accum in self.snap_accum.iter_mut() {
            accum.fill(-90.0);
        }
        self.snap_fft_count = 0;
        self.snap_phase_prev = 0;
    }

    /// Clear FFT ring buffer (called on silence gate)
    pub fn clear_fft_buffer(&mut self) {
        self.fft_input.fill(0.0);
        self.fft_write_pos = 0;
    }
}

impl Default for SnapFFT {
    fn default() -> Self {
        Self::new()
    }
}
