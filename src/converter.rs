//! Conversion orchestrator: Nexo directory in, CraftEngine pack out.
//!
//! Port of `legacy/src/converter.ts`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::audit::{audit_resource_graph, AuditInput, AuditSummary};
use crate::categories::{convert_categories, CategoryConversionOptions, CategoryItem};
use crate::diagnostics::DiagnosticBag;
use crate::glyphs::{convert_glyphs, rewrite_glyph_tags};
use crate::io::{load_yaml, write_json, write_yaml};
use crate::items::{
    convert_item, match_bukkit_material, resolve_item_templates, ItemOptions,
    ResolvedItem, SourceItem,
};
use crate::json::{
    as_string_list, deep_merge, get_boolean, get_number, get_object, get_string, get_value, JsonObject,
};
use crate::mechanics::convert_mechanics;
use crate::model_aliases::discover_model_aliases;
use crate::recipes::{convert_recipe, NexoRecipeType};
use crate::resource_location::validate_namespace;
use crate::resources::{copy_resource_pack, find_resource_pack_root, write_language_resources};
use crate::sounds::convert_sounds;
use crate::source_namespace::{infer_author_namespace_from_nexo_files, NamespaceInference};
use crate::{ClientMode, CmdPolicy};

pub const NEXO_ITEM_NAMESPACE: &str = "nexo";

pub struct ConvertOptions {
    pub input: String,
    pub output: String,
    /// Explicit override; omit to use the author's namespace detected from source files.
    pub namespace: Option<String>,
    /// Trusted full-bundle inference supplied by the archive detector.
    pub source_namespace: Option<NamespaceInference>,
    pub client_mode: ClientMode,
    pub cmd_policy: CmdPolicy,
    pub strict: bool,
    pub force: bool,
    pub audit: bool,
}

#[derive(Debug)]
pub struct ConversionResult {
    pub success: bool,
    pub diagnostics: DiagnosticBag,
    pub report_file: Option<String>,
    pub item_count: usize,
    pub category_count: usize,
    pub template_count: usize,
    pub furniture_count: usize,
    pub block_count: usize,
    pub recipe_count: usize,
    pub sound_count: usize,
    pub glyph_count: usize,
    pub resource_count: usize,
    pub audit: Option<AuditSummary>,
    pub namespace: String,
    pub namespace_mode: &'static str,
}

#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

const RECIPE_TYPES: &[NexoRecipeType] = &[
    NexoRecipeType::Shaped,
    NexoRecipeType::Shapeless,
    NexoRecipeType::Furnace,
    NexoRecipeType::Blasting,
    NexoRecipeType::Smoking,
    NexoRecipeType::Campfire,
    NexoRecipeType::Stonecutting,
    NexoRecipeType::Brewing,
];

fn exists(path: &Path) -> bool {
    path.try_exists().unwrap_or(false)
}

fn list_files(directory: &Path, extension: &str) -> Vec<PathBuf> {
    let mut result: Vec<PathBuf> = Vec::new();
    if !exists(directory) {
        return result;
    }
    fn visit(current: &Path, extension: &str, result: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(current) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_dir() {
                visit(&path, extension, result);
            } else if file_type.is_file() && entry.file_name().to_string_lossy().to_lowercase().ends_with(extension) {
                result.push(path);
            }
        }
    }
    visit(directory, extension, &mut result);
    result.sort();
    result
}

fn resolve_nexo_root(input: &str) -> PathBuf {
    let absolute = std::fs::canonicalize(input).unwrap_or_else(|_| PathBuf::from(input));
    let name = absolute
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name == "items" || name == "item" {
        return absolute.parent().map(Path::to_path_buf).unwrap_or(absolute);
    }
    for candidate in [
        absolute.clone(),
        absolute.join("Nexo"),
        absolute.join("nexo"),
    ] {
        if exists(&candidate.join("items")) || exists(&candidate.join("item")) {
            return candidate;
        }
    }
    absolute
}

/// Canonicalize a path, tolerating missing trailing components (a new output
/// directory). Missing suffixes are expected for a new output, but
/// permission/reparse failures must abort.
fn canonical_path(path: &Path) -> Result<PathBuf, ConvertError> {
    let mut cursor = path.to_path_buf();
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match cursor.symlink_metadata() {
            Ok(_) => break,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(ConvertError::Io(error));
                }
                let Some(parent) = cursor.parent().map(Path::to_path_buf) else {
                    return Err(ConvertError::Io(error));
                };
                if parent == cursor {
                    return Err(ConvertError::Io(error));
                }
                missing.insert(0, cursor.file_name().unwrap_or_default().to_os_string());
                cursor = parent;
            }
        }
    }
    let mut canonical = std::fs::canonicalize(&cursor)?;
    for component in missing {
        canonical.push(component);
    }
    Ok(canonical)
}

fn comparable_path(path: &Path) -> PathBuf {
    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if cfg!(windows) {
        PathBuf::from(absolute.to_string_lossy().to_lowercase())
    } else {
        absolute
    }
}

fn contains_path(parent: &Path, child: &Path) -> bool {
    let Some(relation) = pathdiff::diff_paths(child, parent) else {
        return false;
    };
    if relation.as_os_str().is_empty() {
        return true;
    }
    if relation.is_absolute() {
        return false;
    }
    let first = relation.components().next();
    !matches!(
        first,
        Some(std::path::Component::ParentDir)
    )
}

fn prepare_output(protected_inputs: &[PathBuf], output: &Path, force: bool) -> Result<(), ConvertError> {
    let destination = std::fs::canonicalize(output).unwrap_or_else(|_| output.to_path_buf());
    let canonical_destination = canonical_path(&destination)?;
    for input in protected_inputs {
        let source = canonical_path(input)?;
        if contains_path(&source, &canonical_destination) || contains_path(&canonical_destination, &source) {
            return Err(ConvertError::Message(format!(
                "Output directory must not overlap the Nexo input or resource-pack directory: {}",
                canonical_destination.display()
            )));
        }
    }
    if exists(&destination) {
        let contents: Vec<_> = std::fs::read_dir(&destination)?.flatten().collect();
        if !contents.is_empty() && !force {
            return Err(ConvertError::Message(format!(
                "Output directory is not empty; use --force to replace it: {}",
                destination.display()
            )));
        }
        if force {
            std::fs::remove_dir_all(&destination)?;
        }
    }
    std::fs::create_dir_all(destination.join("configuration"))?;
    std::fs::create_dir_all(destination.join("resourcepack"))?;
    Ok(())
}

/// Stable identity of the file system object behind a path, used to fold
/// hard-link aliases the way the TS loader's inode check does.
fn file_identity_key(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path).ok()?;
        return Some(format!("unix:{}:{}", metadata.dev(), metadata.ino()));
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;

        #[repr(C)]
        struct FileTime {
            low: u32,
            high: u32,
        }
        #[repr(C)]
        #[allow(non_snake_case)]
        struct ByHandleFileInfo {
            dwFileAttributes: u32,
            ftCreationTime: FileTime,
            ftLastAccessTime: FileTime,
            ftLastWriteTime: FileTime,
            dwVolumeSerialNumber: u32,
            nFileSizeHigh: u32,
            nFileSizeLow: u32,
            nNumberOfLinks: u32,
            nFileIndexHigh: u32,
            nFileIndexLow: u32,
        }
        extern "system" {
            fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut ByHandleFileInfo) -> i32;
        }
        let file = std::fs::File::open(path).ok()?;
        let mut info = std::mem::MaybeUninit::<ByHandleFileInfo>::uninit();
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as *mut std::ffi::c_void, info.as_mut_ptr()) };
        if ok == 0 {
            return None;
        }
        let info = unsafe { info.assume_init() };
        let index = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
        return Some(format!("win:{}:{}", info.dwVolumeSerialNumber, index));
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        None
    }
}

/// Deduplicate alias paths. realpath/canonicalize collapses symlink and
/// junction aliases; a file-identity key additionally collapses distinct
/// hard-link names for the same file, matching the TS inode check.
fn unique_canonical_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut unique: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut identities: Vec<String> = Vec::new();
    for path in paths {
        let canonical = canonical_path(path).unwrap_or_else(|_| path.clone());
        let key = comparable_path(&canonical);
        if unique.iter().any(|(existing, _)| *existing == key) {
            continue;
        }
        if let Some(identity) = file_identity_key(path) {
            if identities.iter().any(|existing| *existing == identity) {
                continue;
            }
            identities.push(identity);
        }
        unique.push((key, path.clone()));
    }
    unique.into_iter().map(|(_, path)| path).collect()
}

fn list_item_config_files(root: &Path) -> Vec<PathBuf> {
    let candidate_directories = [root.join("items"), root.join("item")];
    let existing: Vec<PathBuf> = candidate_directories
        .iter()
        .filter(|directory| exists(directory))
        .cloned()
        .collect();
    let directories = unique_canonical_paths(&existing);
    let mut candidates: Vec<PathBuf> = Vec::new();
    for directory in directories {
        candidates.extend(list_files(&directory, ".yml"));
    }
    let mut files = unique_canonical_paths(&candidates);
    files.sort();
    files
}

fn load_items(root: &Path, diagnostics: &mut DiagnosticBag) -> Vec<SourceItem> {
    let files = list_item_config_files(root);
    let mut items: Vec<SourceItem> = Vec::new();
    let mut ids: HashMap<String, String> = HashMap::new();
    for file in &files {
        let loaded = load_yaml(file, diagnostics);
        let Some(Value::Object(map)) = loaded else {
            continue;
        };
        for (id, config) in map {
            let Some(Value::Object(config)) = Some(config) else {
                diagnostics.error(
                    "ITEM_SECTION_INVALID",
                    "Item section must be a map",
                    crate::diagnostics::Details::new()
                        .source(file.display().to_string())
                        .item(id.clone()),
                );
                continue;
            };
            if let Some(previous) = ids.get(&id) {
                diagnostics.error(
                    "DUPLICATE_ITEM_ID",
                    &format!("Item id is also defined in {}", previous),
                    crate::diagnostics::Details::new()
                        .source(file.display().to_string())
                        .item(id.clone()),
                );
                continue;
            }
            ids.insert(id.clone(), file.display().to_string());
            items.push(SourceItem {
                id,
                source: file.display().to_string(),
                config,
                template: false,
            });
        }
    }
    if files.is_empty() {
        diagnostics.error(
            "ITEM_DIRECTORY_EMPTY",
            "No Nexo item YAML files found under items/ or item/",
            crate::diagnostics::Details::new().source(root.display().to_string()),
        );
    }
    items
}

fn item_material(item: &ResolvedItem) -> String {
    match_bukkit_material(get_value(&item.config, "material")).unwrap_or_else(|| "paper".to_string())
}

fn item_model_identity(item: &ResolvedItem) -> String {
    match get_object(&item.config, "Pack") {
        Some(pack) => get_string(pack, "model")
            .or_else(|| get_string(pack, "bbmodel"))
            .map(str::to_string)
            .unwrap_or_else(|| item.id.clone()),
        None => item.id.clone(),
    }
}

fn explicit_cmd(item: &ResolvedItem) -> Option<i64> {
    let pack = get_object(&item.config, "Pack")?;
    let value = get_number(pack, "custom_model_data")?;
    if value.fract() == 0.0 && value > 0.0 {
        Some(value as i64)
    } else {
        None
    }
}

fn allocate_custom_model_data(
    items: &[ResolvedItem],
    options: &ConvertOptions,
    diagnostics: &mut DiagnosticBag,
) -> HashMap<String, i64> {
    let mut assignments: HashMap<String, i64> = HashMap::new();
    let mut used_by_material: HashMap<String, HashMap<i64, String>> = HashMap::new();
    let mut by_model: HashMap<String, HashMap<String, i64>> = HashMap::new();
    let concrete: Vec<&ResolvedItem> = items
        .iter()
        .filter(|item| !item.template && get_object(&item.config, "Pack").is_some())
        .collect();
    for item in &concrete {
        let Some(explicit) = explicit_cmd(item) else { continue };
        let material = item_material(item);
        let model = item_model_identity(item);
        let used = used_by_material.entry(material.clone()).or_default();
        if let Some(conflict) = used.get(&explicit) {
            if conflict != &model {
                diagnostics.error(
                    "CUSTOM_MODEL_DATA_CONFLICT",
                    &format!("CMD {} on {} is already used by model {}", explicit, material, conflict),
                    crate::diagnostics::Details::new()
                        .source(item.source.clone())
                        .item(item.id.clone())
                        .field("Pack.custom_model_data"),
                );
            }
        }
        used.insert(explicit, model.clone());
        by_model.entry(material).or_default().insert(model, explicit);
        if options.cmd_policy != CmdPolicy::Omit {
            assignments.insert(item.id.clone(), explicit);
        } else {
            diagnostics.warning(
                "CUSTOM_MODEL_DATA_OMITTED",
                "Explicit Nexo custom_model_data was omitted by policy",
                crate::diagnostics::Details::new()
                    .source(item.source.clone())
                    .item(item.id.clone())
                    .field("Pack.custom_model_data")
                    .lossy(),
            );
        }
    }
    if options.cmd_policy == CmdPolicy::Allocate {
        for item in &concrete {
            if assignments.contains_key(&item.id) {
                continue;
            }
            let material = item_material(item);
            let model = item_model_identity(item);
            if let Some(existing) = by_model.get(&material).and_then(|models| models.get(&model)) {
                assignments.insert(item.id.clone(), *existing);
                continue;
            }
            let used = used_by_material.entry(material.clone()).or_default();
            let mut candidate = 1000i64;
            while used.contains_key(&candidate) {
                candidate += 1;
            }
            used.insert(candidate, model.clone());
            by_model.entry(material).or_default().insert(model, candidate);
            assignments.insert(item.id.clone(), candidate);
            diagnostics.info(
                "CUSTOM_MODEL_DATA_RECONSTRUCTED",
                &format!("Reconstructed Nexo material-scoped CMD allocation: {}", candidate),
                crate::diagnostics::Details::new()
                    .source(item.source.clone())
                    .item(item.id.clone())
                    .field("Pack.custom_model_data"),
            );
        }
    } else if options.cmd_policy == CmdPolicy::Preserve && options.client_mode != ClientMode::Modern {
        for item in &concrete {
            if !assignments.contains_key(&item.id) {
                diagnostics.warning(
                    "CUSTOM_MODEL_DATA_NOT_EXPLICIT",
                    "Nexo would allocate CMD at runtime, but preserve policy does not invent it; use --cmd-policy allocate after reviewing all source configs",
                    crate::diagnostics::Details::new()
                        .source(item.source.clone())
                        .item(item.id.clone())
                        .field("Pack.custom_model_data")
                        .lossy(),
                );
            }
        }
    }
    assignments
}

fn load_optional_object(file: &Path, diagnostics: &mut DiagnosticBag) -> Option<JsonObject> {
    if !exists(file) {
        return None;
    }
    match load_yaml(file, diagnostics) {
        Some(Value::Object(map)) => Some(map),
        _ => None,
    }
}

fn convert_recipes(root: &Path, namespace: &str, diagnostics: &mut DiagnosticBag) -> JsonObject {
    let mut output = JsonObject::new();
    for recipe_type in RECIPE_TYPES {
        let directory = root.join("recipes").join(recipe_type.as_str());
        for file in list_files(&directory, ".yml") {
            let Some(Value::Object(loaded)) = load_yaml(&file, diagnostics) else {
                continue;
            };
            for (id, section) in loaded {
                let Some(Value::Object(section)) = Some(section) else { continue };
                if let Some(converted) = convert_recipe(*recipe_type, &id, &section, namespace, diagnostics, &file.display().to_string()) {
                    output.insert(format!("{}:{}", namespace, id), Value::Object(converted));
                }
            }
        }
    }
    output
}

pub fn convert(options: &ConvertOptions) -> Result<ConversionResult, ConvertError> {
    let mut diagnostics = DiagnosticBag::new();
    let root = resolve_nexo_root(&options.input);
    let item_config_files = list_item_config_files(&root);
    let inferred_namespace = options
        .source_namespace
        .clone()
        .or_else(|| infer_author_namespace_from_nexo_files(&root, &item_config_files));
    let namespace = options
        .namespace
        .clone()
        .or_else(|| inferred_namespace.as_ref().map(|inference| inference.namespace.clone()))
        .unwrap_or_else(|| NEXO_ITEM_NAMESPACE.to_string());
    let namespace_mode = if options.namespace.is_some() {
        "override"
    } else if inferred_namespace.is_some() {
        "author"
    } else {
        "fallback"
    };
    if !validate_namespace(&namespace) {
        return Err(ConvertError::Message(format!("Invalid namespace: {}", namespace)));
    }
    let output = std::fs::canonicalize(&options.output).unwrap_or_else(|_| PathBuf::from(&options.output));
    // Discover and canonicalize every protected source before creating output.
    // This prevents --force from deleting an ancestor of the input and prevents
    // resource copying from recursing into a destination nested under assets/.
    let resource_pack_root = find_resource_pack_root(&root);
    let mut protected_sources: Vec<PathBuf> = vec![
        root.clone(),
        root.join("items"),
        root.join("item"),
        root.join("glyphs"),
        root.join("settings.yml"),
        root.join("inventory.yml"),
        root.join("mechanics.yml"),
        root.join("sounds.yml"),
        root.join("languages.yml"),
    ];
    for recipe_type in RECIPE_TYPES {
        protected_sources.push(root.join("recipes").join(recipe_type.as_str()));
    }
    if let Some(pack_root) = &resource_pack_root {
        protected_sources.push(pack_root.clone());
        protected_sources.push(pack_root.join("assets"));
    }
    prepare_output(&protected_sources, &output, options.force)?;

    let settings_root = load_optional_object(&root.join("settings.yml"), &mut diagnostics);
    let inventory_root = load_optional_object(&root.join("inventory.yml"), &mut diagnostics);
    let settings_inventory = settings_root
        .as_ref()
        .and_then(|settings| get_object(settings, "NexoInventory").or_else(|| get_object(settings, "nexo_inventory")).cloned());
    let file_inventory = inventory_root.as_ref().and_then(|inventory| {
        get_object(inventory, "NexoInventory")
            .or_else(|| get_object(inventory, "nexo_inventory"))
            .cloned()
            .or_else(|| Some(inventory.clone()))
    });
    let merged_inventory = match (&settings_inventory, &file_inventory) {
        (Some(settings), Some(file)) => Some(deep_merge(settings, file)),
        _ => file_inventory.clone().or_else(|| settings_inventory.clone()),
    };
    let glyph_settings = settings_root.as_ref().and_then(|settings| get_object(settings, "Glyphs").cloned());
    let default_glyph_font = glyph_settings
        .as_ref()
        .and_then(|settings| get_string(settings, "default_font").map(str::to_string))
        .unwrap_or_else(|| format!("{}:default", namespace));
    let default_glyph_permission = glyph_settings
        .as_ref()
        .and_then(|settings| get_string(settings, "default_permission").map(str::to_string))
        .unwrap_or_else(|| "nexo.glyphs.<glyphid>".to_string());
    let glyph_conversion = convert_glyphs(
        &root,
        &namespace,
        &mut diagnostics,
        Some(default_glyph_font.as_str()),
        Some(default_glyph_permission.as_str()),
    )
    .map_err(|error| ConvertError::Message(error.to_string()))?;
    let inventory_source = if file_inventory.is_some() {
        Some(root.join("inventory.yml"))
    } else if settings_inventory.is_some() {
        Some(root.join("settings.yml"))
    } else {
        None
    };
    let inventory_config = merged_inventory.map(|merged| {
        let mut wrapper = JsonObject::new();
        wrapper.insert("NexoInventory".to_string(), Value::Object(merged));
        wrapper
    });
    let mechanics_settings = load_optional_object(&root.join("mechanics.yml"), &mut diagnostics);
    let furniture_settings = mechanics_settings
        .as_ref()
        .and_then(|settings| get_object(settings, "furniture").cloned());
    let furniture_default_properties = furniture_settings
        .as_ref()
        .and_then(|settings| get_object(settings, "default_properties").cloned());
    let default_rotatable_on_sneak = furniture_settings
        .as_ref()
        .map(|settings| get_boolean(settings, "default_rotatable_on_sneak", false))
        .unwrap_or(false);
    let global_furniture_settings = settings_root
        .as_ref()
        .and_then(|settings| get_object(settings, "Furniture").cloned());
    let raw_rotation_gamemodes = global_furniture_settings
        .as_ref()
        .and_then(|settings| get_value(settings, "allowed_gamemodes_for_rotation").cloned());
    let rotation_gamemodes = match raw_rotation_gamemodes {
        Some(value) => as_string_list(Some(&value)),
        None => vec!["SURVIVAL".to_string(), "CREATIVE".to_string()],
    };
    let source_items = load_items(&root, &mut diagnostics);
    let resolved_items = resolve_item_templates(&source_items, &mut diagnostics);
    let model_aliases = discover_model_aliases(resource_pack_root.as_deref(), &resolved_items, &mut diagnostics);
    let cmd = allocate_custom_model_data(&resolved_items, options, &mut diagnostics);
    let mut items = JsonObject::new();
    let mut furniture = JsonObject::new();
    let mut blocks = JsonObject::new();
    let mut mappings = JsonObject::new();
    let mut category_items: Vec<CategoryItem> = Vec::new();
    let mut template_count = 0usize;
    let furniture_runtime = crate::mechanics::FurnitureRuntimeSettings {
        default_rotatable_on_sneak: Some(default_rotatable_on_sneak),
        rotation_gamemodes: Some(rotation_gamemodes),
    };
    for source_item in &resolved_items {
        if source_item.template {
            template_count += 1;
            continue;
        }
        let rewritten_config = rewrite_glyph_tags(
            &Value::Object(source_item.config.clone()),
            &glyph_conversion.entries,
            &mut diagnostics,
            &source_item.source,
            &source_item.id,
        );
        let rewritten_item = ResolvedItem {
            id: source_item.id.clone(),
            source: source_item.source.clone(),
            template: source_item.template,
            template_ids: source_item.template_ids.clone(),
            config: rewritten_config.as_object().cloned().unwrap_or_else(|| source_item.config.clone()),
        };
        let item_options = ItemOptions {
            namespace: namespace.clone(),
            client_mode: options.client_mode,
            model_aliases: Some(&model_aliases),
        };
        let Some(mut converted) = convert_item(&rewritten_item, &item_options, cmd.get(&source_item.id).copied(), &mut diagnostics)
        else {
            continue;
        };
        let mechanics = convert_mechanics(
            &rewritten_item.config,
            &converted.target_id,
            converted.base_model.as_deref(),
            &mut diagnostics,
            &source_item.source,
            &source_item.id,
            furniture_default_properties.as_ref(),
            Some(&furniture_runtime),
        );
        if mechanics.behavior.len() == 1 {
            converted.config.insert("behavior".to_string(), Value::Object(mechanics.behavior.into_iter().next().unwrap()));
        } else if mechanics.behavior.len() > 1 {
            converted.config.insert(
                "behaviors".to_string(),
                Value::Array(mechanics.behavior.into_iter().map(Value::Object).collect()),
            );
        }
        items.insert(converted.target_id.clone(), Value::Object(converted.config.clone()));
        category_items.push(CategoryItem {
            source: source_item.source.clone(),
            source_id: source_item.id.clone(),
            target_id: converted.target_id.clone(),
            config: rewritten_item.config.clone(),
        });
        if let Some(furniture_definition) = mechanics.furniture {
            // Keep generated packs reviewable: concrete CE variants belong directly to
            // their furniture ID instead of a hash-named template/argument graph.
            furniture.insert(converted.target_id.clone(), Value::Object(furniture_definition));
        }
        if let Some(block_definition) = mechanics.block {
            blocks.insert(converted.target_id.clone(), Value::Object(block_definition));
        }
        let relative_source = pathdiff::diff_paths(&source_item.source, &root)
            .unwrap_or_else(|| PathBuf::from(&source_item.source))
            .to_string_lossy()
            .replace('\\', "/");
        let mut semantics = converted.semantics.clone();
        for (key, value) in mechanics.semantics {
            semantics.insert(key, value);
        }
        mappings.insert(
            source_item.id.clone(),
            json!({
                "target": converted.target_id,
                "source": relative_source,
                "template": source_item.template_ids,
                "semantics": semantics,
            }),
        );
    }

    // rewriteText diagnostics are collected in a side bag because the closure
    // cannot share the mutable borrow held by convert_categories.
    let rewrite_bag = std::rc::Rc::new(std::cell::RefCell::new(DiagnosticBag::new()));
    let rewrite_closure: Option<Box<dyn Fn(&str, &str) -> String>> = if let Some(inventory_source) = &inventory_source {
        let entries = &glyph_conversion.entries;
        let bag = rewrite_bag.clone();
        let source = inventory_source.display().to_string();
        Some(Box::new(move |text: &str, field: &str| {
            let rewritten = rewrite_glyph_tags(
                &Value::String(text.to_string()),
                entries,
                &mut bag.borrow_mut(),
                &source,
                field,
            );
            rewritten.as_str().map(str::to_string).unwrap_or_else(|| text.to_string())
        }))
    } else {
        None
    };
    let category_options = CategoryConversionOptions {
        root: root.display().to_string(),
        namespace: namespace.clone(),
        items: category_items,
        inventory: inventory_config,
        inventory_source: inventory_source.as_ref().map(|path| path.display().to_string()),
        rewrite_text: rewrite_closure.as_deref(),
        diagnostics: &mut diagnostics,
    };
    let categories = convert_categories(category_options);
    // Drop the closure (and its Rc clone) before reclaiming the side bag.
    drop(rewrite_closure);
    diagnostics.extend(std::rc::Rc::try_unwrap(rewrite_bag).unwrap().into_inner());

    let recipes = convert_recipes(&root, &namespace, &mut diagnostics);
    let sounds_root = load_optional_object(&root.join("sounds.yml"), &mut diagnostics);
    let sounds = sounds_root
        .map(|sounds_root| convert_sounds(&sounds_root, &mut diagnostics, &root.join("sounds.yml").display().to_string()))
        .unwrap_or_default();
    let mut resource_count = 0usize;
    if let Some(pack_root) = &resource_pack_root {
        resource_count = copy_resource_pack(
            pack_root,
            &output.join("resourcepack"),
            &mut diagnostics,
            Some(&output.join("blueprint")),
        )?;
    } else {
        diagnostics.warning(
            "RESOURCE_PACK_NOT_FOUND",
            "No pack/assets, resourcepack/assets, or assets directory was found",
            crate::diagnostics::Details::new().source(root.display().to_string()).lossy(),
        );
    }
    let languages = load_optional_object(&root.join("languages.yml"), &mut diagnostics);
    if let Some(languages) = languages {
        write_language_resources(&languages, &output.join("resourcepack"), &mut diagnostics, &root.join("languages.yml").display().to_string())?;
    }

    write_yaml(
        &output.join("pack.yml"),
        &json!({
            "author": "nexo2ce",
            "version": "1.0",
            "description": "Converted from Nexo 1.26 with Minecraft semantic auditing",
            "namespace": namespace,
        }),
    )?;
    // A CE pack does not need placeholder files for feature families it does not
    // contain. Emitting blocks: {}, recipes: {}, etc. invents source categories and
    // makes reviews misleading, so create each configuration file only when it has
    // at least one converted definition.
    if !items.is_empty() {
        write_yaml(&output.join("configuration").join("items.yml"), &json!({ "items": items }))?;
    }
    if !categories.is_empty() {
        write_yaml(&output.join("configuration").join("categories.yml"), &json!({ "categories": categories }))?;
    }
    if !furniture.is_empty() {
        write_yaml(&output.join("configuration").join("furniture.yml"), &json!({ "furniture": furniture }))?;
    }
    if !blocks.is_empty() {
        write_yaml(&output.join("configuration").join("blocks.yml"), &json!({ "blocks": blocks }))?;
    }
    if !recipes.is_empty() {
        write_yaml(&output.join("configuration").join("recipes.yml"), &json!({ "recipes": recipes }))?;
    }
    if !sounds.is_empty() {
        write_yaml(&output.join("configuration").join("sounds.yml"), &json!({ "sounds": sounds }))?;
    }
    if !glyph_conversion.images.is_empty() {
        write_yaml(&output.join("configuration").join("images.yml"), &json!({ "images": glyph_conversion.images }))?;
    }
    let mut seen_sources: Vec<String> = Vec::new();
    let mut glyph_mappings = JsonObject::new();
    let mut entry_values: Vec<&crate::glyphs::GlyphEntry> = glyph_conversion.entries.values().collect();
    entry_values.sort_by(|a, b| a.source_id.cmp(&b.source_id));
    for entry in entry_values {
        if seen_sources.contains(&entry.source_id) {
            continue;
        }
        seen_sources.push(entry.source_id.clone());
        glyph_mappings.insert(
            entry.source_id.clone(),
            json!({
                "target": entry.target_id,
                "font": entry.font,
                "chars": entry.chars,
                "start_index": entry.start_index,
            }),
        );
    }
    let mut migration_mapping = JsonObject::new();
    if !mappings.is_empty() {
        migration_mapping.insert("items".to_string(), Value::Object(mappings.clone()));
    }
    if !glyph_mappings.is_empty() {
        migration_mapping.insert("glyphs".to_string(), Value::Object(glyph_mappings.clone()));
    }
    if !migration_mapping.is_empty() {
        write_yaml(&output.join("migration-mapping.yml"), &Value::Object(migration_mapping))?;
    }

    let audit = if options.audit {
        let input = AuditInput {
            resource_root: output.join("resourcepack").display().to_string(),
            items: &items,
            blocks: &blocks,
            images: Some(&glyph_conversion.images),
            blueprint_root: Some(output.join("blueprint").display().to_string()),
        };
        Some(audit_resource_graph(&input, &mut diagnostics))
    } else {
        None
    };
    let success = !diagnostics.has_errors() && !(options.strict && diagnostics.has_lossy());
    let report_file = output.join("conversion-report.json");
    let counts = diagnostics.counts();
    write_json(
        &report_file,
        &json!({
            "converter": { "name": "nexo-to-craftengine", "version": crate::VERSION, "language": "Rust" },
            "lockedReferences": {
                "nexo": { "version": crate::targets::NEXO_VERSION, "jarSha256": "FA6877A46A8C2779B0B0C78C258931DC85AECDE6E70234D91EA8624F91B75B16" },
                "craftEngine": { "version": crate::targets::CRAFTENGINE_VERSION, "commit": crate::targets::CRAFTENGINE_COMMIT },
                "itemDefinitions": "Minecraft 1.21.11 client item-definition and tint semantics",
            },
            "input": root.display().to_string(),
            "output": output.display().to_string(),
            "options": {
                "input": options.input,
                "output": options.output,
                "clientMode": options.client_mode.as_str(),
                "cmdPolicy": options.cmd_policy.as_str(),
                "strict": options.strict,
                "force": options.force,
                "audit": options.audit,
                "namespace": namespace,
                "namespaceMode": namespace_mode,
            },
            "identity": {
                "sourcePlatform": format!("Nexo {}", crate::targets::NEXO_VERSION),
                "sourceRuntimeNamespace": NEXO_ITEM_NAMESPACE,
                "authorNamespace": inferred_namespace.as_ref().map(|inference| inference.namespace.clone()),
                "targetItemNamespace": namespace,
                "namespaceMode": namespace_mode,
                "evidence": inferred_namespace.as_ref().map(|inference| inference.evidence.clone()).unwrap_or_else(|| "No unambiguous author namespace was found; used the Nexo runtime namespace fallback".to_string()),
                "candidates": inferred_namespace.as_ref().map(|inference| inference.candidates.clone()).unwrap_or_default(),
            },
            "counts": {
                "sourceItems": source_items.len(),
                "templates": template_count,
                "items": items.len(),
                "categories": categories.len(),
                "furniture": furniture.len(),
                "blocks": blocks.len(),
                "recipes": recipes.len(),
                "sounds": sounds.len(),
                "glyphs": glyph_mappings.len(),
                "images": glyph_conversion.images.len(),
                "resources": resource_count,
                "diagnostics": {
                    "info": counts.info,
                    "warning": counts.warning,
                    "error": counts.error,
                    "lossy": counts.lossy,
                },
            },
            "audit": audit.as_ref().map(|summary| json!({
                "referencedModels": summary.referenced_models,
                "resolvedModels": summary.resolved_models,
                "generatedModels": summary.generated_models,
                "referencedBlueprints": summary.referenced_blueprints,
                "missingBlueprints": summary.missing_blueprints,
                "copiedItemDefinitions": summary.copied_item_definitions,
                "referencedTextures": summary.referenced_textures,
                "resolvedTextures": summary.resolved_textures,
                "missingModels": summary.missing_models,
                "missingTextures": summary.missing_textures,
            })),
            "success": success,
            "diagnostics": diagnostics.items,
        }),
    )?;
    Ok(ConversionResult {
        success,
        report_file: Some(report_file.display().to_string()),
        item_count: items.len(),
        category_count: categories.len(),
        template_count,
        furniture_count: furniture.len(),
        block_count: blocks.len(),
        recipe_count: recipes.len(),
        sound_count: sounds.len(),
        glyph_count: glyph_conversion.images.len(),
        resource_count,
        audit,
        diagnostics,
        namespace,
        namespace_mode,
    })
}
