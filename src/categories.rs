//! Nexo inventory → CraftEngine categories conversion.
//!
//! Port of legacy/src/categories.ts. FILE mode makes every non-empty item
//! YAML a top-level category; DIRECTORY mode keeps directory parents visible
//! and references hidden subcategories by #namespace:id.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};
use unicode_normalization::UnicodeNormalization;

use crate::diagnostics::{Details, DiagnosticBag};
use crate::json::{get_boolean, get_number, get_object, get_string, JsonObject};

pub struct CategoryItem {
    pub source: String,
    pub source_id: String,
    pub target_id: String,
    pub config: JsonObject,
}

pub struct CategoryConversionOptions<'a> {
    pub root: String,
    pub namespace: String,
    pub items: Vec<CategoryItem>,
    pub inventory: Option<JsonObject>,
    pub inventory_source: Option<String>,
    pub rewrite_text: Option<&'a dyn Fn(&str, &str) -> String>,
    pub diagnostics: &'a mut DiagnosticBag,
}

struct CategoryMetadata {
    name: String,
    icon: String,
    slot: Option<usize>,
}

struct FileGroup {
    relative_file: String,
    relative_stem: String,
    items: Vec<usize>,
}

#[derive(Clone, Copy, PartialEq)]
enum NodeKind {
    Root,
    Directory,
    File,
}

struct CategoryNode {
    kind: NodeKind,
    key: String,
    label: String,
    group: Option<usize>,
    children: Vec<usize>,
    id: Option<String>,
}

struct Arena {
    nodes: Vec<CategoryNode>,
    groups: Vec<FileGroup>,
}

/// Node path.relative(parent, child), returning None when the child is not
/// strictly inside the parent.
fn inside(parent: &str, child: &str) -> Option<String> {
    let candidate = pathdiff::diff_paths(child, parent)?;
    if candidate.as_os_str().is_empty() || candidate.is_absolute() {
        return None;
    }
    let text = candidate.to_string_lossy().replace('\\', "/");
    if text == ".." || text.starts_with("../") {
        return None;
    }
    Some(text)
}

fn relative_item_file(root: &str, source: &str) -> String {
    let root_path = Path::new(root);
    for directory in [root_path.join("items"), root_path.join("item")] {
        if let Some(candidate) = inside(&directory.display().to_string(), source) {
            return candidate;
        }
    }
    Path::new(source)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| source.to_string())
}

/// Node path.extname semantics: only a dot in the final segment counts.
fn without_yaml_extension(path: &str) -> String {
    let base_start = path.rfind(['/', '\\']).map(|index| index + 1).unwrap_or(0);
    let base = &path[base_start..];
    if let Some(dot) = base.rfind('.') {
        if dot > 0 {
            let ext = &base[dot..];
            if ext.eq_ignore_ascii_case(".yml") || ext.eq_ignore_ascii_case(".yaml") {
                return path[..path.len() - ext.len()].to_string();
            }
        }
    }
    path.to_string()
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

static SLUG_INVALID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9._-]+").unwrap());
static SLUG_TRIM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[_./-]+|[_./-]+$").unwrap());

fn category_slug(path: &str) -> String {
    let segments: Vec<String> = path
        .replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let normalized: String = segment
                .nfkd()
                .filter(|character| !('\u{0300}'..='\u{036f}').contains(character))
                .collect::<String>()
                .to_lowercase();
            let replaced = SLUG_INVALID.replace_all(&normalized, "_").to_string();
            let trimmed = SLUG_TRIM.replace_all(&replaced, "").to_string();
            if trimmed.is_empty() { "category".to_string() } else { trimmed }
        })
        .collect();
    let joined = segments.join("/");
    if joined.is_empty() { "category".to_string() } else { joined }
}

fn humanize(value: &str) -> String {
    let parts: Vec<String> = value
        .split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect();
    let joined = parts.join(" ");
    if joined.is_empty() { value.to_string() } else { joined }
}

fn nested_object<'a>(root: Option<&'a JsonObject>, dotted_path: &str) -> Option<&'a JsonObject> {
    let mut current = root?;
    for part in dotted_path.split('.').filter(|part| !part.is_empty()) {
        current = get_object(current, part)?;
    }
    Some(current)
}

fn inventory_section(inventory: Option<&JsonObject>) -> JsonObject {
    let Some(inventory) = inventory else {
        return JsonObject::new();
    };
    get_object(inventory, "NexoInventory")
        .or_else(|| get_object(inventory, "nexo_inventory"))
        .cloned()
        .unwrap_or_else(|| inventory.clone())
}

fn inventory_layout(section: &JsonObject) -> Option<&JsonObject> {
    get_object(section, "layout").or_else(|| get_object(section, "menu_layout"))
}

fn layout_path(relative_path: &str, directory_mode: bool) -> String {
    if directory_mode {
        without_yaml_extension(relative_path).replace('/', ".")
    } else {
        without_yaml_extension(&basename(relative_path))
    }
}

fn first_target(arena: &Arena, items: &[CategoryItem], node_index: usize) -> Option<String> {
    let node = &arena.nodes[node_index];
    if let Some(group_index) = node.group {
        if let Some(item_index) = arena.groups[group_index].items.first() {
            return Some(items[*item_index].target_id.clone());
        }
    }
    for child in node.children.clone() {
        if let Some(target) = first_target(arena, items, child) {
            return Some(target);
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn resolve_icon(
    raw_icon: Option<&str>,
    fallback: &str,
    namespace: &str,
    source_targets: &HashMap<String, String>,
    targets: &HashSet<String>,
    diagnostics: &mut DiagnosticBag,
    source: Option<&str>,
    field: &str,
) -> String {
    let icon = raw_icon.map(str::trim).filter(|icon| !icon.is_empty());
    let Some(icon) = icon else {
        return fallback.to_string();
    };
    let separator = icon.find(':');
    let source_id = match separator {
        None => icon.to_string(),
        Some(index) => icon[index + 1..].to_string(),
    };
    let source_namespace = match separator {
        None => "nexo".to_string(),
        Some(index) => icon[..index].to_lowercase(),
    };
    if let Some(mapped) = source_targets.get(&source_id) {
        return mapped.clone();
    }
    if targets.contains(icon) {
        return icon.to_string();
    }
    if separator.is_some() && source_namespace != "nexo" {
        return icon.to_string();
    }
    let target_candidate = format!("{}:{}", namespace, source_id);
    if targets.contains(&target_candidate) {
        return target_candidate;
    }
    let mut details = Details::new().field(field).lossy();
    if let Some(source) = source {
        details = details.source(source.to_string());
    }
    diagnostics.warning(
        "CATEGORY_ICON_FALLBACK",
        &format!("Category icon {} does not identify a converted item; used {}", icon, fallback),
        details,
    );
    fallback.to_string()
}

#[allow(clippy::too_many_arguments)]
fn category_metadata(
    arena: &Arena,
    items: &[CategoryItem],
    node_index: usize,
    directory_mode: bool,
    section: &JsonObject,
    source_targets: &HashMap<String, String>,
    targets: &HashSet<String>,
    namespace: &str,
    diagnostics: &mut DiagnosticBag,
    source: Option<&str>,
    rewrite_text: Option<&dyn Fn(&str, &str) -> String>,
) -> CategoryMetadata {
    let node = &arena.nodes[node_index];
    let layout = inventory_layout(section);
    let relative_path = if node.kind == NodeKind::File {
        arena.groups[node.group.unwrap()].relative_file.clone()
    } else {
        node.key.clone()
    };
    let path = layout_path(&relative_path, directory_mode);
    let configured = nested_object(layout, &path);
    let styled_names = get_boolean(section, "style_default_names", true);
    let default_label = if styled_names {
        humanize(&node.label)
    } else if node.kind == NodeKind::File {
        basename(&arena.groups[node.group.unwrap()].relative_file)
    } else {
        node.label.clone()
    };
    let configured_name = configured.and_then(|configured| {
        get_string(configured, "itemname")
            .or_else(|| get_string(configured, "displayname"))
            .or_else(|| get_string(configured, "title"))
    });
    let rendered_name = match (configured_name, rewrite_text) {
        (Some(name), Some(rewrite)) => Some(rewrite(name, &format!("NexoInventory.layout.{}.name", path))),
        (Some(name), None) => Some(name.to_string()),
        (None, _) => None,
    };
    let fallback = first_target(arena, items, node_index).unwrap_or_else(|| "minecraft:stone".to_string());
    let configured_icon = configured.and_then(|configured| get_string(configured, "icon"));
    let directory_icon = if node.kind == NodeKind::Directory {
        get_string(section, "directory_icon")
    } else {
        None
    };
    let icon = resolve_icon(
        configured_icon.or(directory_icon),
        &fallback,
        namespace,
        source_targets,
        targets,
        diagnostics,
        source,
        &format!("NexoInventory.layout.{}.icon", path),
    );
    let raw_slot = configured.and_then(|configured| get_number(configured, "slot"));
    let slot = raw_slot
        .filter(|slot| slot.fract() == 0.0 && *slot > 0.0)
        .map(|slot| (slot - 1.0) as usize);
    CategoryMetadata {
        name: format!("<!i><green>{}</green>", rendered_name.unwrap_or(default_label)),
        icon,
        slot,
    }
}

struct PriorityEntry {
    key: String,
    node_index: usize,
    slot: Option<usize>,
}

/// Assigns conflict-free priorities in place; mirrors TS assignPriorities.
fn assign_priorities(entries: &mut [PriorityEntry], diagnostics: &mut DiagnosticBag, source: Option<&str>) {
    let mut used: HashSet<usize> = HashSet::new();
    let mut pending: Vec<usize> = Vec::new();
    for index in 0..entries.len() {
        match entries[index].slot {
            Some(requested) if !used.contains(&requested) => {
                used.insert(requested);
            }
            requested => {
                if let Some(requested) = requested {
                    let mut details = Details::new().field("NexoInventory.layout").lossy();
                    if let Some(source) = source {
                        details = details.source(source.to_string());
                    }
                    diagnostics.warning(
                        "CATEGORY_SLOT_CONFLICT",
                        &format!(
                            "Multiple Nexo inventory entries request slot {}; assigned the next free position",
                            requested + 1
                        ),
                        details,
                    );
                }
                pending.push(index);
            }
        }
    }
    let mut cursor = 0usize;
    for index in pending {
        while used.contains(&cursor) {
            cursor += 1;
        }
        entries[index].slot = Some(cursor);
        used.insert(cursor);
        cursor += 1;
    }
    entries.sort_by(|a, b| a.slot.unwrap().cmp(&b.slot.unwrap()).then_with(|| a.key.cmp(&b.key)));
}

fn allocate_ids(arena: &mut Arena, node_indexes: &[usize], namespace: &str) {
    let mut order: Vec<usize> = node_indexes.to_vec();
    order.sort_by(|a, b| arena.nodes[*a].key.cmp(&arena.nodes[*b].key));
    let mut used: HashSet<String> = HashSet::new();
    for node_index in order {
        let base = category_slug(&arena.nodes[node_index].key.clone());
        let mut path = base.clone();
        let mut suffix = 2usize;
        while used.contains(&path) {
            path = format!("{}-{}", base, suffix);
            suffix += 1;
        }
        used.insert(path.clone());
        arena.nodes[node_index].id = Some(format!("{}:{}", namespace, path));
    }
}

fn converted_groups(options: &CategoryConversionOptions) -> Vec<FileGroup> {
    let mut groups: Vec<FileGroup> = Vec::new();
    for item_index in 0..options.items.len() {
        let item = &options.items[item_index];
        if get_boolean(&item.config, "excludeFromInventory", false) {
            continue;
        }
        let relative_file = relative_item_file(&options.root, &item.source);
        let position = groups.iter().position(|group| group.relative_file == relative_file);
        let group_index = match position {
            Some(position) => position,
            None => {
                groups.push(FileGroup {
                    relative_stem: without_yaml_extension(&relative_file),
                    relative_file,
                    items: Vec::new(),
                });
                groups.len() - 1
            }
        };
        groups[group_index].items.push(item_index);
    }
    groups.sort_by(|a, b| a.relative_file.cmp(&b.relative_file));
    groups
}

/// JS Map from entries keeps the FIRST value for duplicate keys.
fn build_source_maps(options: &CategoryConversionOptions) -> (HashMap<String, String>, HashSet<String>) {
    let mut source_targets: HashMap<String, String> = HashMap::new();
    let mut targets: HashSet<String> = HashSet::new();
    for item in &options.items {
        source_targets.entry(item.source_id.clone()).or_insert_with(|| item.target_id.clone());
        targets.insert(item.target_id.clone());
    }
    (source_targets, targets)
}

fn convert_file_categories(
    options: &mut CategoryConversionOptions,
    section: &JsonObject,
    groups: Vec<FileGroup>,
) -> JsonObject {
    let (source_targets, targets) = build_source_maps(options);
    let mut arena = Arena { nodes: Vec::new(), groups };
    for group_index in 0..arena.groups.len() {
        let group = &arena.groups[group_index];
        let label = without_yaml_extension(&basename(&group.relative_file));
        arena.nodes.push(CategoryNode {
            kind: NodeKind::File,
            key: group.relative_stem.clone(),
            label,
            group: Some(group_index),
            children: Vec::new(),
            id: None,
        });
    }
    let node_indexes: Vec<usize> = (0..arena.nodes.len()).collect();
    allocate_ids(&mut arena, &node_indexes, &options.namespace);
    let diagnostics = &mut *options.diagnostics;
    let mut metadata_by_node: Vec<CategoryMetadata> = Vec::new();
    let mut entries: Vec<PriorityEntry> = Vec::new();
    for node_index in &node_indexes {
        let metadata = category_metadata(
            &arena,
            &options.items,
            *node_index,
            false,
            section,
            &source_targets,
            &targets,
            &options.namespace,
            diagnostics,
            options.inventory_source.as_deref(),
            options.rewrite_text,
        );
        entries.push(PriorityEntry {
            key: arena.nodes[*node_index].key.clone(),
            node_index: *node_index,
            slot: metadata.slot,
        });
        metadata_by_node.push(metadata);
    }
    assign_priorities(&mut entries, diagnostics, options.inventory_source.as_deref());
    let mut categories = JsonObject::new();
    for entry in &entries {
        let node = &arena.nodes[entry.node_index];
        let group = &arena.groups[node.group.unwrap()];
        let list: Vec<Value> = group
            .items
            .iter()
            .map(|item_index| Value::String(options.items[*item_index].target_id.clone()))
            .collect();
        let metadata = &metadata_by_node[entry.node_index];
        categories.insert(
            node.id.clone().unwrap(),
            json!({
                "name": metadata.name,
                "icon": metadata.icon,
                "priority": entry.slot.unwrap(),
                "list": list,
            }),
        );
    }
    categories
}

fn build_directory_tree(arena: &mut Arena) -> usize {
    let root_index = arena.nodes.len();
    arena.nodes.push(CategoryNode {
        kind: NodeKind::Root,
        key: String::new(),
        label: String::new(),
        group: None,
        children: Vec::new(),
        id: None,
    });
    let mut directories: HashMap<String, usize> = HashMap::new();
    directories.insert(String::new(), root_index);
    for group_index in 0..arena.groups.len() {
        let parts: Vec<String> = arena.groups[group_index]
            .relative_stem
            .split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect();
        let mut parent_index = root_index;
        let mut directory_path = String::new();
        for part in &parts[..parts.len().saturating_sub(1)] {
            directory_path = if directory_path.is_empty() {
                part.clone()
            } else {
                format!("{}/{}", directory_path, part)
            };
            let existing = directories.get(&directory_path).copied();
            let directory_index = match existing {
                Some(index) => index,
                None => {
                    let index = arena.nodes.len();
                    arena.nodes.push(CategoryNode {
                        kind: NodeKind::Directory,
                        key: directory_path.clone(),
                        label: part.clone(),
                        group: None,
                        children: Vec::new(),
                        id: None,
                    });
                    directories.insert(directory_path.clone(), index);
                    arena.nodes[parent_index].children.push(index);
                    index
                }
            };
            parent_index = directory_index;
        }
        let file_index = arena.nodes.len();
        arena.nodes.push(CategoryNode {
            kind: NodeKind::File,
            key: arena.groups[group_index].relative_stem.clone(),
            label: parts.last().cloned().unwrap_or_else(|| arena.groups[group_index].relative_stem.clone()),
            group: Some(group_index),
            children: Vec::new(),
            id: None,
        });
        arena.nodes[parent_index].children.push(file_index);
    }
    root_index
}

fn flatten_nodes(arena: &Arena, root_index: usize) -> Vec<usize> {
    let mut result = Vec::new();
    fn visit(arena: &Arena, node_index: usize, result: &mut Vec<usize>) {
        for child in arena.nodes[node_index].children.clone() {
            result.push(child);
            visit(arena, child, result);
        }
    }
    visit(arena, root_index, &mut result);
    result
}

fn convert_directory_categories(
    options: &mut CategoryConversionOptions,
    section: &JsonObject,
    groups: Vec<FileGroup>,
) -> JsonObject {
    let (source_targets, targets) = build_source_maps(options);
    let mut arena = Arena { nodes: Vec::new(), groups };
    let root_index = build_directory_tree(&mut arena);
    let nodes = flatten_nodes(&arena, root_index);
    allocate_ids(&mut arena, &nodes, &options.namespace);

    let diagnostics = &mut *options.diagnostics;
    let mut metadata_by_node: HashMap<usize, CategoryMetadata> = HashMap::new();
    for node_index in &nodes {
        let metadata = category_metadata(
            &arena,
            &options.items,
            *node_index,
            true,
            section,
            &source_targets,
            &targets,
            &options.namespace,
            diagnostics,
            options.inventory_source.as_deref(),
            options.rewrite_text,
        );
        metadata_by_node.insert(*node_index, metadata);
    }

    // Order every sibling group by priority, recursing depth-first.
    fn order_children(
        arena: &Arena,
        metadata_by_node: &HashMap<usize, CategoryMetadata>,
        parent_index: usize,
        diagnostics: &mut DiagnosticBag,
        source: Option<&str>,
    ) -> Vec<usize> {
        let mut entries: Vec<PriorityEntry> = arena.nodes[parent_index]
            .children
            .iter()
            .map(|child_index| PriorityEntry {
                key: arena.nodes[*child_index].key.clone(),
                node_index: *child_index,
                slot: metadata_by_node[child_index].slot,
            })
            .collect();
        assign_priorities(&mut entries, diagnostics, source);
        entries.iter().map(|entry| entry.node_index).collect()
    }
    fn order_recursive(
        arena: &mut Arena,
        metadata_by_node: &HashMap<usize, CategoryMetadata>,
        parent_index: usize,
        diagnostics: &mut DiagnosticBag,
        source: Option<&str>,
    ) {
        let ordered = order_children(arena, metadata_by_node, parent_index, diagnostics, source);
        arena.nodes[parent_index].children = ordered.clone();
        for child_index in ordered {
            order_recursive(arena, metadata_by_node, child_index, diagnostics, source);
        }
    }
    order_recursive(&mut arena, &metadata_by_node, root_index, diagnostics, options.inventory_source.as_deref());

    let top_level: HashSet<usize> = arena.nodes[root_index].children.iter().copied().collect();
    let mut categories = JsonObject::new();
    for node_index in &nodes {
        let node = &arena.nodes[*node_index];
        let metadata = &metadata_by_node[node_index];
        let list: Vec<Value> = if node.kind == NodeKind::File {
            let group = &arena.groups[node.group.unwrap()];
            group
                .items
                .iter()
                .map(|item_index| Value::String(options.items[*item_index].target_id.clone()))
                .collect()
        } else {
            node.children
                .iter()
                .map(|child_index| Value::String(format!("#{}", arena.nodes[*child_index].id.clone().unwrap())))
                .collect()
        };
        let mut entry = JsonObject::new();
        entry.insert("name".to_string(), Value::String(metadata.name.clone()));
        entry.insert("icon".to_string(), Value::String(metadata.icon.clone()));
        entry.insert("list".to_string(), Value::Array(list));
        if top_level.contains(node_index) {
            entry.insert("priority".to_string(), Value::from(metadata.slot.unwrap()));
        } else {
            entry.insert("hidden".to_string(), Value::Bool(true));
        }
        categories.insert(node.id.clone().unwrap(), Value::Object(entry));
    }
    categories
}

pub fn convert_categories(mut options: CategoryConversionOptions) -> JsonObject {
    let groups = converted_groups(&options);
    if groups.is_empty() {
        return JsonObject::new();
    }
    let section = inventory_section(options.inventory.as_ref());
    let raw_type = get_string(&section, "type").map(|value| value.trim().to_uppercase());
    let directory_mode = raw_type.as_deref() == Some("DIRECTORY");
    if let Some(raw_type) = &raw_type {
        if raw_type != "FILE" && raw_type != "DIRECTORY" {
            let mut details = Details::new().field("NexoInventory.type").lossy();
            if let Some(source) = &options.inventory_source {
                details = details.source(source.clone());
            }
            options.diagnostics.warning(
                "CATEGORY_INVENTORY_TYPE_INVALID",
                &format!("Unknown Nexo inventory type {}; used FILE", raw_type),
                details,
            );
        }
    }
    if directory_mode {
        convert_directory_categories(&mut options, &section, groups)
    } else {
        convert_file_categories(&mut options, &section, groups)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_and_humanize_match_legacy_shapes() {
        assert_eq!(category_slug("seasonal/decor"), "seasonal/decor");
        assert_eq!(category_slug("Über Café"), "uber_cafe");
        assert_eq!(humanize("seasonal_decor"), "Seasonal Decor");
        assert_eq!(without_yaml_extension("items/tools.yml"), "items/tools");
        assert_eq!(without_yaml_extension("items/tools.YAML"), "items/tools");
        assert_eq!(without_yaml_extension("items/tools.txt"), "items/tools.txt");
    }

    #[test]
    fn empty_items_yield_no_categories() {
        let mut diagnostics = DiagnosticBag::new();
        let options = CategoryConversionOptions {
            root: "root".to_string(),
            namespace: "demo".to_string(),
            items: Vec::new(),
            inventory: None,
            inventory_source: None,
            rewrite_text: None,
            diagnostics: &mut diagnostics,
        };
        assert!(convert_categories(options).is_empty());
    }
}
