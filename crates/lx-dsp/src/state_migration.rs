// nice-plug → truce State Migration
//
// nice-plug serialized plugin state as JSON:
//   {"version": X, "params": {"param_name": value, ...}}
//
// truce uses a binary envelope with magic bytes. When a DAW session saved
// with a nice-plug build is loaded by the truce build, the binary parser
// rejects the JSON blob. The host then falls back to defaults — silent
// data loss for users.
//
// This module provides a fallback: detect nice-plug JSON in load_state(),
// parse it, extract parameter values, and write them back through the
// normal truce param API. Old sessions "just work".

use serde_json::Value;

/// Try to parse a nice-plug JSON state blob.
///
/// Returns `Some(params)` if the data (optionally after nice-plug's own
/// 8-byte length header) parses as a nice-plug state object with a
/// `"params"` key. Returns `None` if the data does not look like
/// nice-plug JSON (truce binary or empty).
///
/// The returned map has parameter IDs (snake_case, matching the Rust
/// param struct field names, e.g. "bass_gain") as keys and raw values
/// (not 0.0-1.0 normalized) as `f64`. Each nice-plug value is itself a
/// single-key type-tagged object - `{"f32": 0.0}`, `{"bool": false}`,
/// `{"i32": 2}` - not a bare number, so each entry is unwrapped one
/// level before conversion.
pub fn try_parse_niceplug_state(data: &[u8]) -> Option<Vec<(String, f64)>> {
    // nice-plug's own CLAP state_save() writes `[u64 LE length][json
    // bytes]`, not a bare JSON blob - the length header sits in front
    // of the '{'. Confirmed against a real saved nice-plug Equilibrium
    // chunk: first 8 bytes decoded to a u64 that exactly matched the
    // remaining byte count, with the JSON starting right after.
    let json_bytes = if data.first() == Some(&b'{') {
        data
    } else if data.len() >= 8 {
        let len = u64::from_le_bytes(data[0..8].try_into().ok()?);
        let rest = &data[8..];
        if u64::try_from(rest.len()).ok()? == len && rest.first() == Some(&b'{') {
            rest
        } else {
            return None;
        }
    } else {
        return None;
    };

    let root: Value = serde_json::from_slice(json_bytes).ok()?;
    let params = root.get("params")?.as_object()?;

    let mut result = Vec::with_capacity(params.len());
    for (name, val) in params {
        // Unwrap the type tag: {"f32": 0.0} / {"bool": false} / {"i32": 2}
        // -> 0.0 / false / 2. Values that are already bare numbers (not
        // observed in practice, but harmless) pass through untouched.
        let inner = match val.as_object() {
            Some(map) if map.len() == 1 => map.values().next()?,
            _ => val,
        };
        let v = if let Some(f) = inner.as_f64() {
            f
        } else if let Some(b) = inner.as_bool() {
            f64::from(b)
        } else {
            continue;
        };
        result.push((name.clone(), v));
    }

    Some(result)
}
