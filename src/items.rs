//! Nexo item conversion: template resolution, placeholders, components,
//! potion effects, attributes, PDC and the final CraftEngine item shape.
//!
//! Port of `legacy/src/items.ts`.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use crate::component_builders::{convert_nexo_builder_component, BuilderStatus};
use crate::data::materials::is_bukkit_material;
use crate::diagnostics::{Details, DiagnosticBag};
use crate::json::{
    as_string_list, deep_merge, find_key, get_number, get_object, get_string, get_value, without_keys, JsonObject,
};
use crate::models::{convert_models, read_pack_model, ModelContext};
use crate::resource_location::{normalize_location, normalize_model_location};
use crate::ClientMode;

#[derive(Debug, Clone)]
pub struct SourceItem {
    pub id: String,
    pub source: String,
    pub config: JsonObject,
    pub template: bool,
}

#[derive(Debug, Clone)]
pub struct ResolvedItem {
    pub id: String,
    pub source: String,
    pub config: JsonObject,
    pub template: bool,
    pub template_ids: Vec<String>,
}

pub struct ItemOptions<'a> {
    pub namespace: String,
    pub client_mode: ClientMode,
    pub model_aliases: Option<&'a HashMap<String, String>>,
}

#[derive(Debug, Clone)]
pub struct ConvertedItem {
    pub source_id: String,
    pub target_id: String,
    pub config: JsonObject,
    pub model_pointer: Option<String>,
    pub base_model: Option<String>,
    pub semantics: JsonObject,
}

// Minecraft's armor-dye recipe accepts the vanilla `minecraft:dyeable` tag.
// CraftEngine custom items should opt in explicitly so CE registers its custom
// dye recipe instead of relying on the backing vanilla material as an implicit
// fallback. This is the canonical `settings.dyeable` form documented by CE.
const VANILLA_DYEABLE_MATERIALS: &[&str] = &[
    "leather_helmet",
    "leather_chestplate",
    "leather_leggings",
    "leather_boots",
    "leather_horse_armor",
    "wolf_armor",
];

pub fn match_bukkit_material(value: Option<&Value>) -> Option<String> {
    let Value::String(text) = value? else { return None };
    let stripped = text.strip_prefix("minecraft:").unwrap_or(text);
    let candidate: String = stripped
        .to_uppercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<String>()
        .to_lowercase();
    if is_bukkit_material(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

fn capitalize_id(id: &str) -> String {
    id.split('_')
        .map(|part| {
            if part.is_empty() {
                String::new()
            } else {
                let mut chars = part.chars();
                let first = chars.next().unwrap().to_uppercase().to_string();
                format!("{}{}", first, chars.as_str())
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

enum Replacement {
    Single(String),
    Many(Vec<String>),
}

fn placeholder_values(id: &str, item: &JsonObject) -> Vec<(String, Replacement)> {
    let pack = get_object(item, "Pack");
    let parent = pack
        .and_then(|p| get_string(p, "parent_model").or_else(|| get_string(p, "parent")))
        .unwrap_or("minecraft:item/generated");
    let model = pack.and_then(|p| get_string(p, "model")).unwrap_or(id);
    vec![
        ("item_id".to_string(), Replacement::Single(id.to_string())),
        ("item_id_capitalized".to_string(), Replacement::Single(capitalize_id(id))),
        ("lore".to_string(), Replacement::Many(as_string_list(get_value(item, "lore")))),
        ("parent".to_string(), Replacement::Single(parent.to_string())),
        ("model".to_string(), Replacement::Single(model.to_string())),
        (
            "texture".to_string(),
            Replacement::Many(pack.map(|p| as_string_list(get_value(p, "texture"))).unwrap_or_default()),
        ),
    ]
}

fn expand_string(input: &str, replacements: &[(String, Replacement)]) -> Replacement {
    let mut values: Vec<String> = vec![input.to_string()];
    for (key, replacement) in replacements {
        let token = format!("<{}>", key);
        if !values.iter().any(|value| value.contains(&token)) {
            continue;
        }
        let alternatives: &[String] = match replacement {
            Replacement::Single(single) => std::slice::from_ref(single),
            Replacement::Many(many) => many,
        };
        if alternatives.is_empty() {
            return Replacement::Many(Vec::new());
        }
        values = values
            .iter()
            .flat_map(|value| alternatives.iter().map(|entry| value.replace(&token, entry)))
            .collect();
    }
    if values.len() == 1 {
        Replacement::Single(values.into_iter().next().unwrap())
    } else {
        Replacement::Many(values)
    }
}

fn apply_placeholders(value: &Value, replacements: &[(String, Replacement)]) -> Value {
    match value {
        Value::String(text) => match expand_string(text, replacements) {
            Replacement::Single(single) => Value::String(single),
            Replacement::Many(many) => Value::Array(many.into_iter().map(Value::String).collect()),
        },
        Value::Array(entries) => {
            let mut result: Vec<Value> = Vec::new();
            for entry in entries {
                let converted = apply_placeholders(entry, replacements);
                if entry.is_string() {
                    if let Value::Array(expanded) = converted {
                        result.extend(expanded);
                        continue;
                    }
                }
                result.push(converted);
            }
            Value::Array(result)
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, entry)| (key.clone(), apply_placeholders(entry, replacements)))
                .collect(),
        ),
        other => other.clone(),
    }
}

static PLACEHOLDER_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<(item_id|item_id_capitalized|lore|parent|model|texture)>").unwrap());

fn has_placeholder(value: &Value) -> bool {
    match value {
        Value::String(text) => PLACEHOLDER_TOKEN.is_match(text),
        Value::Array(entries) => entries.iter().any(has_placeholder),
        Value::Object(map) => map.values().any(has_placeholder),
        _ => false,
    }
}

pub fn identify_templates(items: &[SourceItem]) -> HashSet<String> {
    let mut referenced = HashSet::new();
    for item in items {
        for id in as_string_list(get_value(&item.config, "template")) {
            referenced.insert(id);
        }
    }
    referenced
}

pub fn resolve_item_templates(items: &[SourceItem], diagnostics: &mut DiagnosticBag) -> Vec<ResolvedItem> {
    let by_id: HashMap<&str, &SourceItem> = items.iter().map(|item| (item.id.as_str(), item)).collect();
    let template_ids = identify_templates(items);
    let mut cache: HashMap<String, JsonObject> = HashMap::new();

    fn resolve_config(
        item: &SourceItem,
        stack: &[String],
        by_id: &HashMap<&str, &SourceItem>,
        cache: &mut HashMap<String, JsonObject>,
        diagnostics: &mut DiagnosticBag,
    ) -> JsonObject {
        if let Some(cached) = cache.get(&item.id) {
            return cached.clone();
        }
        if stack.contains(&item.id) {
            let chain: Vec<&str> = stack.iter().map(String::as_str).chain(std::iter::once(item.id.as_str())).collect();
            diagnostics.error(
                "TEMPLATE_CYCLE",
                &format!("Template cycle: {}", chain.join(" -> ")),
                Details::new().source(item.source.clone()).item(item.id.clone()).field("template"),
            );
            return item.config.clone();
        }
        let mut merged = JsonObject::new();
        let references = as_string_list(get_value(&item.config, "template"));
        for template_id in &references {
            let Some(template) = by_id.get(template_id.as_str()) else {
                diagnostics.error(
                    "TEMPLATE_NOT_FOUND",
                    &format!("Nexo template not found: {}", template_id),
                    Details::new().source(item.source.clone()).item(item.id.clone()).field("template").lossy(),
                );
                continue;
            };
            let mut stack = stack.to_vec();
            stack.push(item.id.clone());
            let resolved_template = without_keys(&resolve_config(template, &stack, by_id, cache, diagnostics), &["injectId"]);
            merged = deep_merge(&merged, &resolved_template);
        }
        let mut own_config = without_keys(&item.config, &["template"]);
        if own_config.contains_key("material") && match_bukkit_material(own_config.get("material")).is_none() {
            diagnostics.info(
                "INVALID_MATERIAL_INHERITED",
                "Nexo ignores an invalid material and inherits its template material, or PAPER when no template supplies one",
                Details::new().source(item.source.clone()).item(item.id.clone()).field("material"),
            );
            own_config.shift_remove("material");
        }
        merged = deep_merge(&merged, &own_config);
        cache.insert(item.id.clone(), merged.clone());
        merged
    }

    items
        .iter()
        .map(|item| {
            let raw = resolve_config(item, &[], &by_id, &mut cache, diagnostics);
            let replacements = placeholder_values(&item.id, &item.config);
            let converted = apply_placeholders(&Value::Object(raw.clone()), &replacements);
            let config = match converted {
                Value::Object(map) => map,
                _ => raw,
            };
            if has_placeholder(&Value::Object(config.clone())) {
                diagnostics.error(
                    "TEMPLATE_PLACEHOLDER_UNRESOLVED",
                    "A supported Nexo template placeholder could not be resolved",
                    Details::new().source(item.source.clone()).item(item.id.clone()).lossy(),
                );
            }
            ResolvedItem {
                id: item.id.clone(),
                source: item.source.clone(),
                template: template_ids.contains(&item.id),
                template_ids: as_string_list(get_value(&item.config, "template")),
                config,
            }
        })
        .collect()
}

fn normalize_component_name(name: &str) -> Option<String> {
    let separator = name.find(':');
    let namespace = match separator {
        Some(index) => &name[..index],
        None => "minecraft",
    };
    let path = match separator {
        Some(index) => &name[index + 1..],
        None => name,
    };
    let valid_ns = !namespace.is_empty() && namespace.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-'));
    let valid_path = !path.is_empty() && path.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '/' | '.' | '_' | '-'));
    if !valid_ns || !valid_path {
        return None;
    }
    if namespace == "minecraft" {
        Some(path.to_string())
    } else {
        Some(format!("{}:{}", namespace, path))
    }
}

const NEXO_COMPONENT_KEYS: &[&str] = &[
    "unset_components", "can_place_on", "can_break", "custom_data", "max_stack_size", "instrument", "enchantment_glint_override",
    "max_damage", "rarity", "food", "tool", "painting_variant", "tooltip_style", "item_model", "jukebox_playable", "use_remainder",
    "death_protection", "use_cooldown", "damage_resistant", "consumable", "equippable", "enchantable", "glider", "repairable", "profile",
    "custom_model_data", "tooltip_display", "break_sound", "weapon", "blocks_attacks", "attack_range", "kinetic_weapon", "piercing_weapon",
    "minimum_attack_charge", "swing_animation", "use_effects", "damage_type",
];

fn bukkit_int(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(number)) => number
            .as_f64()
            .filter(|value| value.is_finite())
            .map(|value| value.trunc() as i64)
            .unwrap_or(0),
        _ => 0,
    }
}

fn bukkit_float(value: Option<&Value>, fallback: f64) -> f64 {
    match value {
        Some(Value::Number(number)) => number.as_f64().filter(|value| value.is_finite()).unwrap_or(fallback),
        _ => fallback,
    }
}

fn bukkit_string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(text)) => {
            if text.is_empty() {
                Vec::new()
            } else {
                vec![text.clone()]
            }
        }
        Some(Value::Array(entries)) => entries
            .iter()
            .filter_map(|entry| match entry {
                Value::String(text) => Some(text.clone()),
                Value::Number(number) => {
                    if let Some(int) = number.as_i64() {
                        Some(int.to_string())
                    } else {
                        number.as_f64().map(|float| float.to_string())
                    }
                }
                Value::Bool(flag) => Some(flag.to_string()),
                _ => None,
            })
            .filter(|entry| !entry.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn component_location(
    value: Option<&Value>,
    key: &str,
    diagnostics: &mut DiagnosticBag,
    source: &str,
    item: &str,
) -> Option<String> {
    let Value::String(text) = value? else { return None };
    if text.is_empty() {
        return None;
    }
    normalize_location(
        text,
        diagnostics,
        &Details::new().source(source).item(item).field(format!("Components.{}", key)),
        &[],
        "minecraft",
    )
}

static DURATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(-?(?:\d+(?:\.\d*)?|\.\d+))\s*(ms|ticks?|t|s|sec(?:onds?)?|m|min(?:utes?)?|h|hours?)?\s*$").unwrap()
});

fn duration_seconds(value: Option<&Value>) -> f64 {
    let number = match value {
        Some(Value::Number(number)) => match number.as_f64().filter(|value| value.is_finite()) {
            Some(value) => return value.max(0.0),
            None => return 0.0,
        },
        Some(Value::String(text)) => text.clone(),
        _ => return 0.0,
    };
    let Some(captures) = DURATION.captures(&number) else {
        return 0.0;
    };
    let amount: f64 = captures[1].parse().unwrap_or(0.0);
    let unit = captures.get(2).map(|m| m.as_str().to_lowercase()).unwrap_or_else(|| "s".to_string());
    let multiplier = if unit == "ms" {
        0.001
    } else if unit == "t" || unit.starts_with("tick") {
        0.05
    } else if unit == "m" || unit.starts_with("min") {
        60.0
    } else if unit == "h" || unit.starts_with("hour") {
        3600.0
    } else {
        1.0
    };
    (amount * multiplier).max(0.0)
}

fn section_entries(value: Option<&Value>) -> Vec<JsonObject> {
    let Some(value) = value else { return Vec::new() };
    match value {
        Value::Array(entries) => entries.iter().filter_map(|entry| entry.as_object().cloned()).collect(),
        Value::Object(map) => {
            if map.get("name").map(Value::is_string).unwrap_or(false)
                || map.get("value").map(Value::is_string).unwrap_or(false)
            {
                return vec![map.clone()];
            }
            map.values().filter_map(|entry| entry.as_object().cloned()).collect()
        }
        _ => Vec::new(),
    }
}

static PROFILE_UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$").unwrap()
});

fn convert_profile(raw: &JsonObject, diagnostics: &mut DiagnosticBag, source: &str, item: &str) -> Option<JsonObject> {
    let mut result = JsonObject::new();
    if let Some(Value::String(name)) = raw.get("name") {
        if !name.is_empty() {
            result.insert("name".to_string(), Value::String(name.clone()));
        }
    }
    if let Some(Value::String(uuid)) = raw.get("uuid") {
        if !PROFILE_UUID.is_match(uuid) {
            diagnostics.error(
                "COMPONENT_PROFILE_UUID_INVALID",
                "Nexo UUID.fromString rejects this Components.profile.uuid",
                Details::new().source(source).item(item).field("Components.profile.uuid"),
            );
            return None;
        }
        result.insert("id".to_string(), Value::String(uuid.clone()));
    }
    let properties: Vec<JsonObject> = section_entries(raw.get("property"))
        .into_iter()
        .chain(section_entries(raw.get("properties")))
        .collect();
    let mut converted: Vec<JsonObject> = Vec::new();
    for (index, property) in properties.iter().enumerate() {
        let (Some(Value::String(name)), Some(Value::String(value))) = (property.get("name"), property.get("value")) else {
            diagnostics.error(
                "COMPONENT_PROFILE_PROPERTY_INVALID",
                "Nexo requires string name and value for every profile property",
                Details::new()
                    .source(source)
                    .item(item)
                    .field(format!("Components.profile.properties[{}]", index)),
            );
            return None;
        };
        let mut entry = JsonObject::new();
        entry.insert("name".to_string(), Value::String(name.clone()));
        entry.insert("value".to_string(), Value::String(value.clone()));
        if let Some(Value::String(signature)) = property.get("signature") {
            entry.insert("signature".to_string(), Value::String(signature.clone()));
        }
        converted.push(entry);
    }
    if !converted.is_empty() {
        result.insert(
            "properties".to_string(),
            Value::Array(converted.into_iter().map(Value::Object).collect()),
        );
    }
    Some(result)
}

fn convert_custom_model_data(raw: &JsonObject, diagnostics: &mut DiagnosticBag, source: &str, item: &str) -> Option<JsonObject> {
    let colors: Vec<i64> = bukkit_string_list(raw.get("color").or_else(|| raw.get("colors")))
        .iter()
        .filter_map(|text| nexo_color(Some(&Value::String(text.clone()))).map(|value| value as i64))
        .collect();
    let mut floats: Vec<f64> = Vec::new();
    for (index, text) in bukkit_string_list(raw.get("float").or_else(|| raw.get("floats"))).iter().enumerate() {
        match text.trim().parse::<f64>() {
            Ok(parsed) if parsed.is_finite() => floats.push(parsed),
            _ => {
                diagnostics.error(
                    "COMPONENT_CMD_FLOAT_INVALID",
                    "Nexo Float.parseFloat rejects this custom_model_data float",
                    Details::new()
                        .source(source)
                        .item(item)
                        .field(format!("Components.custom_model_data.float[{}]", index)),
                );
                return None;
            }
        }
    }
    let strings = bukkit_string_list(raw.get("string").or_else(|| raw.get("strings")));
    let flags: Vec<bool> = bukkit_string_list(raw.get("flag").or_else(|| raw.get("flags")))
        .iter()
        .filter(|text| text.as_str() == "true" || text.as_str() == "false")
        .map(|text| text.as_str() == "true")
        .collect();
    let mut result = JsonObject::new();
    if !colors.is_empty() {
        result.insert("colors".to_string(), Value::Array(colors.into_iter().map(Value::from).collect()));
    }
    if !floats.is_empty() {
        result.insert("floats".to_string(), Value::Array(floats.into_iter().map(Value::from).collect()));
    }
    if !strings.is_empty() {
        result.insert("strings".to_string(), Value::Array(strings.into_iter().map(Value::String).collect()));
    }
    if !flags.is_empty() {
        result.insert("flags".to_string(), Value::Array(flags.into_iter().map(Value::Bool).collect()));
    }
    Some(result)
}

#[allow(clippy::too_many_arguments)]
fn map_components(
    components: &JsonObject,
    data: &mut JsonObject,
    diagnostics: &mut DiagnosticBag,
    source: &str,
    item: &str,
    material: &str,
) -> Option<String> {
    let mut copied = JsonObject::new();
    let mut item_model: Option<String> = None;
    for (key, raw_value) in components {
        // Bukkit ConfigurationSection paths and Nexo's parser are case-sensitive;
        // do not invent aliases by lowercasing source keys.
        if key == "unset_components" || key == "unset_component" {
            continue;
        }
        if key == "potion_contents" {
            diagnostics.info(
                "NEXO_COMPONENT_POTION_CONTENTS_IGNORED",
                "Nexo 1.26 ComponentParser does not recognize Components.potion_contents; it was intentionally not emitted",
                Details::new().source(source).item(item).field("Components.potion_contents"),
            );
            continue;
        }
        if !NEXO_COMPONENT_KEYS.contains(&key.as_str()) {
            diagnostics.info(
                "NEXO_COMPONENT_UNKNOWN_IGNORED",
                &format!("Nexo 1.26 ComponentParser ignores unsupported or differently-cased component key {}", key),
                Details::new().source(source).item(item).field(format!("Components.{}", key)),
            );
            continue;
        }
        if key == "item_model" {
            let Value::String(text) = raw_value else {
                diagnostics.error(
                    "ITEM_MODEL_COMPONENT_INVALID",
                    "Components.item_model must be a resource-location string",
                    Details::new().source(source).item(item).field("Components.item_model"),
                );
                continue;
            };
            item_model = normalize_location(
                text,
                diagnostics,
                &Details::new().source(source).item(item).field("Components.item_model"),
                &[],
                "minecraft",
            );
            continue;
        }
        if let Some(builder) = convert_nexo_builder_component(key, raw_value, diagnostics, source, item, components, material) {
            if builder.status == BuilderStatus::Manual {
                diagnostics.warning(
                    "COMPONENT_CODEC_MANUAL",
                    &format!(
                        "Components.{} cannot be resolved statically: {}",
                        key,
                        builder.reason.unwrap_or_else(|| "runtime registry data is required".to_string())
                    ),
                    Details::new().source(source).item(item).field(format!("Components.{}", key)).lossy(),
                );
            } else if let Some(value) = builder.value {
                copied.insert(key.clone(), value);
            }
            continue;
        }
        match key.as_str() {
            "custom_data" => {
                if let Value::Object(map) = raw_value {
                    copied.insert("custom_data".to_string(), Value::Object(map.clone()));
                }
            }
            "max_stack_size" => {
                copied.insert("max_stack_size".to_string(), Value::from(bukkit_int(Some(raw_value)).clamp(1, 99)));
            }
            "enchantment_glint_override" => match raw_value {
                Value::Bool(flag) => {
                    copied.insert("enchantment_glint_override".to_string(), Value::Bool(*flag));
                }
                Value::String(text) if text == "true" || text == "false" => {
                    copied.insert("enchantment_glint_override".to_string(), Value::Bool(text == "true"));
                }
                _ => {}
            },
            "max_damage" => {
                copied.insert("max_damage".to_string(), Value::from(bukkit_int(Some(raw_value)).max(1)));
            }
            "rarity" => {
                if let Value::String(text) = raw_value {
                    let lower = text.to_lowercase();
                    if ["common", "uncommon", "rare", "epic"].contains(&lower.as_str()) {
                        copied.insert("rarity".to_string(), Value::String(lower));
                    }
                }
            }
            "food" => {
                if let Value::Object(raw) = raw_value {
                    copied.insert(
                        "food".to_string(),
                        json!({
                            "nutrition": bukkit_int(raw.get("nutrition")),
                            "saturation": bukkit_float(raw.get("saturation"), 0.0),
                            "can_always_eat": raw.get("can_always_eat") == Some(&Value::Bool(true)),
                        }),
                    );
                }
            }
            "painting_variant" => {
                if let Some(value) = component_location(Some(raw_value), key, diagnostics, source, item) {
                    copied.insert("painting/variant".to_string(), Value::String(value));
                }
            }
            "instrument" | "tooltip_style" | "break_sound" | "damage_type" => {
                if let Some(value) = component_location(Some(raw_value), key, diagnostics, source, item) {
                    copied.insert(key.clone(), Value::String(value));
                    diagnostics.info(
                        "COMPONENT_REGISTRY_UNVERIFIED",
                        &format!("Registry-backed component {} was syntax-validated but must exist on the target server", key),
                        Details::new().source(source).item(item).field(format!("Components.{}", key)),
                    );
                }
            }
            "use_cooldown" => {
                if let Value::Object(raw) = raw_value {
                    let group = component_location(raw.get("group"), "use_cooldown.group", diagnostics, source, item)
                        .unwrap_or_else(|| format!("nexo:{}", item));
                    copied.insert(
                        "use_cooldown".to_string(),
                        json!({ "seconds": duration_seconds(raw.get("duration")), "cooldown_group": group }),
                    );
                }
            }
            "damage_resistant" => {
                if let Some(value) = component_location(Some(raw_value), key, diagnostics, source, item) {
                    copied.insert("damage_resistant".to_string(), json!({ "types": format!("#{}", value) }));
                    diagnostics.info(
                        "COMPONENT_REGISTRY_UNVERIFIED",
                        "Damage-type tag existence must be checked on the target server",
                        Details::new().source(source).item(item).field("Components.damage_resistant"),
                    );
                }
            }
            "enchantable" => {
                copied.insert("enchantable".to_string(), Value::from(bukkit_int(Some(raw_value)).max(1)));
            }
            "glider" => {
                if raw_value == &Value::Bool(true) {
                    copied.insert("glider".to_string(), json!({}));
                }
            }
            "profile" => {
                if let Value::Object(raw) = raw_value {
                    if let Some(profile) = convert_profile(raw, diagnostics, source, item) {
                        copied.insert("profile".to_string(), Value::Object(profile));
                    }
                }
            }
            "custom_model_data" => {
                if let Value::Object(raw) = raw_value {
                    if let Some(custom_model_data) = convert_custom_model_data(raw, diagnostics, source, item) {
                        copied.insert("custom_model_data".to_string(), Value::Object(custom_model_data));
                    }
                }
            }
            "tooltip_display" => {
                let hidden: Vec<String> = as_string_list(Some(raw_value))
                    .iter()
                    .filter_map(|value| {
                        normalize_location(
                            value,
                            diagnostics,
                            &Details::new().source(source).item(item).field("Components.tooltip_display"),
                            &[],
                            "minecraft",
                        )
                    })
                    .collect();
                if !hidden.is_empty() {
                    copied.insert(
                        "tooltip_display".to_string(),
                        json!({ "hide_tooltip": false, "hidden_components": hidden }),
                    );
                }
            }
            "minimum_attack_charge" => {
                if let Value::Number(number) = raw_value {
                    if let Some(value) = number.as_f64().filter(|value| value.is_finite()) {
                        copied.insert("minimum_attack_charge".to_string(), Value::from(value.clamp(0.0, 1.0)));
                    }
                }
            }
            _ => {}
        }
    }
    if !copied.is_empty() {
        data.insert("components".to_string(), Value::Object(copied));
    }
    item_model
}

const VANILLA_EFFECT_IDS_1_21_11: &[&str] = &[
    "speed", "slowness", "haste", "mining_fatigue", "strength", "instant_health", "instant_damage", "jump_boost", "nausea",
    "regeneration", "resistance", "fire_resistance", "water_breathing", "invisibility", "blindness", "night_vision", "hunger",
    "weakness", "poison", "wither", "health_boost", "absorption", "saturation", "glowing", "levitation", "luck", "unluck",
    "slow_falling", "conduit_power", "dolphins_grace", "bad_omen", "hero_of_the_village", "darkness", "trial_omen", "raid_omen",
    "wind_charged", "weaving", "oozing", "infested", "breath_of_the_nautilus",
];

fn resolve_potion_type(raw: &str) -> Option<String> {
    let normalized = raw.to_lowercase();
    let separator = normalized.find(':');
    let namespace = match separator {
        Some(index) => &normalized[..index],
        None => "minecraft",
    };
    let path = match separator {
        Some(index) => &normalized[index + 1..],
        None => normalized.as_str(),
    };
    if namespace != "minecraft" {
        return None;
    }
    if VANILLA_EFFECT_IDS_1_21_11.contains(&path) {
        Some(format!("minecraft:{}", path))
    } else {
        None
    }
}

fn resolve_direct_potion_effect(
    raw: Option<&Value>,
    diagnostics: &mut DiagnosticBag,
    source: &str,
    item: &str,
    index: usize,
) -> Option<String> {
    if let Some(Value::Number(number)) = raw {
        let Some(value) = number.as_f64() else { return None };
        if value.fract() != 0.0 || value < 1.0 {
            return None;
        }
        return VANILLA_EFFECT_IDS_1_21_11
            .get(value as usize - 1)
            .map(|path| format!("minecraft:{}", path));
    }
    let Value::String(text) = raw? else { return None };
    let normalized = normalize_location(
        text,
        diagnostics,
        &Details::new().source(source).item(item).field(format!("PotionEffects[{}].effect", index)),
        &[],
        "minecraft",
    )?;
    let (namespace, path) = crate::resource_location::split_location(&normalized);
    if namespace == "minecraft" && !VANILLA_EFFECT_IDS_1_21_11.contains(&path) {
        return None;
    }
    Some(normalized)
}

fn valid_i32(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Number(number)) => number
            .as_f64()
            .map(|value| value.fract() == 0.0 && value >= -2147483648.0 && value <= 2147483647.0)
            .unwrap_or(false),
        _ => false,
    }
}

fn convert_potion_effects(
    value: Option<&Value>,
    diagnostics: &mut DiagnosticBag,
    source: &str,
    item: &str,
) -> Vec<JsonObject> {
    let Some(value) = value else { return Vec::new() };
    let Some(Value::Array(entries)) = Some(value) else {
        diagnostics.info(
            "POTION_EFFECTS_NON_LIST_IGNORED",
            "Nexo PotionEffects only accepts a YAML list; this value is ignored by Nexo 1.26",
            Details::new().source(source).item(item).field("PotionEffects"),
        );
        return Vec::new();
    };
    let mut output: Vec<JsonObject> = Vec::new();
    for (index, raw) in entries.iter().enumerate() {
        // linkedMapList filters non-map entries.
        let Some(Value::Object(raw)) = Some(raw) else { continue };
        let mut effective_raw: Option<Value> = raw.get("effect").cloned();
        if let Some(Value::String(type_value)) = raw.get("type") {
            if let Some(from_type) = resolve_potion_type(type_value) {
                effective_raw = Some(Value::String(from_type));
            } else if type_value.contains(':') {
                diagnostics.error(
                    "POTION_EFFECT_CUSTOM_TYPE_UNREPRESENTABLE",
                    "Nexo resolves a namespaced type and then discards its namespace before Bukkit deserialization; this custom effect cannot be migrated reliably",
                    Details::new()
                        .source(source)
                        .item(item)
                        .field(format!("PotionEffects[{}].type", index))
                        .lossy(),
                );
                continue;
            }
        }
        let id = resolve_direct_potion_effect(effective_raw.as_ref(), diagnostics, source, item, index);
        let duration = raw.get("duration");
        let amplifier = raw.get("amplifier");
        let Some(id) = id else {
            diagnostics.error(
                "POTION_EFFECT_TYPE_INVALID",
                "PotionEffects entry has no Bukkit-resolvable effect type",
                Details::new()
                    .source(source)
                    .item(item)
                    .field(format!("PotionEffects[{}].type", index))
                    .lossy(),
            );
            continue;
        };
        if !valid_i32(duration) || !valid_i32(amplifier) {
            diagnostics.error(
                "POTION_EFFECT_INTEGER_REQUIRED",
                "Bukkit PotionEffect requires integer duration and amplifier fields",
                Details::new()
                    .source(source)
                    .item(item)
                    .field(format!("PotionEffects[{}]", index))
                    .lossy(),
            );
            continue;
        }
        if raw.get("hidden_effect").is_some() || raw.get("hidden-potion-effect").is_some() {
            diagnostics.error(
                "POTION_HIDDEN_EFFECT_UNREPRESENTABLE",
                "Nexo's raw linked-map path cannot deserialize a nested hidden PotionEffect from this YAML form",
                Details::new()
                    .source(source)
                    .item(item)
                    .field(format!("PotionEffects[{}]", index))
                    .lossy(),
            );
            continue;
        }
        let ambient = matches!(raw.get("ambient"), Some(Value::Bool(flag)) if *flag);
        let particles = match raw.get("has-particles") {
            Some(Value::Bool(flag)) => *flag,
            _ => true,
        };
        let icon = match raw.get("has-icon") {
            Some(Value::Bool(flag)) => *flag,
            _ => particles,
        };
        output.push(
            json!({
                "id": id,
                "duration": duration.unwrap().as_f64().map(|value| value as i64).unwrap_or(0),
                "amplifier": amplifier.unwrap().as_f64().map(|value| value as i64).unwrap_or(0),
                "ambient": ambient,
                "show_particles": particles,
                "show_icon": icon,
            })
            .as_object()
            .unwrap()
            .clone(),
        );
    }
    output
}

const NAMED_COLORS: &[(&str, u32)] = &[
    ("black", 0x000000), ("dark_blue", 0x0000aa), ("dark_green", 0x00aa00), ("dark_aqua", 0x00aaaa),
    ("dark_red", 0xaa0000), ("dark_purple", 0xaa00aa), ("gold", 0xffaa00), ("gray", 0xaaaaaa),
    ("dark_gray", 0x555555), ("blue", 0x5555ff), ("green", 0x55ff55), ("aqua", 0x55ffff),
    ("red", 0xff5555), ("light_purple", 0xff55ff), ("yellow", 0xffff55), ("white", 0xffffff),
];

static HEX_COLOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[0-9a-f]{8}$").unwrap());
static INTEGER_TEXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-?\d+$").unwrap());
static INTEGER_PART: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-?\d+$").unwrap());

pub fn nexo_color(raw: Option<&Value>) -> Option<u32> {
    let text = match raw {
        Some(Value::Number(number)) => {
            if let Some(int) = number.as_i64() {
                int.to_string()
            } else {
                number.as_f64()?.to_string()
            }
        }
        Some(Value::String(text)) => text.clone(),
        _ => return None,
    };
    if text.starts_with('#') || text.starts_with("0x") {
        let stripped = text.trim_start_matches('#');
        let stripped = stripped.strip_prefix("0x").unwrap_or(stripped);
        let hex = format!("{:F>8}", stripped);
        let hex = &hex[..8.min(hex.len())];
        if !HEX_COLOR.is_match(hex) {
            return None;
        }
        return Some(u32::from_str_radix(hex, 16).ok()? & 0xffffff);
    }
    if text.contains(',') {
        let cleaned = text.replace(' ', "");
        let parts: Vec<&str> = cleaned.split(',').collect();
        if (parts.len() != 3 && parts.len() != 4) || !parts.iter().all(|part| INTEGER_PART.is_match(part)) {
            return None;
        }
        let values: Vec<i64> = parts.iter().filter_map(|part| part.parse().ok()).collect();
        let rgb: &[i64] = if parts.len() == 3 { &values } else { &values[1..] };
        if !rgb.iter().all(|part| (0..=255).contains(part)) {
            return None;
        }
        return Some(((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32);
    }
    if INTEGER_TEXT.is_match(&text) {
        let value: i64 = text.parse().ok()?;
        return if (0..=0xffffff).contains(&value) {
            Some(value as u32)
        } else {
            None
        };
    }
    NAMED_COLORS.iter().find(|(name, _)| *name == text).map(|(_, value)| *value)
}

fn section_list(value: Option<&Value>) -> Vec<JsonObject> {
    let Some(value) = value else { return Vec::new() };
    match value {
        Value::Array(entries) => entries.iter().filter_map(|entry| entry.as_object().cloned()).collect(),
        Value::Object(map) => {
            let all_objects = !map.is_empty() && map.values().all(Value::is_object);
            if all_objects && find_key(map, "attribute").is_none() && find_key(map, "key").is_none() {
                map.values().filter_map(|entry| entry.as_object().cloned()).collect()
            } else {
                vec![map.clone()]
            }
        }
        _ => Vec::new(),
    }
}

fn convert_attributes(
    value: Option<&Value>,
    diagnostics: &mut DiagnosticBag,
    source: &str,
    item: &str,
) -> Option<Vec<Value>> {
    let mut converted: Vec<Value> = Vec::new();
    for (index, modifier) in section_list(value).iter().enumerate() {
        let raw_attribute = get_string(modifier, "attribute");
        let amount = get_number(modifier, "amount");
        let (Some(raw_attribute), Some(amount)) = (raw_attribute, amount) else {
            diagnostics.warning(
                "ATTRIBUTE_MODIFIER_INVALID",
                "Nexo ignores an attribute modifier without a valid attribute and amount",
                Details::new()
                    .source(source)
                    .item(item)
                    .field(format!("AttributeModifiers[{}]", index))
                    .lossy(),
            );
            continue;
        };
        let lower = raw_attribute.to_lowercase();
        let without_generic = lower.strip_prefix("generic_").unwrap_or(&lower);
        let stripped = without_generic.strip_prefix("player_").unwrap_or(without_generic).to_string();
        let attribute_type = if stripped.contains(':') {
            stripped.clone()
        } else {
            format!("minecraft:{}", stripped)
        };
        let operation = match (get_string(modifier, "operation").unwrap_or("ADD_NUMBER")).to_lowercase().as_str() {
            "add_number" => "add_value",
            "add_scalar" => "add_multiplied_base",
            "multiply_scalar_1" => "add_multiplied_total",
            _ => "add_value",
        };
        let after_namespace = &stripped[stripped.find(':').map(|index| index + 1).unwrap_or(0)..];
        let path = strip_leading_segment(after_namespace);
        let mut output = JsonObject::new();
        output.insert("type".to_string(), Value::String(attribute_type));
        output.insert("slot".to_string(), Value::String((get_string(modifier, "slot").unwrap_or("any")).to_lowercase()));
        output.insert(
            "id".to_string(),
            Value::String(
                get_string(modifier, "key")
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("nexo:{}_{}", item, path))
                    .to_lowercase(),
            ),
        );
        output.insert("amount".to_string(), json!(amount));
        output.insert("operation".to_string(), Value::String(operation.to_string()));
        if let Some(display) = get_object(modifier, "display") {
            let raw_type = (get_string(display, "type").unwrap_or("reset")).to_lowercase();
            let display_value = if raw_type == "override" {
                json!({ "type": "override", "value": get_string(display, "text").unwrap_or("") })
            } else {
                json!({ "type": if raw_type == "hidden" { "hidden" } else { "default" } })
            };
            output.insert("display".to_string(), display_value);
        }
        converted.push(Value::Object(output));
    }
    if converted.is_empty() {
        None
    } else {
        Some(converted)
    }
}

/// Mirrors TS `path.replace(/^[^.]+\./, "")`: drop the first dot-segment prefix.
fn strip_leading_segment(path: &str) -> String {
    match path.find('.') {
        Some(index) if index > 0 && !path[..index].contains('.') => path[index + 1..].to_string(),
        _ => path.to_string(),
    }
}

fn convert_persistent_data(
    value: Option<&Value>,
    diagnostics: &mut DiagnosticBag,
    source: &str,
    item: &str,
) -> Option<JsonObject> {
    let mut output = JsonObject::new();
    for (index, entry) in section_list(value).iter().enumerate() {
        let key = get_string(entry, "key");
        let raw_value = get_value(entry, "value");
        let pdc_type = (get_string(entry, "type").unwrap_or("")).to_uppercase();
        let (Some(key), Some(raw_value)) = (key, raw_value) else {
            diagnostics.warning(
                "PERSISTENT_DATA_INVALID",
                "Nexo ignores PersistentData entries without key, type, and value",
                Details::new()
                    .source(source)
                    .item(item)
                    .field(format!("PersistentData[{}]", index))
                    .lossy(),
            );
            continue;
        };
        if pdc_type.is_empty() {
            diagnostics.warning(
                "PERSISTENT_DATA_INVALID",
                "Nexo ignores PersistentData entries without key, type, and value",
                Details::new()
                    .source(source)
                    .item(item)
                    .field(format!("PersistentData[{}]", index))
                    .lossy(),
            );
            continue;
        }
        output.insert(key.to_lowercase(), raw_value.clone());
        if pdc_type != "STRING" && pdc_type != "INTEGER" {
            diagnostics.warning(
                "PERSISTENT_DATA_TYPE_APPROXIMATED",
                &format!(
                    "CraftEngine pdc YAML cannot force Nexo's {} scalar/array tag width; verify this value manually",
                    pdc_type
                ),
                Details::new()
                    .source(source)
                    .item(item)
                    .field(format!("PersistentData[{}]", index))
                    .lossy(),
            );
        }
    }
    if output.is_empty() {
        None
    } else {
        Some(output)
    }
}

fn map_root_data(
    config: &JsonObject,
    diagnostics: &mut DiagnosticBag,
    source: &str,
    item: &str,
    material: &str,
) -> (JsonObject, Option<String>) {
    let mut data = JsonObject::new();
    if let Some(Value::String(itemname)) = config.get("itemname") {
        if !itemname.is_empty() {
            data.insert("item_name".to_string(), Value::String(itemname.clone()));
        }
    }
    if let Some(Value::String(customname)) = config.get("customname") {
        if !customname.is_empty() {
            data.insert("custom_name".to_string(), Value::String(customname.clone()));
        }
    }
    if let Some(Value::Array(entries)) = config.get("lore") {
        let lore: Vec<String> = entries
            .iter()
            .filter_map(|entry| entry.as_str().map(str::to_string))
            .collect();
        if !lore.is_empty() {
            data.insert("lore".to_string(), Value::Array(lore.into_iter().map(Value::String).collect()));
        }
    }
    let root_color = match config.get("color") {
        Some(Value::String(text)) => nexo_color(Some(&Value::String(text.clone()))),
        _ => None,
    };
    if let Some(color) = root_color {
        data.insert("dyed_color".to_string(), Value::from(color));
    }
    if config.contains_key("unbreakable") {
        data.insert(
            "unbreakable".to_string(),
            Value::Bool(config.get("unbreakable") == Some(&Value::Bool(true))),
        );
    }
    if let Some(Value::Object(entries)) = config.get("Enchantments") {
        let mut enchantments = JsonObject::new();
        for (raw_id, raw_level) in entries {
            let Some(id) = normalize_location(
                raw_id,
                diagnostics,
                &Details::new().source(source).item(item).field(format!("Enchantments.{}", raw_id)),
                &[],
                "minecraft",
            ) else {
                continue;
            };
            let level = bukkit_int(Some(raw_level));
            if !(1..=255).contains(&level) {
                diagnostics.warning(
                    "ENCHANTMENT_LEVEL_CE_LIMIT",
                    "Nexo does not clamp enchantment levels, but CraftEngine accepts only 1..255; the value was clamped",
                    Details::new()
                        .source(source)
                        .item(item)
                        .field(format!("Enchantments.{}", raw_id))
                        .lossy(),
                );
            }
            enchantments.insert(id, Value::from(level.clamp(1, 255)));
        }
        if !enchantments.is_empty() {
            data.insert("enchantments".to_string(), Value::Object(enchantments));
        }
    }
    if config.contains_key("max_durability") {
        diagnostics.info(
            "ROOT_MAX_DURABILITY_IGNORED",
            "Nexo 1.26 has no root max_durability parser; use Components.max_damage",
            Details::new().source(source).item(item).field("max_durability"),
        );
    }
    if let Some(attributes) = convert_attributes(get_value(config, "AttributeModifiers"), diagnostics, source, item) {
        data.insert("attribute_modifiers".to_string(), Value::Array(attributes));
    }
    if let Some(pdc) = convert_persistent_data(get_value(config, "PersistentData"), diagnostics, source, item) {
        data.insert("pdc".to_string(), Value::Object(pdc));
    }
    let trim_pattern = match config.get("trim_pattern") {
        Some(Value::String(text)) => normalize_location(
            text,
            diagnostics,
            &Details::new().source(source).item(item).field("trim_pattern"),
            &[],
            "minecraft",
        ),
        _ => None,
    };
    let trim_material = match config.get("trim_material") {
        Some(Value::String(text)) => normalize_location(
            text,
            diagnostics,
            &Details::new().source(source).item(item).field("trim_material"),
            &[],
            "minecraft",
        ),
        _ => None,
    };
    if let Some(pattern) = trim_pattern {
        data.insert(
            "trim".to_string(),
            json!({ "pattern": pattern, "material": trim_material.unwrap_or_else(|| "minecraft:redstone".to_string()) }),
        );
    } else if trim_material.is_some() {
        diagnostics.info(
            "TRIM_MATERIAL_WITHOUT_PATTERN_IGNORED",
            "Nexo only emits an armor trim when trim_pattern resolves",
            Details::new().source(source).item(item).field("trim_material"),
        );
    }
    let components = match config.get("Components") {
        Some(Value::Object(map)) => Some(map),
        _ => None,
    };
    let component_item_model = components.and_then(|map| map_components(map, &mut data, diagnostics, source, item, material));
    let unset_primary = components
        .map(|map| as_string_list(get_value(map, "unset_components")))
        .unwrap_or_default();
    let unset = if !unset_primary.is_empty() || components.is_none() {
        unset_primary
    } else {
        as_string_list(get_value(components.unwrap(), "unset_component"))
    };
    let normalized_unset: Vec<String> = unset
        .iter()
        .filter_map(|name| normalize_component_name(name))
        .collect();

    let effects = convert_potion_effects(get_value(config, "PotionEffects"), diagnostics, source, item);
    if !effects.is_empty() && !normalized_unset.contains(&"potion_contents".to_string()) {
        let mut component_data = match data.get("components") {
            Some(Value::Object(map)) => map.clone(),
            _ => JsonObject::new(),
        };
        let mut potion_contents = JsonObject::new();
        potion_contents.insert(
            "custom_effects".to_string(),
            Value::Array(effects.into_iter().map(Value::Object).collect()),
        );
        if let Some(color) = root_color {
            potion_contents.insert("custom_color".to_string(), Value::from(color));
        }
        component_data.insert("potion_contents".to_string(), Value::Object(potion_contents));
        data.insert("components".to_string(), Value::Object(component_data));
    }
    // Nexo applies unset_components after every generated component, including
    // root PotionEffects, so keep this processor last in the emitted data map.
    if !normalized_unset.is_empty() {
        data.insert(
            "remove_components".to_string(),
            Value::Array(normalized_unset.into_iter().map(Value::String).collect()),
        );
    }
    if get_value(config, "unset_components").is_some() {
        diagnostics.info(
            "ROOT_UNSET_COMPONENTS_IGNORED",
            "Nexo 1.26 reads unset_components only inside Components",
            Details::new().source(source).item(item).field("unset_components"),
        );
    }
    if get_value(config, "ItemFlags").is_some() {
        diagnostics.warning(
            "ITEM_FLAGS_MANUAL",
            "Legacy Bukkit ItemFlags do not map one-to-one to modern tooltip_display",
            Details::new().source(source).item(item).field("ItemFlags").lossy(),
        );
    }
    (data, component_item_model)
}

pub fn convert_item(
    item: &ResolvedItem,
    options: &ItemOptions,
    assigned_custom_model_data: Option<i64>,
    diagnostics: &mut DiagnosticBag,
) -> Option<ConvertedItem> {
    if item.template {
        return None;
    }
    let target_id = format!("{}:{}", options.namespace, item.id);
    let material_raw = item.config.get("material");
    let matched_material = match_bukkit_material(material_raw);
    let material = matched_material.clone().unwrap_or_else(|| "paper".to_string());
    if matched_material.is_none() && material_raw.is_some() {
        diagnostics.info(
            "INVALID_MATERIAL_DEFAULTED",
            &format!(
                "Nexo Material.matchMaterial cannot resolve {}; PAPER was used",
                material_raw.unwrap()
            ),
            Details::new().source(item.source.clone()).item(item.id.clone()).field("material"),
        );
    }
    let pack = get_object(&item.config, "Pack");
    let item_model_section = get_object(&item.config, "ItemModel");
    let mut model_context = ModelContext {
        source: item.source.clone(),
        item: item.id.clone(),
        diagnostics,
        model_aliases: options.model_aliases,
    };
    let pack_info = read_pack_model(pack, &item.id, &mut model_context);
    let (mut data, component_item_model) = map_root_data(
        &item.config,
        model_context.diagnostics,
        &item.source,
        &item.id,
        &material,
    );
    let effective_color = match item.config.get("color") {
        Some(value @ Value::String(_)) if nexo_color(Some(value)).is_some() => Some(value),
        _ => None,
    };
    let converted_models = convert_models(
        &pack_info,
        item_model_section,
        &material,
        effective_color,
        options.client_mode,
        &mut model_context,
    );
    let mut ce = JsonObject::new();
    ce.insert("material".to_string(), Value::String(material.clone()));
    let dyeable = VANILLA_DYEABLE_MATERIALS.contains(&material.as_str());
    if dyeable {
        ce.insert("settings".to_string(), json!({ "dyeable": true }));
    }
    if !data.is_empty() {
        ce.insert("data".to_string(), Value::Object(std::mem::take(&mut data)));
    }
    if let Some(model) = &converted_models.model {
        ce.insert("model".to_string(), model.clone());
    }
    if let Some(legacy_model) = &converted_models.legacy_model {
        ce.insert("legacy_model".to_string(), Value::Object(legacy_model.clone()));
    }
    if let Some(custom_model_data) = assigned_custom_model_data {
        ce.insert("custom_model_data".to_string(), Value::from(custom_model_data));
    }
    if let Some(metadata) = &converted_models.metadata {
        ce.insert("hand_animation_on_swap".to_string(), Value::Bool(metadata.hand_animation_on_swap));
        ce.insert("oversized_in_gui".to_string(), Value::Bool(metadata.oversized_in_gui));
        ce.insert("swap_animation_scale".to_string(), json!(metadata.swap_animation_scale));
    }
    let mut model_pointer: Option<String> = None;
    if options.client_mode != crate::ClientMode::Legacy {
        model_pointer = component_item_model;
        if model_pointer.is_none() && converted_models.generated_item_model {
            model_pointer = normalize_model_location(
                &target_id,
                model_context.diagnostics,
                &Details::new()
                    .source(item.source.clone())
                    .item(item.id.clone())
                    .field("generated item_model"),
            );
        }
        if let Some(pointer) = &model_pointer {
            ce.insert("item_model".to_string(), Value::String(pointer.clone()));
        }
    } else if component_item_model.is_some() {
        model_context.diagnostics.warning(
            "ITEM_MODEL_DROPPED_IN_LEGACY_MODE",
            "Components.item_model is unavailable to legacy clients",
            Details::new()
                .source(item.source.clone())
                .item(item.id.clone())
                .field("Components.item_model")
                .lossy(),
        );
    }
    if get_value(&item.config, "crucible").is_some()
        || get_value(&item.config, "crucible_id").is_some()
        || get_value(&item.config, "mmoitem").is_some()
    {
        model_context.diagnostics.warning(
            "EXTERNAL_ITEM_PROVIDER",
            "External item providers require a matching CraftEngine integration and were not copied automatically",
            Details::new().source(item.source.clone()).item(item.id.clone()).lossy(),
        );
    }
    let mut semantics = JsonObject::new();
    semantics.insert("material_scope".to_string(), Value::String(material));
    semantics.insert("dyeable".to_string(), Value::Bool(dyeable));
    semantics.insert(
        "item_model".to_string(),
        model_pointer.as_ref().map(|p| Value::String(p.clone())).unwrap_or(Value::Null),
    );
    semantics.insert(
        "custom_model_data".to_string(),
        assigned_custom_model_data.map(Value::from).unwrap_or(Value::Null),
    );
    for (key, value) in converted_models.model_semantics {
        semantics.insert(key, value);
    }
    Some(ConvertedItem {
        source_id: item.id.clone(),
        target_id,
        config: ce,
        model_pointer,
        base_model: converted_models.base_model,
        semantics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_item(id: &str, config: Value) -> SourceItem {
        SourceItem {
            id: id.to_string(),
            source: "items.yml".to_string(),
            config: config.as_object().unwrap().clone(),
            template: false,
        }
    }

    #[test]
    fn match_material_normalizes_like_bukkit() {
        assert_eq!(match_bukkit_material(Some(&json!("minecraft:DIAMOND_SWORD"))).as_deref(), Some("diamond_sword"));
        assert_eq!(match_bukkit_material(Some(&json!("diamond sword"))).as_deref(), Some("diamond_sword"));
        assert_eq!(match_bukkit_material(Some(&json!("nope"))), None);
        assert_eq!(match_bukkit_material(None), None);
    }

    #[test]
    fn template_inheritance_merges_and_reports_missing() {
        let items = vec![
            source_item("base", json!({ "material": "diamond", "lore": ["<item_id_capitalized>"] })),
            source_item("child", json!({ "template": "base", "material": "paper" })),
            source_item("broken", json!({ "template": "missing" })),
        ];
        let mut diags = DiagnosticBag::new();
        let resolved = resolve_item_templates(&items, &mut diags);
        assert_eq!(resolved[1].config.get("material"), Some(&json!("paper")));
        assert!(diags.items.iter().any(|d| d.code == "TEMPLATE_NOT_FOUND"));
    }

    #[test]
    fn placeholders_expand_in_names_and_lore() {
        let items = vec![
            source_item("base", json!({})),
            source_item("my_item", json!({ "template": "base", "itemname": "<item_id_capitalized>", "lore": ["<item_id>"] })),
        ];
        let mut diags = DiagnosticBag::new();
        let resolved = resolve_item_templates(&items, &mut diags);
        assert_eq!(resolved[1].config.get("itemname"), Some(&json!("My Item")));
        assert_eq!(resolved[1].config.get("lore"), Some(&json!(["my_item"])));
    }

    #[test]
    fn nexo_color_parses_hex_rgb_decimal_and_named() {
        assert_eq!(nexo_color(Some(&json!("#ff0000"))), Some(0xff0000));
        assert_eq!(nexo_color(Some(&json!("255,0,0"))), Some(0xff0000));
        assert_eq!(nexo_color(Some(&json!("16711680"))), Some(0xff0000));
        assert_eq!(nexo_color(Some(&json!("red"))), Some(0xff5555));
        assert_eq!(nexo_color(Some(&json!("nonsense"))), None);
    }

    #[test]
    fn convert_item_defaults_to_paper_and_emits_semantics() {
        let item = ResolvedItem {
            id: "thing".to_string(),
            source: "items.yml".to_string(),
            config: json!({ "material": "not_real" }).as_object().unwrap().clone(),
            template: false,
            template_ids: vec![],
        };
        let options = ItemOptions {
            namespace: "author".to_string(),
            client_mode: ClientMode::Hybrid,
            model_aliases: None,
        };
        let mut diags = DiagnosticBag::new();
        let converted = convert_item(&item, &options, None, &mut diags).unwrap();
        assert_eq!(converted.config.get("material"), Some(&json!("paper")));
        assert_eq!(converted.target_id, "author:thing");
        assert!(diags.items.iter().any(|d| d.code == "INVALID_MATERIAL_DEFAULTED"));
    }
}



