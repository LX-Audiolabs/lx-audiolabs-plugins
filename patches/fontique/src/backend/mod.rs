// Copyright 2024 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! System backends.

#[cfg(all(feature = "system", target_os = "windows"))]
#[path = "dwrite.rs"]
mod system;

#[cfg(all(feature = "system", target_vendor = "apple"))]
#[path = "coretext.rs"]
mod system;

// ponytail: fontconfig backend requires opt-in "fontconfig" feature.
// Disabled by default → cross-compiles from Windows without Linux sysroot.
#[cfg(all(feature = "system", feature = "fontconfig", target_os = "linux"))]
#[path = "fontconfig.rs"]
mod system;

#[cfg(all(feature = "system", target_os = "android"))]
#[path = "android.rs"]
mod system;

#[allow(unused_imports)]
use super::{
    FallbackKey, FamilyId, FamilyInfo, FontInfo, GenericFamily, Script, SourceInfo,
    family_name::{FamilyName, FamilyNameMap},
    generic::GenericFamilyMap,
    scan,
};

#[cfg(feature = "std")]
#[allow(unused_imports)]
use super::source::SourcePathMap;

pub(crate) use system::SystemFonts;

// Dummy system font backend for targets like wasm32-unknown-unknown,
// and for Linux when the "fontconfig" feature is not enabled
// (ponytail: cross-compile from Windows without Linux sysroot).
#[cfg(any(
    not(feature = "system"),
    not(any(
        target_os = "windows",
        target_os = "android",
        target_vendor = "apple"
    )),
    all(target_os = "linux", not(feature = "fontconfig"))
))]
mod system {
    #[cfg(feature = "system")]
    use super::{FallbackKey, FamilyId, FamilyInfo};
    use super::{FamilyNameMap, GenericFamilyMap};
    use alloc::sync::Arc;

    #[derive(Default)]
    pub(crate) struct SystemFonts {
        pub(crate) name_map: Arc<FamilyNameMap>,
        pub(crate) generic_families: Arc<GenericFamilyMap>,
    }

    impl SystemFonts {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        #[cfg(feature = "system")]
        pub(crate) fn family(&mut self, _id: FamilyId) -> Option<FamilyInfo> {
            None
        }

        #[cfg(feature = "system")]
        pub(crate) fn fallback(&mut self, _key: impl Into<FallbackKey>) -> Option<FamilyId> {
            None
        }
    }
}
