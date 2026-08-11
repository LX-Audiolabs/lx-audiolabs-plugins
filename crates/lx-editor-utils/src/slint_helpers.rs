//! Small Slint-specific utilities.

use slint::{ModelRc, SharedString, VecModel};

/// Convert a `&[String]` into a Slint `ModelRc<SharedString>`.
pub fn names_model(names: &[String]) -> ModelRc<SharedString> {
    let v: Vec<SharedString> = names.iter().map(|s| SharedString::from(s.as_str())).collect();
    ModelRc::new(VecModel::from(v))
}

/// Parse a user-typed floating point value, accepting both comma and dot as
/// decimal separator.
pub fn parse_f32(s: &str) -> Option<f32> {
    s.trim().replace(',', ".").parse::<f32>().ok()
}
