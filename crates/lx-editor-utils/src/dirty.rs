//! Dirty-checking helpers for host→UI mirroring.
//!
//! Only call the Slint setter when the value actually changed, avoiding
//! redundant UI updates on every tick.

/// Update a cached `f32` and return `true` if it changed.
#[inline]
pub fn changed_f32(prev: &mut f32, v: f32) -> bool {
    if *prev != v {
        *prev = v;
        true
    } else {
        false
    }
}

/// Update a cached `Option<bool>` and return `true` if it changed.
/// `None` as previous value means "never pushed" and always applies.
#[inline]
pub fn changed_bool(prev: &mut Option<bool>, v: bool) -> bool {
    if *prev != Some(v) {
        *prev = Some(v);
        true
    } else {
        false
    }
}

/// Update a cached `String` and return `true` if it changed.
#[inline]
pub fn changed_str(prev: &mut String, v: &str) -> bool {
    if prev.as_str() != v {
        prev.clear();
        prev.push_str(v);
        true
    } else {
        false
    }
}

/// Update a cached `i32` and return `true` if it changed.
#[inline]
pub fn changed_i32(prev: &mut i32, v: i32) -> bool {
    if *prev != v {
        *prev = v;
        true
    } else {
        false
    }
}
