//! Meter formatting and normalization helpers.

/// Map peak dB → 0..1 over the standard LX LED peak meter range (−60 .. +6 dB).
#[inline]
pub fn db_to_meter(db: f32) -> f32 {
    ((db + 60.0) / 66.0).clamp(0.0, 1.0)
}

/// Alias for [`db_to_meter`] that preserves the old `peak_norm` call sites.
#[inline]
pub fn peak_norm(db: f32) -> f32 {
    db_to_meter(db)
}

/// Format a dB value for readouts: "-inf" below −60 dB, otherwise one decimal.
pub fn fmt_db(v: f32) -> String {
    if v <= -60.0 {
        "-inf".into()
    } else {
        format!("{v:.1}")
    }
}
