//! Typo recovery for model references.
//!
//! Port of `legacy/src/model-aliases.ts`. Recovers only a uniquely
//! identifiable filename typo by pointing at an existing model in the same
//! namespace/directory. This never creates or renames assets.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::diagnostics::{Details, DiagnosticBag};
use crate::items::ResolvedItem;
use crate::json::{get_object, get_string, get_value};

const SINGLE_MODEL_KEYS: &[&str] = &[
    "model",
    "blocking_model",
    "charged_model",
    "cast_model",
    "broken_model",
    "firework_model",
    "dyeable_model",
    "throwing_model",
];
const LIST_MODEL_KEYS: &[&str] = &["pulling_models", "damaged_models", "composite_models"];

static LOCATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:([a-z0-9_.-]+):)?([a-z0-9/._-]+?)(?:\.json)?$").unwrap());
static MODEL_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^([^/]+)/models/(.+)\.json$").unwrap());

fn lookup_location(raw: &str) -> Option<String> {
    let normalized = raw.trim().replace('\\', "/");
    let captures = LOCATION.captures(&normalized)?;
    let namespace = captures.get(1).map(|m| m.as_str()).unwrap_or("minecraft");
    Some(format!("{}:{}", namespace, &captures[2]))
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right_chars: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right_chars.len()).collect();
    for (i, left_char) in left.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, right_char) in right_chars.iter().enumerate() {
            let cost = if left_char == *right_char { 0 } else { 1 };
            current.push(
                (current[j] + 1)
                    .min(previous[j + 1] + 1)
                    .min(previous[j] + cost),
            );
        }
        previous = current;
    }
    previous[right_chars.len()]
}

fn common_prefix_ratio(left: &str, right: &str) -> f64 {
    let mut length = 0usize;
    let left_chars: Vec<char> = left.chars().collect();
    let right_chars: Vec<char> = right.chars().collect();
    while length < left_chars.len() && length < right_chars.len() && left_chars[length] == right_chars[length] {
        length += 1;
    }
    length as f64 / left_chars.len().max(right_chars.len()).max(1) as f64
}

fn model_locations(resource_pack_root: &Path) -> Vec<String> {
    let assets_root = resource_pack_root.join("assets");
    let mut result: Vec<String> = Vec::new();

    fn visit(directory: &Path, assets_root: &Path, result: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(directory) else { return };
        for entry in entries.flatten() {
            let child = entry.path();
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_dir() {
                visit(&child, assets_root, result);
            } else if file_type.is_file()
                && entry.file_name().to_string_lossy().to_lowercase().ends_with(".json")
            {
                let Ok(relative) = child.strip_prefix(assets_root) else { continue };
                let path = relative.to_string_lossy().replace('\\', "/");
                if let Some(captures) = MODEL_FILE.captures(&path) {
                    result.push(format!("{}:{}", &captures[1], &captures[2]));
                }
            }
        }
    }

    visit(&assets_root, &assets_root, &mut result);
    result
}

struct SourceReference {
    location: String,
    source: String,
    item: String,
    field: String,
}

fn source_references(items: &[ResolvedItem]) -> Vec<SourceReference> {
    let mut result = Vec::new();
    for item in items {
        if item.template {
            continue;
        }
        let Some(pack) = get_object(&item.config, "Pack") else { continue };
        for key in SINGLE_MODEL_KEYS {
            let raw = get_string(pack, key);
            let location = raw.and_then(|value| lookup_location(value));
            if let Some(location) = location {
                result.push(SourceReference {
                    location,
                    source: item.source.clone(),
                    item: item.id.clone(),
                    field: format!("Pack.{}", key),
                });
            }
        }
        for key in LIST_MODEL_KEYS {
            let raw = get_value(pack, key);
            let values: Vec<String> = match raw {
                Some(serde_json::Value::Array(entries)) => entries
                    .iter()
                    .filter_map(|entry| entry.as_str().map(str::to_string))
                    .collect(),
                Some(serde_json::Value::String(text)) => vec![text.clone()],
                _ => Vec::new(),
            };
            for value in values {
                if let Some(location) = lookup_location(&value) {
                    result.push(SourceReference {
                        location,
                        source: item.source.clone(),
                        item: item.id.clone(),
                        field: format!("Pack.{}", key),
                    });
                }
            }
        }
    }
    result
}

/// Mirrors TS `basename.slice(0, basename.lastIndexOf("_"))`, where a
/// missing underscore yields `slice(0, -1)` (drop the final character).
fn stem_of(basename: &str) -> String {
    match basename.rfind('_') {
        Some(index) => basename[..index].to_string(),
        None => basename.chars().take(basename.chars().count().saturating_sub(1)).collect(),
    }
}

fn directory_of(location: &str) -> String {
    match location.rfind('/') {
        Some(slash) => location[..slash].to_string(),
        None => location[..location.find(':').map(|index| index + 1).unwrap_or(location.len())].to_string(),
    }
}

pub fn discover_model_aliases(
    resource_pack_root: Option<&Path>,
    items: &[ResolvedItem],
    diagnostics: &mut DiagnosticBag,
) -> HashMap<String, String> {
    let Some(resource_pack_root) = resource_pack_root else {
        return HashMap::new();
    };
    let existing_list = model_locations(resource_pack_root);
    let existing: std::collections::HashSet<&str> = existing_list.iter().map(String::as_str).collect();
    let mut by_directory: HashMap<String, Vec<&String>> = HashMap::new();
    for location in &existing_list {
        by_directory.entry(directory_of(location)).or_default().push(location);
    }

    let mut aliases: HashMap<String, String> = HashMap::new();
    let mut reported: std::collections::HashSet<String> = std::collections::HashSet::new();

    for reference in source_references(items) {
        if existing.contains(reference.location.as_str()) || aliases.contains_key(&reference.location) {
            continue;
        }
        let directory = directory_of(&reference.location);
        let slash = reference.location.rfind('/');
        let basename = &reference.location[slash.map(|index| index + 1).unwrap_or(0)..];
        if basename.chars().count() < 12 {
            continue;
        }
        let stem = stem_of(basename);
        if stem.is_empty() {
            continue;
        }
        let mut candidates: Vec<(&String, String, usize)> = (by_directory.get(&directory).cloned().unwrap_or_default())
            .into_iter()
            .filter_map(|location| {
                let candidate = &location[location.rfind('/').map(|index| index + 1).unwrap_or(0)..];
                let candidate_stem = stem_of(candidate);
                let distance = edit_distance(basename, candidate);
                if candidate_stem == stem && distance <= 2 && common_prefix_ratio(basename, candidate) >= 0.75 {
                    Some((location, candidate.to_string(), distance))
                } else {
                    None
                }
            })
            .collect();
        candidates.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(b.0)));
        if candidates.is_empty() {
            continue;
        }
        let best = &candidates[0];
        if candidates.get(1).map(|entry| entry.2) == Some(best.2) {
            continue;
        }
        aliases.insert(reference.location.clone(), best.0.clone());
        if reported.insert(reference.location.clone()) {
            diagnostics.info(
                "MODEL_REFERENCE_TYPO_RECOVERED",
                &format!(
                    "Missing model reference {} was redirected to the unique existing near-match {}; no asset file was created",
                    reference.location, best.0
                ),
                Details::new()
                    .source(reference.source.clone())
                    .item(reference.item.clone())
                    .field(reference.field.clone()),
            );
        }
    }
    aliases
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lookup_location_defaults_to_minecraft() {
        assert_eq!(lookup_location("item/sword").as_deref(), Some("minecraft:item/sword"));
        assert_eq!(lookup_location("custom:item/sword.json").as_deref(), Some("custom:item/sword"));
        assert_eq!(lookup_location("Bad Location"), None);
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("same", "same"), 0);
    }

    #[test]
    fn unique_typo_is_recovered_from_pack_tree() {
        let dir = std::env::temp_dir().join("nexo2ce-alias-test");
        let _ = std::fs::remove_dir_all(&dir);
        let models = dir.join("assets/custom/models/item");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(models.join("author_longsword.json"), "{}").unwrap();

        let items = vec![ResolvedItem {
            id: "sword".to_string(),
            source: "items.yml".to_string(),
            config: json!({ "Pack": { "model": "custom:item/author_longswordd" } })
                .as_object()
                .unwrap()
                .clone(),
            template: false,
            template_ids: vec![],
        }];
        let mut diags = DiagnosticBag::new();
        let aliases = discover_model_aliases(Some(&dir), &items, &mut diags);
        assert_eq!(
            aliases.get("custom:item/author_longswordd").map(String::as_str),
            Some("custom:item/author_longsword")
        );
        assert!(diags.items.iter().any(|d| d.code == "MODEL_REFERENCE_TYPO_RECOVERED"));
    }
}
