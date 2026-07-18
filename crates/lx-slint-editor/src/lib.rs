//! LX FemtoVG/OpenGL Slint editor for truce plugins.
//!
//! API mirrors `truce_slint` (`SlintEditor`, `PluginContext`, `SyncFn`, `bind!`)
//! so Aether/Meridian can switch runtime present without rewriting UI setup.

mod editor;
mod parent;
mod platform;
mod translate;

pub use editor::{SlintEditor, SyncFn};
pub use paste::paste;
pub use slint;
pub use truce_core;
pub use truce_core::editor::PluginContext;

/// Bind Slint properties to truce parameters (same surface as truce-slint).
#[macro_export]
macro_rules! bind {
    ($state:expr, $ui:expr, $( $id:expr => $name:ident $( : $ty:ident $(($arg:expr))? )? ),* $(,)?) => {{
        $(
            $crate::bind!(@wire $state, $ui, $id, $name $( : $ty $(($arg))? )?);
        )*
        let ui = $ui;
        Box::new(move |state: &$crate::PluginContext<_>| {
            $(
                $crate::bind!(@sync state, ui, $id, $name $( : $ty $(($arg))? )?);
            )*
        })
    }};

    (@wire $state:expr, $ui:expr, $id:expr, $name:ident) => {
        {
            let s = $state.clone();
            let id: u32 = $id.into();
            $crate::paste! {
                $ui.[<on_ $name _changed>](move |v| s.automate(id, v as f64));
            }
        }
    };
    (@sync $state:expr, $ui:expr, $id:expr, $name:ident) => {
        $crate::paste! {
            $ui.[<set_ $name>]($state.get_param($id.into()).to_f32());
        }
    };

    (@wire $state:expr, $ui:expr, $id:expr, $name:ident : bool) => {
        {
            let s = $state.clone();
            let id: u32 = $id.into();
            $crate::paste! {
                $ui.[<on_ $name _changed>](move |v: bool| {
                    s.automate(id, if v { 1.0 } else { 0.0 });
                });
            }
        }
    };
    (@sync $state:expr, $ui:expr, $id:expr, $name:ident : bool) => {
        $crate::paste! {
            $ui.[<set_ $name>]($state.get_param($id.into()) > 0.5);
        }
    };

    (@wire $state:expr, $ui:expr, $id:expr, $name:ident : choice($count:expr)) => {
        {
            let s = $state.clone();
            let id: u32 = $id.into();
            let count: u32 = $count;
            $crate::paste! {
                $ui.[<on_ $name _changed>](move |v: i32| {
                    let norm = $crate::truce_core::cast::discrete_norm(
                        v.max(0) as usize,
                        count as usize,
                    );
                    s.automate(id, norm);
                });
            }
        }
    };
    (@sync $state:expr, $ui:expr, $id:expr, $name:ident : choice($count:expr)) => {
        {
            let count: u32 = $count;
            let norm = $state.get_param($id.into()).to_f64();
            let idx = $crate::truce_core::cast::discrete_index(norm, count as usize) as i32;
            $crate::paste! {
                $ui.[<set_ $name>](idx);
            }
        }
    };
}
