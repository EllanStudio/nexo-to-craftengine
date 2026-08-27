//! Minecraft resource-location validation and normalization.
//!
//! Port of `legacy/src/resource-location.ts`. Pack-relative paths are
//! always slash-separated regardless of host OS.

use crate::diagnostics::{Details, DiagnosticBag};

fn is_namespace_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-')
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '/' | '.' | '_' | '-')
}

fn valid_namespace(namespace: &str) -> bool {
    !namespace.is_empty() && namespace.chars().all(is_namespace_char)
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.chars().all(is_path_char)
        && !path.starts_with('/')
        && !path.split('/').any(|segment| segment == "..")
}

pub fn strip_known_extension(value: &str, extensions: &[&str]) -> String {
    let lower = value.to_lowercase();
    for extension in extensions {
        if lower.ends_with(&extension.to_lowercase()) {
            return value[..value.len() - extension.len()].to_string();
        }
    }
    value.to_string()
}

#[allow(clippy::too_many_arguments)]
pub fn normalize_location(
    input: &str,
    diagnostics: &mut DiagnosticBag,
    details: &Details,
    extensions: &[&str],
    default_namespace: &str,
) -> Option<String> {
    let value = strip_known_extension(&input.trim().replace('\\', "/"), extensions);
    let (namespace, path) = match value.find(':') {
        Some(separator) => (&value[..separator], &value[separator + 1..]),
        None => (default_namespace, value.as_str()),
    };
    if !valid_namespace(namespace) || !valid_path(path) {
        diagnostics.error(
            "INVALID_RESOURCE_LOCATION",
            &format!("Invalid Minecraft resource location: {}", input),
            details.clone(),
        );
        return None;
    }
    Some(format!("{}:{}", namespace, path))
}

pub fn normalize_model_location(
    input: &str,
    diagnostics: &mut DiagnosticBag,
    details: &Details,
) -> Option<String> {
    normalize_location(input, diagnostics, details, &[".json"], "minecraft")
}

pub fn normalize_texture_location(
    input: &str,
    diagnostics: &mut DiagnosticBag,
    details: &Details,
) -> Option<String> {
    if input.starts_with('#') {
        return Some(input.to_string());
    }
    normalize_location(input, diagnostics, details, &[".png"], "minecraft")
}

pub fn normalize_sound_location(
    input: &str,
    diagnostics: &mut DiagnosticBag,
    details: &Details,
) -> Option<String> {
    normalize_location(input, diagnostics, details, &[".ogg"], "minecraft")
}

pub fn normalize_item_path(
    input: &str,
    diagnostics: &mut DiagnosticBag,
    details: &Details,
) -> Option<String> {
    let value = input.trim().to_lowercase().replace(' ', "_");
    if !valid_path(&value) {
        diagnostics.error(
            "INVALID_ITEM_ID",
            &format!("Invalid item id: {}", input),
            details.clone(),
        );
        return None;
    }
    if value != input {
        diagnostics.warning(
            "ITEM_ID_NORMALIZED",
            &format!("Item id normalized from {} to {}", input, value),
            details.clone().lossy(),
        );
    }
    Some(value)
}

pub fn validate_namespace(namespace: &str) -> bool {
    valid_namespace(namespace)
}

/// Split a validated `namespace:path` location.
pub fn split_location(location: &str) -> (&str, &str) {
    match location.find(':') {
        Some(separator) => (&location[..separator], &location[separator + 1..]),
        None => (location, ""),
    }
}

pub type AssetCategory = &'static str;

pub const ASSET_MODELS: AssetCategory = "models";
pub const ASSET_TEXTURES: AssetCategory = "textures";
pub const ASSET_ITEMS: AssetCategory = "items";
pub const ASSET_SOUNDS: AssetCategory = "sounds";
pub const ASSET_FONT: AssetCategory = "font";

/// Slash-separated asset path under a resource-pack root.
pub fn asset_file(resource_root: &str, category: AssetCategory, location: &str, extension: &str) -> String {
    let (namespace, path) = split_location(location);
    format!("{}/assets/{}/{}/{}{}", resource_root, namespace, category, path, extension)
}

pub fn minimal_location(location: &str) -> &str {
    location.strip_prefix("minecraft:").unwrap_or(location)
}

pub fn minecraft_key(value: &str) -> &str {
    value.strip_prefix("minecraft:").unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bag() -> DiagnosticBag {
        DiagnosticBag::new()
    }

    #[test]
    fn normalize_location_applies_default_namespace_and_extension() {
        let mut d = bag();
        let details = Details::new();
        assert_eq!(
            normalize_location("stone", &mut d, &details, &[], "minecraft").as_deref(),
            Some("minecraft:stone")
        );
        // Extension stripping is case-insensitive, but uppercase path chars
        // stay invalid exactly like the TS RESOURCE_PATH regex.
        assert_eq!(
            normalize_location("custom:item.PNG", &mut d, &details, &[".png"], "minecraft").as_deref(),
            Some("custom:item")
        );
        assert!(!d.has_errors());
        assert!(normalize_location("custom:Item.PNG", &mut d, &details, &[".png"], "minecraft").is_none());
        assert!(d.has_errors());
    }

    #[test]
    fn normalize_location_rejects_invalid_paths() {
        let mut d = bag();
        let details = Details::new();
        assert!(normalize_location("a/../b", &mut d, &details, &[], "minecraft").is_none());
        assert!(normalize_location("/abs", &mut d, &details, &[], "minecraft").is_none());
        assert!(normalize_location("Bad:NS", &mut d, &details, &[], "minecraft").is_none());
        assert!(d.has_errors());
    }

    #[test]
    fn texture_location_passes_through_references() {
        let mut d = bag();
        let details = Details::new();
        assert_eq!(
            normalize_texture_location("#layer0", &mut d, &details).as_deref(),
            Some("#layer0")
        );
    }

    #[test]
    fn item_path_normalizes_and_warns() {
        let mut d = bag();
        let details = Details::new();
        assert_eq!(normalize_item_path("My Item", &mut d, &details).as_deref(), Some("my_item"));
        assert!(d.has_lossy());
        let mut d2 = bag();
        assert_eq!(normalize_item_path("plain", &mut d2, &details).as_deref(), Some("plain"));
        assert!(!d2.has_lossy());
    }

    #[test]
    fn asset_file_is_slash_separated() {
        assert_eq!(
            asset_file("pack", ASSET_TEXTURES, "custom:item", ".png"),
            "pack/assets/custom/textures/item.png"
        );
    }

    #[test]
    fn minimal_location_strips_minecraft_prefix() {
        assert_eq!(minimal_location("minecraft:stone"), "stone");
        assert_eq!(minimal_location("custom:stone"), "custom:stone");
    }
}
