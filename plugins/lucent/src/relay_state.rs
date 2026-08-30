//! Port of dev lucent `ui/relay_state.rs` (Vizia) — editor-side relay list:
//! per-slot EMA smoothing (α = 1/6) and slot-stable active toggles feeding
//! `LucentShared::relay_active_mask`.

use crate::RelaySlot;
use lx_shm::SPECTRUM_BINS;

/// Single Relay feed as shown in the UI (from the Lucent-Relay plugins).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RelayUi {
    /// SHM publisher slot — stable identity even when labels collide.
    pub slot: u8,
    pub name: String,
    /// EMA-smoothed FFT bins in dB.
    pub spectrum: Vec<f32>,
    pub active: bool,
}

/// Editor relay list — manages the live relay feeds + user toggles.
#[derive(Default)]
pub(crate) struct RelayState {
    pub relays: Vec<RelayUi>,
}

impl RelayState {
    /// Clear all relay data (Standalone mode — no relay interaction).
    pub fn clear(&mut self) {
        self.relays.clear();
    }

    /// Replace the relay list with live feeds published from `process()`,
    /// preserving the user's per-relay active toggle (matched by SHM slot).
    /// Applies EMA smoothing (α = 1/6) so relay spectra don't jump
    /// frame-to-frame.
    pub fn sync(&mut self, feeds: &[RelaySlot]) {
        let alpha: f32 = 1.0 / 6.0; // ~100 ms smoothing at ~17 FFT frames/s
        let new_relays = feeds
            .iter()
            .map(|f| {
                let prev = self.relays.iter().find(|r| r.slot == f.slot);
                let active = prev.map(|r| r.active).unwrap_or(true);
                let spectrum = if let Some(p) = prev {
                    if p.spectrum.len() == SPECTRUM_BINS {
                        p.spectrum
                            .iter()
                            .zip(f.bins.iter())
                            .map(|(&p, &s)| p * (1.0 - alpha) + s * alpha)
                            .collect()
                    } else {
                        f.bins.to_vec()
                    }
                } else {
                    f.bins.to_vec()
                };
                RelayUi {
                    slot: f.slot,
                    name: f.name.clone(),
                    spectrum,
                    active,
                }
            })
            .collect();
        self.relays = new_relays;
    }

    /// Bitmask for `LucentShared::relay_active_mask` (bit `i` = slot `i` on).
    pub fn active_mask(&self) -> u32 {
        let mut mask = 0u32;
        for r in &self.relays {
            if r.active {
                mask |= 1u32 << r.slot;
            }
        }
        mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(slot: u8, name: &str, v: f32) -> RelaySlot {
        RelaySlot {
            slot,
            name: name.to_string(),
            bins: [v; SPECTRUM_BINS],
        }
    }

    #[test]
    fn sync_relays_applies_ema_smoothing() {
        let mut ui = RelayState::default();
        ui.sync(&[feed(0, "Kick", -40.0)]);
        ui.sync(&[feed(0, "Kick", -34.0)]);
        let alpha = 1.0 / 6.0;
        let expect = -40.0 * (1.0 - alpha) + -34.0 * alpha;
        assert!(
            (ui.relays[0].spectrum[0] - expect).abs() < 1e-4,
            "EMA α=1/6 expected {expect}, got {}",
            ui.relays[0].spectrum[0]
        );
    }

    #[test]
    fn sync_relays_preserves_active_toggle_by_slot() {
        let mut ui = RelayState::default();
        ui.sync(&[feed(0, "Kick", -40.0)]);
        ui.relays[0].active = false;

        ui.sync(&[feed(0, "Kick Renamed", -30.0)]);
        assert!(!ui.relays[0].active);
        assert_eq!(ui.relays[0].name, "Kick Renamed");
    }

    #[test]
    fn relay_active_mask_reflects_toggles() {
        let mut ui = RelayState::default();
        ui.sync(&[feed(0, "A", -90.0), feed(2, "B", -90.0)]);
        ui.relays[0].active = false;
        assert_eq!(ui.active_mask(), 1u32 << 2);
    }
}
