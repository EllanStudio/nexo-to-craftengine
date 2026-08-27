//! Semantics-aware Nexo 1.26 → CraftEngine 26.8 converter core (Rust rewrite).
//!
//! The conversion is not a field rename: before converting, the rules are
//! checked against the Nexo implementation, the CraftEngine implementation
//! and Minecraft's resource-pack loading rules, so that a vanilla client
//! keeps the same display, collision, placement and interaction results.
//!
//! This crate is being ported module-by-module from `legacy/src/*.ts`.
//! Fields that cannot be represented equivalently are never guessed: the
//! converter omits the erroneous output and emits a `lossy` diagnostic.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod diagnostics;
pub mod json;
pub mod resource_location;

/// Locked conversion targets, mirrored from the legacy TypeScript core.
pub mod targets {
    /// Nexo version this converter is audited against.
    pub const NEXO_VERSION: &str = "1.26";
    /// CraftEngine version this converter is audited against.
    pub const CRAFTENGINE_VERSION: &str = "26.8";
    /// CraftEngine commit this converter is audited against.
    pub const CRAFTENGINE_COMMIT: &str = "c9a2ab61db6f5cea7314f506b098dea08c7bd323";
    /// Minecraft version whose registries/codecs are mirrored below.
    pub const MINECRAFT_VERSION: &str = "1.21.11";
}
