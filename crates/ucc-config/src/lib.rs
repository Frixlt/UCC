//! Configuration crate for Frixit Control Center.
//!
//! Handles:
//! - Loading and atomic saving of settings (`/etc/ucc/settings.json`, `~/.config/ucc/settings.json`).
//! - Standard Built-in profiles (Quiet, Balanced, Performance) with canonical IDs.
//! - Device-specific default profile catalogs.
//! - Safe atomic file writes with `.bak` backup creation and permissions checking.
//! - Read-only preview mode enforcement (`UCC_READ_ONLY_PREVIEW`).
//! - Legacy TccSettings and SettingsManager round-trip compatibility.

pub mod defaults;
pub mod error;
pub mod legacy;
pub mod manager;
pub mod persistence;
pub mod settings;

pub use defaults::*;
pub use error::*;
pub use legacy::*;
pub use manager::*;
pub use persistence::*;
pub use settings::*;
pub use ucc_core::*;
