//! Resource-graph audit: resolves model/texture/blueprint references from the
//! item/block configuration against the files of the generated resource pack.
//!
//! Port of `legacy/src/audit.ts`. Diagnostic codes, messages, counting
//! semantics, and reference ordering (JS Map/Set insertion order) are kept
//! identical to the TypeScript original.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::Value;

use crate::diagnostics::{Details, DiagnosticBag};
use crate::json::JsonObject;
use crate::resource_location::{
    asset_file, normalize_model_location, normalize_texture_location, split_location,
    ASSET_MODELS, ASSET_TEXTURES,
};

pub struct AuditInput<'a> {
    pub resource_root: String,
    pub items: &'a JsonObject,
    pub blocks: &'a JsonObject,
    pub images: Option<&'a JsonObject>,
    pub blueprint_root: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuditSummary {
    pub referenced_models: usize,
    pub resolved_models: usize,
    pub generated_models: usize,
    pub referenced_blueprints: usize,
    pub missing_blueprints: usize,
    pub copied_item_definitions: usize,
    pub referenced_textures: usize,
    pub resolved_textures: usize,
    pub missing_models: usize,
    pub missing_textures: usize,
}

/// Insertion-ordered string set (JS Set semantics: first-insert position, dedup).
#[derive(Default)]
struct OrderedSet {
    order: Vec<String>,
    seen: HashSet<String>,
}

impl OrderedSet {
    /// Returns true when the value was newly inserted.
    fn insert(&mut self, value: &str) -> bool {
        if self.seen.insert(value.to_string()) {
            self.order.push(value.to_string());
            true
        } else {
            false
        }
    }

    fn len(&self) -> usize {
        self.order.len()
    }

    fn iter(&self) -> std::slice::Iter<'_, String> {
        self.order.iter()
    }
}

/// Insertion-ordered model reference map (JS Map semantics); the generated
/// flag is OR-merged on duplicate keys.
#[derive(Default)]
struct ModelRefs {
    entries: Vec<(String, bool)>,
    index: HashMap<String, usize>,
}

impl ModelRefs {
    fn add(&mut self, raw: &str, generated: bool) {
        match self.index.get(raw) {
            Some(&slot) => self.entries[slot].1 |= generated,
            None => {
                self.index.insert(raw.to_string(), self.entries.len());
                self.entries.push((raw.to_string(), generated));
            }
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn iter(&self) -> std::slice::Iter<'_, (String, bool)> {
        self.entries.iter()
    }
}

fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

/// Node `dirname` equivalent.
fn dirname(path: &str) -> String {
    match Path::new(path).parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_string_lossy().into_owned(),
        _ => ".".to_string(),
    }
}

/// Recursively list `.json` files (case-insensitive extension), like the TS
/// `listJsonFiles` walker. Missing directory yields an empty list.
fn list_json_files(directory: &Path) -> Vec<std::path::PathBuf> {
    if !directory.exists() {
        return Vec::new();
    }
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(directory)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        let name = entry.file_name().to_string_lossy();
        if entry.file_type().is_file() && name.to_lowercase().ends_with(".json") {
            files.push(entry.into_path());
        }
    }
    files
}

fn known_vanilla_parent(location: &str) -> bool {
    if location.starts_with("builtin:") {
        return true;
    }
    let Some(path) = location.strip_prefix("minecraft:") else {
        return false;
    };
    path == "item/generated"
        || path == "item/handheld"
        || path == "item/handheld_rod"
        || path.starts_with("item/template_")
        || path == "block/block"
        || path == "block/cube"
        || path.starts_with("block/cube_")
        || path.starts_with("block/orientable")
        || path == "block/cross"
        || path.starts_with("block/template_")
        || path.contains("stairs")
        || path.contains("slab")
}

fn collect_generation(
    generation: &JsonObject,
    model_refs: &mut ModelRefs,
    texture_refs: &mut OrderedSet,
) {
    if let Some(parent) = generation.get("parent").and_then(Value::as_str) {
        model_refs.add(parent, false);
    }
    if let Some(textures) = generation.get("textures").and_then(Value::as_object) {
        for texture in textures.values() {
            if let Some(texture) = texture.as_str() {
                if !texture.starts_with('#') {
                    texture_refs.insert(texture);
                }
            }
        }
    }
}

fn collect_model_nodes(
    value: &Value,
    model_refs: &mut ModelRefs,
    texture_refs: &mut OrderedSet,
    blueprint_refs: &mut OrderedSet,
    parent_key: &str,
) {
    if let Some(entries) = value.as_array() {
        for entry in entries {
            collect_model_nodes(entry, model_refs, texture_refs, blueprint_refs, parent_key);
        }
        return;
    }
    let object = match value.as_object() {
        Some(object) => object,
        None => {
            // Bare strings under a "model" key are plain model references.
            if let Some(raw) = value.as_str() {
                if parent_key == "model" {
                    model_refs.add(raw, false);
                }
            }
            return;
        }
    };
    let node_type = match object.get("type").and_then(Value::as_str) {
        Some(value) => value.strip_prefix("minecraft:").unwrap_or(value),
        None => "",
    };
    let generation = object.get("generation").and_then(Value::as_object);
    let blueprint = object.get("blueprint").and_then(Value::as_str);
    let generated = generation.is_some() || blueprint.is_some();
    let base = object.get("base").and_then(Value::as_str);
    if node_type == "model" {
        let path = object
            .get("path")
            .and_then(Value::as_str)
            .or_else(|| object.get("model").and_then(Value::as_str));
        if let Some(path) = path {
            model_refs.add(path, generated);
        }
    } else if node_type == "special" && base.is_some() {
        model_refs.add(base.unwrap(), generated);
    } else if object.contains_key("predicate") || generation.is_some() {
        if let Some(path) = object.get("path").and_then(Value::as_str) {
            model_refs.add(path, generated);
        }
    }
    if let Some(generation) = generation {
        collect_generation(generation, model_refs, texture_refs);
    }
    if let Some(blueprint) = blueprint {
        blueprint_refs.insert(blueprint);
    }
    for (key, entry) in object {
        collect_model_nodes(entry, model_refs, texture_refs, blueprint_refs, key);
    }
}

/// Read a JSON object file, reporting `code` diagnostics on failure.
/// Strips a leading UTF-8 BOM like the TS reader.
fn read_object(file: &str, diagnostics: &mut DiagnosticBag, code: &str) -> Option<JsonObject> {
    let content = match std::fs::read_to_string(file) {
        Ok(content) => content,
        Err(error) => {
            diagnostics.error(code, &error.to_string(), Details::new().source(file));
            return None;
        }
    };
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content.as_str());
    match serde_json::from_str::<Value>(content) {
        Ok(Value::Object(object)) => Some(object),
        Ok(_) => {
            diagnostics.error(code, "JSON root is not an object", Details::new().source(file));
            None
        }
        Err(error) => {
            diagnostics.error(code, &error.to_string(), Details::new().source(file));
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn visit_model(
    raw_location: &str,
    generated: bool,
    source: &str,
    input: &AuditInput<'_>,
    diagnostics: &mut DiagnosticBag,
    visited_models: &mut OrderedSet,
    texture_refs: &mut OrderedSet,
    generated_models: &mut usize,
    missing_models: &mut usize,
) {
    if raw_location.starts_with("builtin/") {
        return;
    }
    let location = if raw_location.starts_with("builtin:") {
        raw_location.to_string()
    } else {
        match normalize_model_location(
            raw_location,
            diagnostics,
            &Details::new().source(source).field("resource graph"),
        ) {
            Some(location) => location,
            None => return,
        }
    };
    if !visited_models.insert(&location) {
        return;
    }
    if generated {
        *generated_models += 1;
        return;
    }
    if known_vanilla_parent(&location) {
        return;
    }
    let file = asset_file(&input.resource_root, ASSET_MODELS, &location, ".json");
    if !exists(&file) {
        diagnostics.error(
            "MODEL_FILE_MISSING",
            &format!("Referenced static model does not exist: {}", location),
            Details::new().source(source).field(file).lossy(),
        );
        *missing_models += 1;
        return;
    }
    let model = match read_object(&file, diagnostics, "MODEL_JSON_INVALID") {
        Some(model) => model,
        None => return,
    };
    if let Some(parent) = model.get("parent").and_then(Value::as_str) {
        let parent = parent.to_string();
        visit_model(
            &parent,
            false,
            &file,
            input,
            diagnostics,
            visited_models,
            texture_refs,
            generated_models,
            missing_models,
        );
    }
    if let Some(textures) = model.get("textures").and_then(Value::as_object) {
        for texture in textures.values() {
            if let Some(texture) = texture.as_str() {
                if !texture.starts_with('#') {
                    texture_refs.insert(texture);
                }
            }
        }
    }
    if let Some(overrides) = model.get("overrides").and_then(Value::as_array) {
        for override_entry in overrides {
            if let Some(target) = override_entry.get("model").and_then(Value::as_str) {
                let target = target.to_string();
                visit_model(
                    &target,
                    false,
                    &file,
                    input,
                    diagnostics,
                    visited_models,
                    texture_refs,
                    generated_models,
                    missing_models,
                );
            }
        }
    }
}

pub fn audit_resource_graph(input: &AuditInput<'_>, diagnostics: &mut DiagnosticBag) -> AuditSummary {
    let mut model_refs = ModelRefs::default();
    let mut texture_refs = OrderedSet::default();
    let mut blueprint_refs = OrderedSet::default();
    let mut generated_pointers = OrderedSet::default();

    for item in input.items.values() {
        let Some(item) = item.as_object() else { continue };
        if let Some(model) = item.get("model") {
            collect_model_nodes(model, &mut model_refs, &mut texture_refs, &mut blueprint_refs, "");
        }
        if let Some(legacy_model) = item.get("legacy_model") {
            collect_model_nodes(legacy_model, &mut model_refs, &mut texture_refs, &mut blueprint_refs, "");
        }
        if item.contains_key("model") {
            if let Some(item_model) = item.get("item_model").and_then(Value::as_str) {
                generated_pointers.insert(item_model);
            }
        }
    }
    for block in input.blocks.values() {
        if block.is_object() {
            collect_model_nodes(block, &mut model_refs, &mut texture_refs, &mut blueprint_refs, "");
        }
    }
    if let Some(images) = input.images {
        for image in images.values() {
            if let Some(file) = image.get("file").and_then(Value::as_str) {
                texture_refs.insert(file);
            }
        }
    }

    // Blueprint existence runs before the item-definition scan, exactly like
    // the TS order: blueprints first seen in copied definitions are counted
    // but never checked.
    let blueprint_root = match &input.blueprint_root {
        Some(root) => root.clone(),
        None => Path::new(&dirname(&input.resource_root))
            .join("blueprint")
            .to_string_lossy()
            .into_owned(),
    };
    let mut missing_blueprints = 0usize;
    for blueprint in blueprint_refs.iter() {
        let name = if blueprint.ends_with(".bbmodel") {
            blueprint.clone()
        } else {
            format!("{}.bbmodel", blueprint)
        };
        // TS rewrites "/" to "\\" before join (Windows blueprint layout).
        let file = Path::new(&blueprint_root)
            .join(name.replace('/', "\\"))
            .to_string_lossy()
            .into_owned();
        if !exists(&file) {
            diagnostics.error(
                "BLUEPRINT_FILE_MISSING",
                &format!("Referenced Blockbench blueprint does not exist: {}", blueprint),
                Details::new().source(file).lossy(),
            );
            missing_blueprints += 1;
        }
    }

    let mut copied_item_definitions = 0usize;
    let assets_root = Path::new(&input.resource_root).join("assets");
    for file in list_json_files(&assets_root) {
        let file = file.to_string_lossy().into_owned();
        // TS slash-normalizes the walked path before the "/items/" check.
        if !file.replace('\\', "/").contains("/items/") {
            continue;
        }
        copied_item_definitions += 1;
        if let Some(definition) = read_object(&file, diagnostics, "ITEM_DEFINITION_JSON_INVALID") {
            if let Some(model) = definition.get("model") {
                collect_model_nodes(model, &mut model_refs, &mut texture_refs, &mut blueprint_refs, "");
            }
        }
    }
    for pointer in generated_pointers.iter() {
        let qualified = if pointer.contains(':') {
            pointer.clone()
        } else {
            format!("minecraft:{}", pointer)
        };
        let (namespace, path) = split_location(&qualified);
        let file = Path::new(&input.resource_root)
            .join("assets")
            .join(namespace)
            .join("items")
            .join(format!("{}.json", path))
            .to_string_lossy()
            .into_owned();
        if exists(&file) {
            diagnostics.warning(
                "ITEM_DEFINITION_CONFLICT",
                &format!(
                    "A copied item definition occupies a path CraftEngine will generate: {}",
                    pointer
                ),
                Details::new().source(file).lossy(),
            );
        }
    }

    let mut visited_models = OrderedSet::default();
    let mut generated_models = 0usize;
    let mut missing_models = 0usize;
    for (location, generated) in model_refs.iter() {
        visit_model(
            location,
            *generated,
            "configuration",
            input,
            diagnostics,
            &mut visited_models,
            &mut texture_refs,
            &mut generated_models,
            &mut missing_models,
        );
    }

    let mut normalized_textures = OrderedSet::default();
    let mut missing_textures = 0usize;
    for texture in texture_refs.iter() {
        let location = match normalize_texture_location(
            texture,
            diagnostics,
            &Details::new().source("resource graph").field("texture"),
        ) {
            Some(location) => location,
            None => continue,
        };
        if location == "minecraft:missingno" || !normalized_textures.insert(&location) {
            continue;
        }
        let file = asset_file(&input.resource_root, ASSET_TEXTURES, &location, ".png");
        if !exists(&file) {
            diagnostics.error(
                "TEXTURE_FILE_MISSING",
                &format!("Referenced texture does not exist: {}", location),
                Details::new().source(file).lossy(),
            );
            missing_textures += 1;
        }
    }

    AuditSummary {
        referenced_models: model_refs.len(),
        resolved_models: visited_models.len() - missing_models,
        generated_models,
        referenced_blueprints: blueprint_refs.len(),
        missing_blueprints,
        copied_item_definitions,
        referenced_textures: normalized_textures.len(),
        resolved_textures: normalized_textures.len() - missing_textures,
        missing_models,
        missing_textures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn known_vanilla_parents_match_ts() {
        assert!(known_vanilla_parent("builtin:entity"));
        assert!(known_vanilla_parent("minecraft:item/generated"));
        assert!(known_vanilla_parent("minecraft:block/template_torch"));
        assert!(known_vanilla_parent("minecraft:block/oak_stairs"));
        assert!(!known_vanilla_parent("minecraft:item/diamond"));
        assert!(!known_vanilla_parent("custom:block/cube"));
    }

    #[test]
    fn collects_model_texture_and_blueprint_refs() {
        let value: Value = json!({
            "type": "minecraft:model",
            "path": "custom:item/sword",
            "blueprint": "models/sword",
            "generation": {
                "parent": "minecraft:item/generated",
                "textures": { "layer0": "#0", "side": "custom:block/steel" }
            },
            "variants": [{ "model": "custom:item/sword_guard" }]
        });
        let mut model_refs = ModelRefs::default();
        let mut texture_refs = OrderedSet::default();
        let mut blueprint_refs = OrderedSet::default();
        collect_model_nodes(&value, &mut model_refs, &mut texture_refs, &mut blueprint_refs, "");
        let models: Vec<(&str, bool)> = model_refs
            .iter()
            .map(|(raw, generated)| (raw.as_str(), *generated))
            .collect();
        assert_eq!(
            models,
            vec![
                ("custom:item/sword", true),
                ("minecraft:item/generated", false),
                ("custom:item/sword_guard", false),
            ]
        );
        assert_eq!(texture_refs.order, vec!["custom:block/steel".to_string()]);
        assert_eq!(blueprint_refs.order, vec!["models/sword".to_string()]);
    }

    #[test]
    fn audits_pack_and_reports_missing_assets() {
        let root = std::env::temp_dir().join(format!("dsh-audit-rs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let model_dir = root.join("assets").join("pack").join("models").join("item");
        let items_dir = root.join("assets").join("pack").join("items");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::create_dir_all(&items_dir).unwrap();
        std::fs::write(
            model_dir.join("good.json"),
            r#"{"parent":"minecraft:item/generated","textures":{"layer0":"pack:item/good"}}"#,
        )
        .unwrap();
        std::fs::write(items_dir.join("taken.json"), "{}").unwrap();
        let items: JsonObject = serde_json::from_value(json!({
            "good": {
                "model": { "type": "model", "path": "pack:item/good" },
                "item_model": "pack:taken"
            },
            "bad": { "model": { "type": "model", "path": "pack:item/bad" } }
        }))
        .unwrap();
        let blocks = JsonObject::new();
        let mut diagnostics = DiagnosticBag::new();
        let input = AuditInput {
            resource_root: root.to_string_lossy().into_owned(),
            items: &items,
            blocks: &blocks,
            images: None,
            blueprint_root: None,
        };
        let summary = audit_resource_graph(&input, &mut diagnostics);
        assert_eq!(summary.referenced_models, 2);
        assert_eq!(summary.missing_models, 1);
        // good + its vanilla parent + bad = 3 visited, 1 missing.
        assert_eq!(summary.resolved_models, 2);
        assert_eq!(summary.copied_item_definitions, 1);
        assert_eq!(summary.referenced_textures, 1);
        assert_eq!(summary.missing_textures, 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
