//! LX Audiolabs shared Slint UI — design tokens, chrome, meters, and viz.
//!
//! Plugin crates import the `.slint` files directly:
//! ```text
//! import { Lx, LxHeader, LxKnob, ... } from "../../../crates/lx-ui-slint/ui/lx.slint";
//! ```
//!
//! This crate compiles the gallery root so the module graph stays buildable
//! in isolation and can host future Rust path-builders / helpers.

slint::include_modules!();
