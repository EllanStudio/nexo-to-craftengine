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

/// Resource-pack model strategy, mirrored from the legacy CLI --client-mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientMode {
    Modern,
    Hybrid,
    Legacy,
}

impl ClientMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "modern" => Some(Self::Modern),
            "hybrid" => Some(Self::Hybrid),
            "legacy" => Some(Self::Legacy),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modern => "modern",
            Self::Hybrid => "hybrid",
            Self::Legacy => "legacy",
        }
    }
}

/// Legacy custom_model_data handling, mirrored from the legacy CLI --cmd-policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmdPolicy {
    Preserve,
    Allocate,
    Omit,
}

impl CmdPolicy {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "preserve" => Some(Self::Preserve),
            "allocate" => Some(Self::Allocate),
            "omit" => Some(Self::Omit),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::Allocate => "allocate",
            Self::Omit => "omit",
        }
    }
}

pub mod audit;
pub mod categories;
pub mod component_builders;
pub mod converter;
pub mod data;
pub mod diagnostics;
pub mod glyphs;
pub mod io;
pub mod items;
pub mod json;
pub mod mechanics;
pub mod model_aliases;
pub mod models;
pub mod recipes;
pub mod resource_location;
pub mod resources;
pub mod sounds;
pub mod source_namespace;

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
