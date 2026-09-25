//! Flutter FFI adapter for the TsinghuaKit SDK.
//!
//! The protocol implementation and curated Rust API are dependencies. This
//! crate keeps the generated bridge at the actual FFI boundary and re-exports
//! the legacy bridge surface during the application migration.

#![allow(unknown_lints)]
#![allow(unexpected_cfgs)]
#![recursion_limit = "256"]

#[cfg(feature = "ffi-bridge")]
#[rustfmt::skip]
mod frb_generated;

pub use tsinghua_kit_engine::*;
/// Curated public SDK exposed to native bridge adapters.
pub use tsinghua_kit_sdk as sdk;

/// Local DTOs and opaque handles used to adapt the public SDK to FRB.
pub mod sdk_api;
