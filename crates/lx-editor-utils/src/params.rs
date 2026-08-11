//! Parameter binding macros and helpers for `aura-editor` / Slint plugins.
//!
//! The macros use `$crate::paste::paste!` internally so consuming crates do
//! not need a direct `paste` dependency.

pub use aura::FloatParam;

/// Normalized default value of a `FloatParam` (respects log/linear/skew).
pub fn float_default_norm(p: &FloatParam) -> f32 {
    p.info.range.normalize(p.info.default_plain) as f32
}

/// Normalize a discrete choice index to `[0, 1]`.
///
/// `count` is the total number of discrete steps. Returns `0.0` for the first
/// step and `1.0` for the last. Safe for `count <= 1`.
pub fn discrete_norm(index: usize, count: usize) -> f64 {
    if count <= 1 {
        0.0
    } else {
        (index.min(count - 1) as f64) / ((count - 1) as f64)
    }
}

/// Convert a normalized value back to a discrete choice index.
pub fn discrete_index(norm: f64, count: usize) -> usize {
    if count <= 1 {
        0
    } else {
        (norm * (count - 1) as f64).round() as usize
    }
}

/// Bind Slint callbacks to float parameter automation.
#[macro_export]
macro_rules! bind_floats {
    ($ui:expr, $state:expr, $($p:expr => $name:ident),* $(,)?) => {
        $(
            $crate::paste::paste! {
                let s = $state.clone();
                $ui.[<on_ $name _changed>](move |v| s.automate($p, v as f64));
            }
        )*
    };
}

/// Bind Slint callbacks to bool parameter automation.
#[macro_export]
macro_rules! bind_bools {
    ($ui:expr, $state:expr, $($p:expr => $name:ident),* $(,)?) => {
        $(
            $crate::paste::paste! {
                let s = $state.clone();
                $ui.[<on_ $name _changed>](move |v: bool| {
                    s.automate($p, if v { 1.0 } else { 0.0 });
                });
            }
        )*
    };
}

/// Bind Slint callbacks to discrete int parameter automation.
#[macro_export]
macro_rules! bind_ints {
    ($ui:expr, $state:expr, $count:expr, $($p:expr => $name:ident),* $(,)?) => {
        $(
            $crate::paste::paste! {
                let s = $state.clone();
                let count = $count as usize;
                $ui.[<on_ $name _changed>](move |v: f32| {
                    s.automate($p, $crate::params::discrete_norm(v.max(0.0) as usize, count));
                });
            }
        )*
    };
}

/// Set the Slint `*_default` properties used for right-click reset.
#[macro_export]
macro_rules! set_float_defaults {
    ($ui:expr, $params:expr, $($name:ident),* $(,)?) => {
        $(
            $crate::paste::paste! {
                $ui.[<set_ $name _default>]($crate::params::float_default_norm(&$params.$name));
            }
        )*
    };
}

/// Reset multiple float parameters to their param defaults.
#[macro_export]
macro_rules! reset_floats {
    ($state:expr, $params:expr, $($p:expr => $name:ident),* $(,)?) => {
        $(
            $state.automate($p, $crate::params::float_default_norm(&$params.$name) as f64);
        )*
    };
}

/// Dirty host→UI float push. Does **not** update the `*_text` property.
#[macro_export]
macro_rules! sync_floats_dirty {
    ($ui:expr, $state:expr, $cache:expr, $($idx:expr, $p:expr => $name:ident),* $(,)?) => {
        $(
            $crate::paste::paste! {
                let v = ::aura_editor::typed::PluginContextReadF32::get_param($state, $p);
                if $crate::dirty::changed_f32(&mut $cache.floats[$idx], v) {
                    $ui.[<set_ $name>](v);
                }
            }
        )*
    };
}

/// Dirty host→UI float push that also updates the `*_text` property using
/// `PluginContext::format_param`.
#[macro_export]
macro_rules! sync_floats_dirty_with_text {
    ($ui:expr, $state:expr, $cache:expr, $($idx:expr, $p:expr => $name:ident),* $(,)?) => {
        $(
            $crate::paste::paste! {
                let v = ::aura_editor::typed::PluginContextReadF32::get_param($state, $p);
                if $crate::dirty::changed_f32(&mut $cache.floats[$idx], v) {
                    $ui.[<set_ $name>](v);
                    $ui.[<set_ $name _text>](::slint::SharedString::from($state.format_param($p)));
                }
            }
        )*
    };
}

/// Dirty host→UI int push.
#[macro_export]
macro_rules! sync_ints_dirty {
    ($ui:expr, $state:expr, $cache:expr, $count:expr, $($idx:expr, $p:expr => $name:ident),* $(,)?) => {
        $(
            $crate::paste::paste! {
                let value = $crate::params::discrete_index(
                    ::aura_editor::typed::PluginContextReadF32::get_param($state, $p) as f64,
                    $count,
                ) as f32;
                if $crate::dirty::changed_f32(&mut $cache.ints[$idx], value) {
                    $ui.[<set_ $name>](value);
                }
            }
        )*
    };
}

/// Dirty host→UI bool push.
#[macro_export]
macro_rules! sync_bools_dirty {
    ($ui:expr, $state:expr, $cache:expr, $($idx:expr, $p:expr => $name:ident),* $(,)?) => {
        $(
            $crate::paste::paste! {
                let v = ::aura_editor::typed::PluginContextReadF32::get_param($state, $p) > 0.5;
                if $crate::dirty::changed_bool(&mut $cache.bools[$idx], v) {
                    $ui.[<set_ $name>](v);
                }
            }
        )*
    };
}
