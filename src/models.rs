//! Item model conversion: Pack shortcut fields, explicit ItemModel ASTs and
//! legacy override generation.
//!
//! Port of `legacy/src/models.ts`. Nexo 1.26 builds item models either from
//! the Pack shortcut fields (base model/textures plus state variants such as
//! pulling, damaged or blocking) or from an explicit ItemModel AST; the two
//! sources are lowered into CraftEngine's modern model AST and, for legacy
//! clients, into a predicate-override tree. Every diagnostic code, message,
//! default and boundary behavior mirrors the TypeScript implementation.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::diagnostics::{Details, DiagnosticBag};
use crate::json::{
    as_string_list, get_boolean, get_number, get_object, get_string, get_value, JsonObject,
};
use crate::resource_location::{
    minecraft_key, normalize_location, normalize_model_location, normalize_texture_location,
};
use crate::ClientMode;

/// Where a model reference came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelOrigin {
    Model,
    Texture,
    Default,
}

/// One resolved model reference (base or state variant).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelReference {
    pub path: String,
    pub generation: Option<JsonObject>,
    pub blueprint: Option<String>,
    pub origin: ModelOrigin,
}

/// Everything read from an item's `Pack` section that affects models.
#[derive(Debug, Clone)]
pub struct PackModelInfo {
    pub has_pack: bool,
    pub base: ModelReference,
    pub parent: String,
    pub custom_model_data: Option<f64>,
    pub pulling: Vec<ModelReference>,
    pub damaged: Vec<ModelReference>,
    pub composite: Vec<ModelReference>,
    pub dyeable: Option<ModelReference>,
    pub throwing: Option<ModelReference>,
    pub cast: Option<ModelReference>,
    pub broken: Option<ModelReference>,
    pub blocking: Option<ModelReference>,
    pub charged: Option<ModelReference>,
    pub firework: Option<ModelReference>,
    pub hand_animation_on_swap: bool,
    pub oversized_in_gui: bool,
    pub swap_animation_scale: f64,
}

/// Item-model metadata emitted next to the generated model.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemModelMetadata {
    pub hand_animation_on_swap: bool,
    pub oversized_in_gui: bool,
    pub swap_animation_scale: f64,
}

/// Result of converting one item's model sources.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvertedModels {
    pub model: Option<Value>,
    pub legacy_model: Option<JsonObject>,
    pub base_model: Option<String>,
    pub generated_item_model: bool,
    pub metadata: Option<ItemModelMetadata>,
    pub model_semantics: JsonObject,
}

/// Per-item conversion context shared by the model functions.
pub struct ModelContext<'a> {
    pub source: String,
    pub item: String,
    pub diagnostics: &'a mut DiagnosticBag,
    pub model_aliases: Option<&'a HashMap<String, String>>,
}

fn details(context: &ModelContext, field: impl Into<String>) -> Details {
    Details::new()
        .source(context.source.clone())
        .item(context.item.clone())
        .field(field)
}

/// TS truthiness for `getString` results: the empty string is falsy.
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

fn normalize_parent(value: &str, context: &mut ModelContext) -> String {
    let details = details(context, "Pack.parent_model");
    normalize_model_location(value, context.diagnostics, &details)
        .unwrap_or_else(|| "minecraft:item/generated".to_string())
}

fn normalize_static_model(value: &str, context: &mut ModelContext, field: &str) -> Option<String> {
    let details = details(context, field);
    let location = normalize_model_location(value, context.diagnostics, &details)?;
    Some(
        context
            .model_aliases
            .and_then(|aliases| aliases.get(&location))
            .cloned()
            .unwrap_or(location),
    )
}

fn normalize_texture_value(value: &str, context: &mut ModelContext, field: &str) -> Option<String> {
    let details = details(context, field);
    normalize_texture_location(value, context.diagnostics, &details)
}

/// Maps a flat layer list onto the texture variables expected by a block
/// parent model. Explicit variables always win.
fn model_texture_map(parent_location: &str, layers: &[String], variables: &JsonObject) -> JsonObject {
    if !variables.is_empty() {
        return variables.clone();
    }
    if layers.is_empty() {
        return JsonObject::new();
    }
    let mut result = JsonObject::new();
    let first = layers.first().map(String::as_str).unwrap_or("minecraft:missingno");
    result.insert("particle".to_string(), Value::String(first.to_string()));
    // Drop the namespace, mirroring `parentLocation.slice(parentLocation.indexOf(":") + 1)`.
    let parent = match parent_location.find(':') {
        Some(separator) => &parent_location[separator + 1..],
        None => parent_location,
    };
    let layer = |index: usize| -> String {
        layers
            .get(index)
            .or_else(|| layers.first())
            .map(String::as_str)
            .unwrap_or("minecraft:missingno")
            .to_string()
    };
    if parent == "block/cube" || parent == "block/cube_directional" || parent == "block/cube_mirrored" {
        result.insert("particle".to_string(), Value::String(layer(2)));
        result.insert("down".to_string(), Value::String(layer(0)));
        result.insert("up".to_string(), Value::String(layer(1)));
        result.insert("north".to_string(), Value::String(layer(2)));
        result.insert("south".to_string(), Value::String(layer(3)));
        result.insert("west".to_string(), Value::String(layer(4)));
        result.insert("east".to_string(), Value::String(layer(5)));
    } else if parent == "block/cube_all" || parent == "block/cube_mirrored_all" {
        result.insert("all".to_string(), Value::String(layer(0)));
    } else if parent == "block/cross" {
        result.insert("cross".to_string(), Value::String(layer(0)));
    } else if parent.starts_with("block/orientable") {
        result.insert("front".to_string(), Value::String(layer(0)));
        result.insert("side".to_string(), Value::String(layer(1)));
        if !parent.ends_with("vertical") {
            result.insert("top".to_string(), Value::String(layer(2)));
        }
        if parent.ends_with("with_bottom") {
            result.insert("bottom".to_string(), Value::String(layer(3)));
        }
    } else if parent.starts_with("block/cube_column") {
        result.insert("end".to_string(), Value::String(layer(0)));
        result.insert("side".to_string(), Value::String(layer(1)));
    } else if parent == "block/cube_bottom_top" || parent.contains("block/slab") || parent.ends_with("stairs") {
        result.insert("bottom".to_string(), Value::String(layer(0)));
        result.insert("side".to_string(), Value::String(layer(1)));
        result.insert("top".to_string(), Value::String(layer(2)));
    } else if parent == "block/cube_top" {
        result.insert("top".to_string(), Value::String(layer(0)));
        result.insert("side".to_string(), Value::String(layer(1)));
    } else if parent.contains("block/door_") {
        result.insert("bottom".to_string(), Value::String(layer(0)));
        result.insert("top".to_string(), Value::String(layer(1)));
    } else if parent.contains("trapdoor") || parent.contains("chain") {
        result.insert("texture".to_string(), Value::String(layer(0)));
    } else if parent.contains("lantern") {
        result.insert("lantern".to_string(), Value::String(layer(0)));
    } else if parent.contains("template_bars") {
        result.insert("bars".to_string(), Value::String(layer(0)));
        result.insert("edge".to_string(), Value::String(layer(1)));
    } else {
        for (index, value) in layers.iter().enumerate() {
            result.insert(format!("layer{}", index), Value::String(value.clone()));
        }
    }
    result
}

/// Reads the base texture variables of a Pack, either from the `textures`
/// object or from the `texture`/`textures` layer lists.
fn read_base_textures(pack: &JsonObject, parent: &str, context: &mut ModelContext) -> Option<JsonObject> {
    if let Some(raw_variables) = get_object(pack, "textures") {
        let mut variables = JsonObject::new();
        for (name, raw) in raw_variables {
            let Some(text) = raw.as_str() else {
                context.diagnostics.error(
                    "PACK_TEXTURE_NOT_STRING",
                    &format!("Texture variable {} must be a string", name),
                    details(context, format!("Pack.textures.{}", name)),
                );
                continue;
            };
            if let Some(value) = normalize_texture_value(text, context, &format!("Pack.textures.{}", name)) {
                variables.insert(name.clone(), Value::String(value));
            }
        }
        return Some(model_texture_map(parent, &[], &variables));
    }
    let raw_texture = get_value(pack, "texture");
    let mut list = as_string_list(raw_texture);
    let textures_value = get_value(pack, "textures");
    if list.is_empty() && matches!(textures_value, Some(Value::Array(_))) {
        list.extend(as_string_list(textures_value));
    }
    let mut layers = Vec::new();
    for value in &list {
        if let Some(normalized) = normalize_texture_value(value, context, "Pack.texture") {
            layers.push(normalized);
        }
    }
    if layers.is_empty() {
        None
    } else {
        Some(model_texture_map(parent, &layers, &JsonObject::new()))
    }
}

fn generation(parent: &str, textures: Option<&JsonObject>) -> Option<JsonObject> {
    let textures = textures?;
    if textures.is_empty() {
        return None;
    }
    let mut map = JsonObject::new();
    map.insert("parent".to_string(), Value::String(parent.to_string()));
    map.insert("textures".to_string(), Value::Object(textures.clone()));
    Some(map)
}

fn read_single_variant(
    pack: &JsonObject,
    model_key: &str,
    texture_key: &str,
    base: &ModelReference,
    parent: &str,
    context: &mut ModelContext,
) -> Option<ModelReference> {
    if let Some(model) = non_empty(get_string(pack, model_key)) {
        let path = normalize_static_model(model, context, &format!("Pack.{}", model_key))?;
        return Some(ModelReference {
            path,
            generation: None,
            blueprint: None,
            origin: ModelOrigin::Model,
        });
    }
    let texture = non_empty(get_string(pack, texture_key))?;
    let path = normalize_texture_value(texture, context, &format!("Pack.{}", texture_key))?;
    let parent_choice = if base.origin == ModelOrigin::Default {
        parent.to_string()
    } else {
        base.path.clone()
    };
    let special_textures = model_texture_map(&parent_choice, std::slice::from_ref(&path), &JsonObject::new());
    let generation = generation(&parent_choice, Some(&special_textures));
    Some(ModelReference {
        path,
        generation,
        blueprint: None,
        origin: ModelOrigin::Texture,
    })
}

fn read_list_variant(
    pack: &JsonObject,
    model_key: &str,
    texture_key: &str,
    base: &ModelReference,
    parent: &str,
    context: &mut ModelContext,
) -> Vec<ModelReference> {
    let models = as_string_list(get_value(pack, model_key));
    if !models.is_empty() {
        return models
            .iter()
            .filter_map(|value| normalize_static_model(value, context, &format!("Pack.{}", model_key)))
            .map(|path| ModelReference {
                path,
                generation: None,
                blueprint: None,
                origin: ModelOrigin::Model,
            })
            .collect();
    }
    let textures = as_string_list(get_value(pack, texture_key));
    let mut result = Vec::new();
    for value in &textures {
        let Some(path) = normalize_texture_value(value, context, &format!("Pack.{}", texture_key)) else {
            continue;
        };
        let parent_choice = if base.origin == ModelOrigin::Default {
            parent.to_string()
        } else {
            base.path.clone()
        };
        let special_textures = model_texture_map(&parent_choice, std::slice::from_ref(&path), &JsonObject::new());
        let generation = generation(&parent_choice, Some(&special_textures));
        result.push(ModelReference {
            path,
            generation,
            blueprint: None,
            origin: ModelOrigin::Texture,
        });
    }
    result
}

pub fn read_pack_model(pack: Option<&JsonObject>, item_id: &str, context: &mut ModelContext) -> PackModelInfo {
    let Some(pack) = pack else {
        return PackModelInfo {
            has_pack: false,
            base: ModelReference {
                path: format!("minecraft:{}", item_id),
                generation: None,
                blueprint: None,
                origin: ModelOrigin::Default,
            },
            parent: "minecraft:item/generated".to_string(),
            custom_model_data: None,
            pulling: Vec::new(),
            damaged: Vec::new(),
            composite: Vec::new(),
            dyeable: None,
            throwing: None,
            cast: None,
            broken: None,
            blocking: None,
            charged: None,
            firework: None,
            hand_animation_on_swap: true,
            oversized_in_gui: false,
            swap_animation_scale: 1.0,
        };
    };
    let parent_raw = get_string(pack, "parent_model")
        .or_else(|| get_string(pack, "parent"))
        .unwrap_or("minecraft:item/generated");
    let parent = normalize_parent(parent_raw, context);
    let bbmodel = non_empty(get_string(pack, "bbmodel")).map(|value| value.to_string());
    let explicit_model = non_empty(get_string(pack, "model")).map(|value| value.to_string());
    let textures = read_base_textures(pack, &parent, context);
    let base: ModelReference = if let Some(bbmodel) = &bbmodel {
        let details = details(context, "Pack.bbmodel");
        let path = normalize_location(bbmodel, context.diagnostics, &details, &[".bbmodel"], "minecraft")
            .unwrap_or_else(|| format!("minecraft:{}", item_id));
        let blueprint = match path.find(':') {
            Some(separator) => format!("{}/{}", &path[..separator], &path[separator + 1..]),
            // Mirrors JS `slice(0, -1) + "/" + slice(0)`; unreachable for
            // normalized locations, which always contain a colon.
            None => format!("{}/{}", &path[..path.len().saturating_sub(1)], path),
        };
        context.diagnostics.warning(
            "BBMODEL_CONVERTER_REVIEW",
            "The .bbmodel is delegated to CraftEngine's Blockbench converter; verify rotations, animation metadata, and extracted texture paths",
            details.clone().lossy(),
        );
        ModelReference {
            path,
            generation: None,
            blueprint: Some(blueprint),
            origin: ModelOrigin::Model,
        }
    } else if let Some(explicit_model) = &explicit_model {
        let path = normalize_static_model(explicit_model, context, "Pack.model")
            .unwrap_or_else(|| format!("minecraft:{}", item_id));
        ModelReference {
            path,
            generation: None,
            blueprint: None,
            origin: ModelOrigin::Model,
        }
    } else {
        let details = details(context, "Pack.model(default)");
        let path = normalize_model_location(item_id, context.diagnostics, &details)
            .unwrap_or_else(|| format!("minecraft:{}", item_id));
        let origin = if textures.is_some() {
            ModelOrigin::Texture
        } else {
            ModelOrigin::Default
        };
        let generation = generation(&parent, textures.as_ref());
        ModelReference {
            path,
            generation,
            blueprint: None,
            origin,
        }
    };
    // Nexo 1.26 does not read Pack.generate_model when choosing between an
    // explicit model and generated textures. Matching that parser behavior is
    // exact and does not require a per-item diagnostic.
    let custom_model_data = get_number(pack, "custom_model_data");
    if let Some(value) = custom_model_data {
        if value.fract() != 0.0 || value <= 0.0 || value > 16_777_216.0 {
            context.diagnostics.error(
                "INVALID_CUSTOM_MODEL_DATA",
                "custom_model_data must be an integer in 1..16777216",
                details(context, "Pack.custom_model_data"),
            );
        }
    }
    let mut info = PackModelInfo {
        has_pack: true,
        base,
        parent,
        // The value is kept even when out of range; only the error above is
        // emitted, matching the TypeScript field assignment.
        custom_model_data: custom_model_data.filter(|value| value.fract() == 0.0 && *value > 0.0),
        pulling: Vec::new(),
        damaged: Vec::new(),
        composite: Vec::new(),
        dyeable: None,
        throwing: None,
        cast: None,
        broken: None,
        blocking: None,
        charged: None,
        firework: None,
        hand_animation_on_swap: get_boolean(pack, "hand_swap_animation", true),
        oversized_in_gui: get_boolean(pack, "oversized_in_gui", false),
        swap_animation_scale: get_number(pack, "swap_animation_scale").unwrap_or(1.0),
    };
    info.blocking = read_single_variant(pack, "blocking_model", "blocking_texture", &info.base, &info.parent, context);
    info.charged = read_single_variant(pack, "charged_model", "charged_texture", &info.base, &info.parent, context);
    info.cast = read_single_variant(pack, "cast_model", "cast_texture", &info.base, &info.parent, context);
    info.broken = read_single_variant(pack, "broken_model", "broken_texture", &info.base, &info.parent, context);
    info.firework = read_single_variant(pack, "firework_model", "firework_texture", &info.base, &info.parent, context);
    info.dyeable = read_single_variant(pack, "dyeable_model", "dyeable_texture", &info.base, &info.parent, context);
    info.throwing = read_single_variant(pack, "throwing_model", "throwing_texture", &info.base, &info.parent, context);
    info.pulling = read_list_variant(pack, "pulling_models", "pulling_textures", &info.base, &info.parent, context);
    info.damaged = read_list_variant(pack, "damaged_models", "damaged_textures", &info.base, &info.parent, context);
    info.composite = read_list_variant(pack, "composite_models", "composite_textures", &info.base, &info.parent, context);
    info
}

fn model_node(reference: &ModelReference, tints: Option<&[Value]>) -> JsonObject {
    let mut node = JsonObject::new();
    node.insert("type".to_string(), Value::String("model".to_string()));
    node.insert("path".to_string(), Value::String(reference.path.clone()));
    if let Some(tints) = tints {
        if !tints.is_empty() {
            node.insert("tints".to_string(), Value::Array(tints.to_vec()));
        }
    }
    if let Some(generation) = &reference.generation {
        node.insert("generation".to_string(), Value::Object(generation.clone()));
    }
    if let Some(blueprint) = &reference.blueprint {
        node.insert("blueprint".to_string(), Value::String(blueprint.clone()));
    }
    node
}

/// JS `Math.round`: halves round toward +infinity (`floor(x + 0.5)`).
fn js_math_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// ECMA-262 `Number.prototype.toFixed` scaled integer: the integer `n`
/// minimizing `|n / 10^digits - value|` against the exact binary value of
/// `value`, with exact ties going to the larger `n` (the spec's "larger n"
/// rule). Negative zero cannot be represented here; it is unreachable in the
/// predicate domain and would only differ by the sign of zero.
fn to_fixed_scaled(value: f64, digits: u32) -> i128 {
    let bits = value.to_bits();
    let negative = bits >> 63 != 0;
    let biased = ((bits >> 52) & 0x7ff) as i32;
    let fraction = (bits & ((1u64 << 52) - 1)) as i128;
    let (mantissa, exponent) = if biased == 0 {
        (fraction, -1074i32)
    } else {
        (fraction | (1i128 << 52), biased - 1075)
    };
    let mantissa = if negative { -mantissa } else { mantissa };
    if mantissa == 0 {
        return 0;
    }
    let scale = 10i128.pow(digits);
    // value * scale == numerator * 2^exponent exactly (|numerator| < 2^120).
    let numerator = mantissa * scale;
    if exponent >= 0 {
        if exponent <= 60 {
            return numerator << exponent;
        }
        // Far beyond the predicate domain; JS toFixed would fall back to
        // ToString for values >= 10^21 anyway.
        return (value * scale as f64) as i128;
    }
    let shift = (-exponent) as u32;
    if shift >= 127 {
        // |value * scale| < 0.5, so 0 is the unique nearest integer.
        return 0;
    }
    let denominator: i128 = 1 << shift;
    let quotient = numerator.div_euclid(denominator);
    let remainder = numerator.rem_euclid(denominator);
    if 2 * remainder >= denominator {
        quotient + 1
    } else {
        quotient
    }
}

/// Mirrors `roundPredicate`: `Math.min(Number((Math.round(value / 0.05) *
/// 0.05).toFixed(2)), maximum)`. The toFixed round-trip is reproduced exactly
/// (e.g. `13 * 0.05 == 0.65000000000000002` still rounds to `0.65`).
fn round_predicate(value: f64, maximum: f64) -> f64 {
    let stepped = js_math_round(value / 0.05) * 0.05;
    let rounded = (to_fixed_scaled(stepped, 2) as f64) / 100.0;
    if rounded < maximum {
        rounded
    } else {
        maximum
    }
}

/// Mirrors `colorInteger`: integers are masked to 24 bits (JS `& 0xffffff`
/// wraps through ToInt32, which `rem_euclid(2^24)` reproduces exactly),
/// `#rrggbb` hex and `r,g,b` component strings are parsed, everything else
/// falls back to white.
fn color_integer(raw: Option<&Value>) -> i64 {
    match raw {
        Some(Value::Number(number)) => match number.as_f64() {
            Some(value) if value.is_finite() && value.fract() == 0.0 => {
                value.rem_euclid(16_777_216.0) as i64
            }
            _ => 16_777_215,
        },
        Some(Value::String(text)) => {
            let text = text.trim();
            if text.len() == 7
                && text.starts_with('#')
                && text[1..].chars().all(|c| c.is_ascii_hexdigit())
            {
                return i64::from_str_radix(&text[1..], 16).unwrap_or(16_777_215);
            }
            // JS keeps unparseable entries as NaN inside the split list, so a
            // single bad component invalidates the whole triple via `every`.
            let parts: Vec<Option<f64>> = text.split(',').map(|entry| js_number(entry.trim())).collect();
            if parts.len() == 3
                && parts.iter().all(|entry| {
                    matches!(entry, Some(value) if value.is_finite() && value.fract() == 0.0 && *value >= 0.0 && *value <= 255.0)
                })
            {
                let red = parts[0].unwrap() as i64;
                let green = parts[1].unwrap() as i64;
                let blue = parts[2].unwrap() as i64;
                return (red << 16) | (green << 8) | blue;
            }
            16_777_215
        }
        _ => 16_777_215,
    }
}

/// JS `Number()` coercion for color components: empty/whitespace strings are
/// `0`, hex literals and infinities parse, anything else is `NaN`.
fn js_number(text: &str) -> Option<f64> {
    if text.is_empty() {
        return Some(0.0);
    }
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return i64::from_str_radix(hex, 16).ok().map(|value| value as f64);
    }
    if let Some(hex) = text.strip_prefix("-0x").or_else(|| text.strip_prefix("-0X")) {
        return i64::from_str_radix(hex, 16).ok().map(|value| -(value as f64));
    }
    match text {
        "Infinity" | "+Infinity" => return Some(f64::INFINITY),
        "-Infinity" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    text.parse::<f64>().ok()
}

/// Vanilla 1.21.11 reference tints, entry for entry from the TypeScript table.
fn vanilla_reference_tints_1_21_11(material: &str) -> Vec<Value> {
    match material {
        "fern" | "grass_block" | "large_fern" | "short_grass" | "tall_grass" => {
            vec![json!({ "type": "grass", "downfall": 1, "temperature": 0.5 })]
        }
        "filled_map" => vec![
            json!({ "type": "constant", "value": -1 }),
            json!({ "type": "map_color", "default": 4603950 }),
        ],
        "firework_star" => vec![
            json!({ "type": "constant", "value": -1 }),
            json!({ "type": "firework", "default": -7697782 }),
        ],
        "leather_horse_armor" => vec![json!({ "type": "dye", "default": -6265536 })],
        "lingering_potion" | "potion" | "splash_potion" | "tipped_arrow" => {
            vec![json!({ "type": "potion", "default": -13083194 })]
        }
        _ => Vec::new(),
    }
}

fn into_object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        _ => unreachable!("json! object literals always produce objects"),
    }
}

fn build_shortcut_modern(info: &PackModelInfo, material: &str, color: Option<&Value>) -> JsonObject {
    // Nexo inherits tints only when the vanilla item's top-level 1.21.11 model is a simple reference.
    let tint_sources: Vec<Value> = match color {
        None => vanilla_reference_tints_1_21_11(material),
        Some(color) => vec![json!({ "type": "dye", "default": color_integer(Some(color)) })],
    };
    let reference_tints: Vec<Value> = if !tint_sources.is_empty() {
        tint_sources.clone()
    } else {
        vec![json!({ "type": "dye", "default": -1 })]
    };
    let base = model_node(&info.base, Some(&tint_sources));
    let reference_base = model_node(&info.base, Some(&reference_tints));
    let mut primary: JsonObject;
    if !info.pulling.is_empty() {
        let entries: Vec<Value> = info
            .pulling
            .iter()
            .enumerate()
            .map(|(index, reference)| {
                let threshold = if index == 0 {
                    json!(0)
                } else {
                    json!(round_predicate(
                        (index + 1) as f64 / info.pulling.len() as f64,
                        0.9
                    ))
                };
                json!({
                    "threshold": threshold,
                    "model": model_node(reference, Some(&reference_tints)),
                })
            })
            .collect();
        let pulling = json!({
            "type": "condition",
            "property": "using_item",
            "on_true": {
                "type": "range_dispatch",
                "property": if material == "crossbow" { "crossbow/pull" } else { "use_duration" },
                "scale": if material == "crossbow" { json!(1) } else { json!(0.05) },
                "entries": entries,
            },
            "on_false": reference_base,
        });
        if material == "crossbow" && (info.charged.is_some() || info.firework.is_some()) {
            let mut cases: Vec<Value> = Vec::new();
            if let Some(charged) = &info.charged {
                cases.push(json!({ "when": "arrow", "model": model_node(charged, Some(&reference_tints)) }));
            }
            if let Some(firework) = &info.firework {
                cases.push(json!({ "when": "rocket", "model": model_node(firework, Some(&reference_tints)) }));
            }
            primary = into_object(json!({
                "type": "select",
                "property": "charge_type",
                "cases": cases,
                "fallback": pulling,
            }));
        } else {
            primary = into_object(pulling);
        }
    } else if let Some(dyeable) = &info.dyeable {
        primary = into_object(json!({
            "type": "condition",
            "property": "has_component",
            "component": "minecraft:dyed_color",
            "on_true": model_node(dyeable, Some(&[json!({ "type": "dye", "default": color_integer(color) })])),
            "on_false": model_node(&info.base, None),
        }));
    } else {
        let selected = info
            .cast
            .as_ref()
            .or(info.broken.as_ref())
            .or(info.throwing.as_ref())
            .or(info.blocking.as_ref());
        if let Some(selected) = selected {
            let mut property = "using_item";
            if info.cast.is_some() {
                property = "fishing_rod/cast";
            } else if info.broken.is_some() {
                property = "broken";
            }
            primary = into_object(json!({
                "type": "condition",
                "property": property,
                "on_true": model_node(selected, Some(&reference_tints)),
                "on_false": reference_base,
            }));
        } else if material == "player_head" {
            let mut node = JsonObject::new();
            node.insert("type".to_string(), Value::String("special".to_string()));
            node.insert("base".to_string(), Value::String(info.base.path.clone()));
            node.insert("model".to_string(), json!({ "type": "player_head" }));
            if let Some(generation) = &info.base.generation {
                node.insert("generation".to_string(), Value::Object(generation.clone()));
            }
            if let Some(blueprint) = &info.base.blueprint {
                node.insert("blueprint".to_string(), Value::String(blueprint.clone()));
            }
            primary = node;
        } else {
            primary = base;
        }
    }
    if !info.composite.is_empty() {
        let mut models = vec![Value::Object(primary)];
        models.extend(info.composite.iter().map(|entry| Value::Object(model_node(entry, None))));
        primary = into_object(json!({ "type": "composite", "models": models }));
    }
    primary
}

/// Normalizes an explicit ItemModel AST: `-` becomes `_` in keys,
/// `type`/`property` become bare minecraft keys, `component` gets a
/// namespace, and `model`/`path` strings inside `model` nodes are
/// normalized into `path`.
fn normalize_ast_value(value: &Value, context: &mut ModelContext, parent_key: &str, node_type: &str) -> Value {
    match value {
        Value::Array(entries) => Value::Array(
            entries
                .iter()
                .map(|entry| normalize_ast_value(entry, context, parent_key, node_type))
                .collect(),
        ),
        Value::Object(map) => {
            let raw_type = match map.get("type") {
                Some(Value::String(text)) => minecraft_key(text).to_string(),
                _ => node_type.to_string(),
            };
            let mut output = JsonObject::new();
            for (raw_key, raw_value) in map {
                let key = raw_key.replace('-', "_");
                if key == "type" && raw_value.is_string() {
                    output.insert(
                        "type".to_string(),
                        Value::String(minecraft_key(raw_value.as_str().unwrap()).to_string()),
                    );
                } else if key == "property" && raw_value.is_string() {
                    output.insert(
                        "property".to_string(),
                        Value::String(minecraft_key(raw_value.as_str().unwrap()).to_string()),
                    );
                } else if key == "component" && raw_value.is_string() {
                    let details = details(context, "ItemModel.component");
                    let normalized = normalize_location(
                        raw_value.as_str().unwrap(),
                        context.diagnostics,
                        &details,
                        &[],
                        "minecraft",
                    )
                    .unwrap_or_else(|| raw_value.as_str().unwrap().to_string());
                    output.insert("component".to_string(), Value::String(normalized));
                } else if (key == "model" || key == "path") && raw_value.is_string() && raw_type == "model" {
                    let details = details(context, "ItemModel.model");
                    let normalized =
                        normalize_model_location(raw_value.as_str().unwrap(), context.diagnostics, &details)
                            .unwrap_or_else(|| raw_value.as_str().unwrap().to_string());
                    output.insert("path".to_string(), Value::String(normalized));
                } else {
                    let normalized = normalize_ast_value(raw_value, context, &key, &raw_type);
                    output.insert(key, normalized);
                }
            }
            Value::Object(output)
        }
        Value::String(text) if parent_key == "model" && node_type == "model" => {
            let details = details(context, "ItemModel.model");
            match normalize_model_location(text, context.diagnostics, &details) {
                Some(normalized) => Value::String(normalized),
                None => value.clone(),
            }
        }
        _ => value.clone(),
    }
}

fn explicit_item_model_metadata(raw: &JsonObject) -> ItemModelMetadata {
    ItemModelMetadata {
        hand_animation_on_swap: get_boolean(raw, "hand_animation_on_swap", true),
        oversized_in_gui: get_boolean(raw, "oversized_in_gui", false),
        swap_animation_scale: get_number(raw, "swap_animation_scale").unwrap_or(1.0),
    }
}

pub fn convert_explicit_item_model(raw: Option<&JsonObject>, context: &mut ModelContext) -> Option<JsonObject> {
    let raw = raw?;
    let metadata_keys = ["hand_animation_on_swap", "oversized_in_gui", "swap_animation_scale"];
    let body: JsonObject = raw
        .iter()
        .filter(|(key, _)| !metadata_keys.contains(&key.to_lowercase().as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let normalized = normalize_ast_value(&Value::Object(body), context, "", "");
    match normalized {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn legacy_reference(reference: &ModelReference, predicate: Option<JsonObject>) -> JsonObject {
    let mut result = JsonObject::new();
    result.insert("path".to_string(), Value::String(reference.path.clone()));
    if let Some(predicate) = predicate {
        result.insert("predicate".to_string(), Value::Object(predicate));
    }
    if let Some(generation) = &reference.generation {
        result.insert("generation".to_string(), Value::Object(generation.clone()));
    }
    if let Some(blueprint) = &reference.blueprint {
        result.insert("blueprint".to_string(), Value::String(blueprint.clone()));
    }
    result
}

fn legacy_override(reference: &ModelReference, predicate: JsonObject) -> Value {
    Value::Object(legacy_reference(reference, Some(predicate)))
}

pub fn build_legacy_model(info: &PackModelInfo) -> JsonObject {
    let mut result = legacy_reference(&info.base, None);
    let mut overrides: Vec<Value> = Vec::new();
    if let Some(reference) = &info.blocking {
        overrides.push(legacy_override(reference, into_object(json!({ "blocking": 1 }))));
    }
    if let Some(reference) = &info.charged {
        overrides.push(legacy_override(reference, into_object(json!({ "charged": 1 }))));
    }
    if let Some(reference) = &info.cast {
        overrides.push(legacy_override(reference, into_object(json!({ "cast": 1 }))));
    }
    if let Some(reference) = &info.broken {
        overrides.push(legacy_override(reference, into_object(json!({ "broken": 1 }))));
    }
    if let Some(reference) = &info.firework {
        overrides.push(legacy_override(reference, into_object(json!({ "firework": 1 }))));
    }
    for (index, reference) in info.pulling.iter().enumerate() {
        let pull = if index == 0 {
            json!(0)
        } else {
            json!(round_predicate(
                (index + 1) as f64 / info.pulling.len() as f64,
                0.9
            ))
        };
        overrides.push(legacy_override(
            reference,
            into_object(json!({ "pulling": 1, "pull": pull })),
        ));
    }
    for (offset, reference) in info.damaged.iter().skip(1).enumerate() {
        let index = offset + 1;
        overrides.push(legacy_override(
            reference,
            into_object(json!({
                "pulling": 1,
                "damage": round_predicate(index as f64 / info.damaged.len() as f64, 0.99),
            })),
        ));
    }
    if !overrides.is_empty() {
        result.insert("overrides".to_string(), Value::Array(overrides));
    }
    result
}

pub fn convert_models(
    info: &PackModelInfo,
    explicit_item_model: Option<&JsonObject>,
    material: &str,
    color: Option<&Value>,
    client_mode: ClientMode,
    context: &mut ModelContext,
) -> ConvertedModels {
    if !info.has_pack && explicit_item_model.is_none() {
        return ConvertedModels {
            model: None,
            legacy_model: None,
            base_model: None,
            generated_item_model: false,
            metadata: None,
            model_semantics: JsonObject::new(),
        };
    }
    let has_shortcut = !info.pulling.is_empty()
        || !info.composite.is_empty()
        || info.dyeable.is_some()
        || info.cast.is_some()
        || info.broken.is_some()
        || info.throwing.is_some()
        || info.blocking.is_some()
        || info.charged.is_some()
        || info.firework.is_some();
    let modern: Option<JsonObject> = if explicit_item_model.is_some() && !has_shortcut {
        convert_explicit_item_model(explicit_item_model, context)
    } else {
        Some(build_shortcut_modern(info, material, color))
    };
    let model: Option<Value> = if client_mode == ClientMode::Legacy {
        Some(Value::Object(model_node(&info.base, None)))
    } else {
        modern.map(Value::Object)
    };
    let metadata = if explicit_item_model.is_some() && !has_shortcut && client_mode != ClientMode::Legacy {
        explicit_item_model_metadata(explicit_item_model.unwrap())
    } else {
        ItemModelMetadata {
            hand_animation_on_swap: info.hand_animation_on_swap,
            oversized_in_gui: info.oversized_in_gui,
            swap_animation_scale: info.swap_animation_scale,
        }
    };
    let pulling_thresholds: Vec<Value> = info
        .pulling
        .iter()
        .enumerate()
        .map(|(index, _)| {
            if index == 0 {
                json!(0)
            } else {
                json!(round_predicate(
                    (index + 1) as f64 / info.pulling.len() as f64,
                    0.9
                ))
            }
        })
        .collect();
    let mut model_semantics = JsonObject::new();
    model_semantics.insert("base_model".to_string(), Value::String(info.base.path.clone()));
    model_semantics.insert(
        "modern_source".to_string(),
        Value::String(
            if explicit_item_model.is_some() {
                "ItemModel_or_Pack_shortcut_priority"
            } else {
                "Pack"
            }
            .to_string(),
        ),
    );
    model_semantics.insert("pulling_thresholds".to_string(), Value::Array(pulling_thresholds));
    model_semantics.insert("damaged_legacy_only".to_string(), Value::Bool(!info.damaged.is_empty()));
    let generated_item_model = model.is_some();
    let mut converted = ConvertedModels {
        model,
        legacy_model: None,
        base_model: Some(info.base.path.clone()),
        generated_item_model,
        metadata: if generated_item_model { Some(metadata) } else { None },
        model_semantics,
    };
    if client_mode != ClientMode::Modern {
        converted.legacy_model = Some(build_legacy_model(info));
    }
    if !info.damaged.is_empty() {
        context.diagnostics.warning(
            "NEXO_DAMAGED_MODEL_LEGACY_QUIRK",
            "Nexo 1.26 only consumes damaged_models in legacy overrides and also adds pulling=1; the converter preserves that actual behavior",
            details(context, "Pack.damaged_models"),
        );
    }
    if explicit_item_model.is_some() && has_shortcut {
        context.diagnostics.info(
            "PACK_SHORTCUT_PRECEDENCE",
            "Nexo Pack shortcut fields take precedence over the explicit ItemModel in Nexo 1.26",
            details(context, "ItemModel"),
        );
    }
    converted
}


#[cfg(test)]
mod tests {
    use super::*;

    fn jo(value: Value) -> JsonObject {
        match value {
            Value::Object(map) => map,
            _ => panic!("expected object"),
        }
    }

    fn context_with<'a>(diagnostics: &'a mut DiagnosticBag) -> ModelContext<'a> {
        ModelContext {
            source: "items.yml".to_string(),
            item: "my_item".to_string(),
            diagnostics,
            model_aliases: None,
        }
    }

    fn codes(bag: &DiagnosticBag) -> Vec<&str> {
        bag.items.iter().map(|item| item.code.as_str()).collect()
    }

    fn model_reference(path: &str) -> ModelReference {
        ModelReference {
            path: path.to_string(),
            generation: None,
            blueprint: None,
            origin: ModelOrigin::Model,
        }
    }

    fn pack_info(base: ModelReference) -> PackModelInfo {
        PackModelInfo {
            has_pack: true,
            base,
            parent: "minecraft:item/generated".to_string(),
            custom_model_data: None,
            pulling: Vec::new(),
            damaged: Vec::new(),
            composite: Vec::new(),
            dyeable: None,
            throwing: None,
            cast: None,
            broken: None,
            blocking: None,
            charged: None,
            firework: None,
            hand_animation_on_swap: true,
            oversized_in_gui: false,
            swap_animation_scale: 1.0,
        }
    }

    fn default_info() -> PackModelInfo {
        let mut info = pack_info(ModelReference {
            path: "minecraft:my_item".to_string(),
            generation: None,
            blueprint: None,
            origin: ModelOrigin::Default,
        });
        info.has_pack = false;
        info
    }

    #[test]
    fn read_pack_model_defaults_when_pack_missing() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let info = read_pack_model(None, "ruby", &mut context);
        assert!(!info.has_pack);
        assert_eq!(info.base.path, "minecraft:ruby");
        assert_eq!(info.base.origin, ModelOrigin::Default);
        assert!(info.base.generation.is_none());
        assert_eq!(info.parent, "minecraft:item/generated");
        assert!(info.custom_model_data.is_none());
        assert!(info.hand_animation_on_swap);
        assert!(!info.oversized_in_gui);
        assert_eq!(info.swap_animation_scale, 1.0);
        assert!(diags.items.is_empty());
    }

    #[test]
    fn read_pack_model_texture_layers_generate_model() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let pack = jo(json!({ "texture": ["custom:a", "custom:b"] }));
        let info = read_pack_model(Some(&pack), "my_item", &mut context);
        assert_eq!(info.base.origin, ModelOrigin::Texture);
        assert_eq!(info.base.path, "minecraft:my_item");
        assert_eq!(
            info.base.generation,
            Some(jo(json!({
                "parent": "minecraft:item/generated",
                "textures": { "particle": "custom:a", "layer0": "custom:a", "layer1": "custom:b" },
            })))
        );
        assert!(diags.items.is_empty());
    }

    #[test]
    fn read_pack_model_texture_variables_win() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let pack = jo(json!({ "textures": { "layer0": "custom:a" } }));
        let info = read_pack_model(Some(&pack), "my_item", &mut context);
        assert_eq!(info.base.origin, ModelOrigin::Texture);
        assert_eq!(
            info.base.generation,
            Some(jo(json!({
                "parent": "minecraft:item/generated",
                "textures": { "layer0": "custom:a" },
            })))
        );
    }

    #[test]
    fn read_pack_model_texture_variable_must_be_string() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let pack = jo(json!({ "textures": { "layer0": 5 } }));
        let info = read_pack_model(Some(&pack), "my_item", &mut context);
        assert_eq!(codes(&diags), vec!["PACK_TEXTURE_NOT_STRING"]);
        assert_eq!(diags.items[0].field.as_deref(), Some("Pack.textures.layer0"));
        assert_eq!(
            diags.items[0].message,
            "Texture variable layer0 must be a string"
        );
        // The textures object existed, so the base still counts as texture
        // originated, but the empty variable map yields no generation.
        assert_eq!(info.base.origin, ModelOrigin::Texture);
        assert!(info.base.generation.is_none());
    }

    #[test]
    fn read_pack_model_parent_texture_map() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let pack = jo(json!({ "parent_model": "minecraft:block/cube_all", "texture": ["custom:a"] }));
        let info = read_pack_model(Some(&pack), "my_item", &mut context);
        assert_eq!(info.parent, "minecraft:block/cube_all");
        assert_eq!(
            info.base.generation,
            Some(jo(json!({
                "parent": "minecraft:block/cube_all",
                "textures": { "particle": "custom:a", "all": "custom:a" },
            })))
        );
    }

    #[test]
    fn read_pack_model_invalid_parent_falls_back() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let pack = jo(json!({ "parent_model": "Bad Parent" }));
        let info = read_pack_model(Some(&pack), "my_item", &mut context);
        assert_eq!(info.parent, "minecraft:item/generated");
        assert_eq!(codes(&diags), vec!["INVALID_RESOURCE_LOCATION"]);
        assert_eq!(diags.items[0].field.as_deref(), Some("Pack.parent_model"));
    }

    #[test]
    fn read_pack_model_bbmodel_blueprint_and_warning() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let pack = jo(json!({ "bbmodel": "custom:models/fancy.bbmodel" }));
        let info = read_pack_model(Some(&pack), "my_item", &mut context);
        assert_eq!(info.base.path, "custom:models/fancy");
        assert_eq!(info.base.blueprint.as_deref(), Some("custom/models/fancy"));
        assert_eq!(info.base.origin, ModelOrigin::Model);
        assert_eq!(codes(&diags), vec!["BBMODEL_CONVERTER_REVIEW"]);
        assert!(diags.items[0].lossy);
    }

    #[test]
    fn read_pack_model_empty_model_string_falls_through_to_textures() {
        // TS truthiness: an empty Pack.model is falsy, so the generated
        // texture path is taken instead of the explicit model path.
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let pack = jo(json!({ "model": "", "texture": "custom:t" }));
        let info = read_pack_model(Some(&pack), "my_item", &mut context);
        assert_eq!(info.base.path, "minecraft:my_item");
        assert_eq!(info.base.origin, ModelOrigin::Texture);
        assert!(!codes(&diags).contains(&"INVALID_RESOURCE_LOCATION"));
    }

    #[test]
    fn read_pack_model_explicit_model_with_alias() {
        let mut diags = DiagnosticBag::new();
        let aliases: HashMap<String, String> =
            [("minecraft:custom".to_string(), "other:aliased".to_string())]
                .into_iter()
                .collect();
        let mut context = ModelContext {
            source: "items.yml".to_string(),
            item: "my_item".to_string(),
            diagnostics: &mut diags,
            model_aliases: Some(&aliases),
        };
        let pack = jo(json!({ "model": "custom" }));
        let info = read_pack_model(Some(&pack), "my_item", &mut context);
        assert_eq!(info.base.path, "other:aliased");
        assert_eq!(info.base.origin, ModelOrigin::Model);
    }

    #[test]
    fn custom_model_data_validation_and_retention() {
        for (value, expect_error, expected) in [
            (json!(0), true, None),
            (json!(1.5), true, None),
            (json!(16_777_217), true, Some(16_777_217.0)),
            (json!(16_777_216), false, Some(16_777_216.0)),
            (json!(1), false, Some(1.0)),
        ] {
            let mut diags = DiagnosticBag::new();
            let mut context = context_with(&mut diags);
            let pack = jo(json!({ "custom_model_data": value }));
            let info = read_pack_model(Some(&pack), "my_item", &mut context);
            assert_eq!(codes(&diags).contains(&"INVALID_CUSTOM_MODEL_DATA"), expect_error, "value {:?}", value);
            if expect_error {
                assert_eq!(
                    diags.items[0].message,
                    "custom_model_data must be an integer in 1..16777216"
                );
                assert_eq!(diags.items[0].field.as_deref(), Some("Pack.custom_model_data"));
            }
            assert_eq!(info.custom_model_data, expected, "value {:?}", value);
        }
    }

    #[test]
    fn read_pack_model_metadata_flags() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let pack = jo(json!({
            "hand_swap_animation": false,
            "oversized_in_gui": true,
            "swap_animation_scale": 2.5,
        }));
        let info = read_pack_model(Some(&pack), "my_item", &mut context);
        assert!(!info.hand_animation_on_swap);
        assert!(info.oversized_in_gui);
        assert_eq!(info.swap_animation_scale, 2.5);
    }

    #[test]
    fn single_variant_prefers_model_over_texture() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let pack = jo(json!({ "blocking_model": "custom:bmodel", "blocking_texture": "custom:btex" }));
        let info = read_pack_model(Some(&pack), "my_item", &mut context);
        let blocking = info.blocking.unwrap();
        assert_eq!(blocking.path, "custom:bmodel");
        assert_eq!(blocking.origin, ModelOrigin::Model);
        assert!(blocking.generation.is_none());
    }

    #[test]
    fn single_variant_texture_gets_generation() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let pack = jo(json!({ "blocking_texture": "custom:btex" }));
        let info = read_pack_model(Some(&pack), "my_item", &mut context);
        let blocking = info.blocking.unwrap();
        assert_eq!(blocking.path, "custom:btex");
        assert_eq!(blocking.origin, ModelOrigin::Texture);
        assert_eq!(
            blocking.generation,
            Some(jo(json!({
                "parent": "minecraft:item/generated",
                "textures": { "particle": "custom:btex", "layer0": "custom:btex" },
            })))
        );
    }

    #[test]
    fn list_variant_invalid_model_entry_is_dropped_with_error() {
        // asStringList keeps the empty string, normalization then fails on it,
        // so the list yields nothing and the texture fallback is not taken.
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let pack = jo(json!({ "pulling_models": [""], "pulling_textures": ["custom:t"] }));
        let info = read_pack_model(Some(&pack), "my_item", &mut context);
        assert!(info.pulling.is_empty());
        assert_eq!(codes(&diags), vec!["INVALID_RESOURCE_LOCATION"]);
    }

    #[test]
    fn round_predicate_matches_to_fixed_semantics() {
        assert_eq!(round_predicate(1.0, 0.9), 0.9);
        assert_eq!(round_predicate(2.0 / 3.0, 0.9), 0.65); // 13 * 0.05 == 0.65000000000000002 -> "0.65"
        assert_eq!(round_predicate(1.0 / 3.0, 0.9), 0.35);
        assert_eq!(round_predicate(0.5, 0.9), 0.5);
        assert_eq!(round_predicate(0.05, 0.9), 0.05);
        assert_eq!(round_predicate(0.024, 0.9), 0.0);
        assert_eq!(round_predicate(0.95, 0.99), 0.95);
        assert_eq!(round_predicate(0.99, 0.99), 0.99); // rounds to 1.0, capped
        assert_eq!(round_predicate(19.0 / 20.0, 0.99), 0.95);
    }

    #[test]
    fn color_integer_variants() {
        assert_eq!(color_integer(None), 16_777_215);
        assert_eq!(color_integer(Some(&json!(16_711_680))), 16_711_680);
        assert_eq!(color_integer(Some(&json!(16_777_216))), 0); // 24-bit mask wraps
        assert_eq!(color_integer(Some(&json!(-1))), 16_777_215);
        assert_eq!(color_integer(Some(&json!(1.5))), 16_777_215);
        assert_eq!(color_integer(Some(&json!(true))), 16_777_215);
        assert_eq!(color_integer(Some(&json!("#ff0000"))), 16_711_680);
        assert_eq!(color_integer(Some(&json!("#FF0000"))), 16_711_680);
        assert_eq!(color_integer(Some(&json!("#ff00"))), 16_777_215);
        assert_eq!(color_integer(Some(&json!("255,0,0"))), 16_711_680);
        assert_eq!(color_integer(Some(&json!(" 255 , 128 , 0 "))), 16_744_448);
        assert_eq!(color_integer(Some(&json!("255,,0"))), 16_711_680); // empty entry is 0
        assert_eq!(color_integer(Some(&json!("255,x,0"))), 16_777_215);
        assert_eq!(color_integer(Some(&json!("1,2,3,4"))), 16_777_215);
        assert_eq!(color_integer(Some(&json!("256,0,0"))), 16_777_215);
    }

    #[test]
    fn vanilla_reference_tints_table() {
        assert_eq!(
            vanilla_reference_tints_1_21_11("potion"),
            vec![json!({ "type": "potion", "default": -13083194 })]
        );
        assert_eq!(
            vanilla_reference_tints_1_21_11("filled_map"),
            vec![
                json!({ "type": "constant", "value": -1 }),
                json!({ "type": "map_color", "default": 4603950 }),
            ]
        );
        assert_eq!(
            vanilla_reference_tints_1_21_11("grass_block"),
            vec![json!({ "type": "grass", "downfall": 1, "temperature": 0.5 })]
        );
        assert!(vanilla_reference_tints_1_21_11("stone").is_empty());
    }

    #[test]
    fn convert_explicit_item_model_none_is_none() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        assert_eq!(convert_explicit_item_model(None, &mut context), None);
    }

    #[test]
    fn convert_explicit_item_model_strips_metadata_and_normalizes() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let raw = jo(json!({
            "Hand_Animation_On_Swap": false,
            "swap_animation_scale": 3.0,
            "type": "minecraft:condition",
            "property": "minecraft:using_item",
            "on-true": { "type": "model", "model": "custom:thing.json" },
            "on_false": { "type": "model", "path": "other" },
            "component": "dyed_color",
        }));
        let converted = convert_explicit_item_model(Some(&raw), &mut context).unwrap();
        assert_eq!(
            Value::Object(converted),
            json!({
                "type": "condition",
                "property": "using_item",
                "on_true": { "type": "model", "path": "custom:thing" },
                "on_false": { "type": "model", "path": "minecraft:other" },
                "component": "minecraft:dyed_color",
            })
        );
        assert!(diags.items.is_empty());
    }

    #[test]
    fn normalize_ast_model_key_rules() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        // Arrays under "model" in a model node normalize each string entry.
        let array_node = normalize_ast_value(
            &json!({ "type": "model", "model": ["a", "custom:b.json"] }),
            &mut context,
            "",
            "",
        );
        assert_eq!(
            array_node,
            json!({ "type": "model", "model": ["minecraft:a", "custom:b"] })
        );
        // A "model" string outside a model node is left untouched.
        let foreign = normalize_ast_value(
            &json!({ "type": "condition", "model": "x" }),
            &mut context,
            "",
            "",
        );
        assert_eq!(foreign, json!({ "type": "condition", "model": "x" }));
        // Invalid locations keep the raw value and record an error.
        let invalid = normalize_ast_value(
            &json!({ "type": "model", "path": "Bad/Path" }),
            &mut context,
            "",
            "",
        );
        assert_eq!(invalid, json!({ "type": "model", "path": "Bad/Path" }));
        assert_eq!(codes(&diags), vec!["INVALID_RESOURCE_LOCATION"]);
        assert_eq!(diags.items[0].field.as_deref(), Some("ItemModel.model"));
    }

    #[test]
    fn build_legacy_model_override_order_and_predicates() {
        let mut info = pack_info(model_reference("minecraft:my_item"));
        info.blocking = Some(model_reference("custom:blocking"));
        info.charged = Some(model_reference("custom:charged"));
        info.cast = Some(model_reference("custom:cast"));
        info.broken = Some(model_reference("custom:broken"));
        info.firework = Some(model_reference("custom:firework"));
        info.pulling = vec![
            model_reference("custom:p0"),
            model_reference("custom:p1"),
            model_reference("custom:p2"),
        ];
        info.damaged = vec![
            model_reference("custom:d0"),
            model_reference("custom:d1"),
            model_reference("custom:d2"),
        ];
        let result = build_legacy_model(&info);
        assert_eq!(
            Value::Object(result),
            json!({
                "path": "minecraft:my_item",
                "overrides": [
                    { "path": "custom:blocking", "predicate": { "blocking": 1 } },
                    { "path": "custom:charged", "predicate": { "charged": 1 } },
                    { "path": "custom:cast", "predicate": { "cast": 1 } },
                    { "path": "custom:broken", "predicate": { "broken": 1 } },
                    { "path": "custom:firework", "predicate": { "firework": 1 } },
                    { "path": "custom:p0", "predicate": { "pulling": 1, "pull": 0 } },
                    { "path": "custom:p1", "predicate": { "pulling": 1, "pull": 0.65 } },
                    { "path": "custom:p2", "predicate": { "pulling": 1, "pull": 0.9 } },
                    { "path": "custom:d1", "predicate": { "pulling": 1, "damage": 0.35 } },
                    { "path": "custom:d2", "predicate": { "pulling": 1, "damage": 0.65 } },
                ],
            })
        );
    }

    #[test]
    fn convert_models_without_sources_returns_nothing() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let info = default_info();
        let converted = convert_models(&info, None, "paper", None, ClientMode::Modern, &mut context);
        assert!(!converted.generated_item_model);
        assert!(converted.model.is_none());
        assert!(converted.base_model.is_none());
        assert!(converted.metadata.is_none());
        assert!(converted.model_semantics.is_empty());
        assert!(diags.items.is_empty());
    }

    #[test]
    fn convert_models_explicit_modern() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let info = default_info();
        let explicit = jo(json!({
            "hand_animation_on_swap": false,
            "swap_animation_scale": 2.5,
            "type": "model",
            "path": "custom:explicit",
        }));
        let converted = convert_models(&info, Some(&explicit), "paper", None, ClientMode::Modern, &mut context);
        assert_eq!(converted.model, Some(json!({ "type": "model", "path": "custom:explicit" })));
        assert!(converted.generated_item_model);
        assert!(converted.legacy_model.is_none());
        assert_eq!(converted.base_model.as_deref(), Some("minecraft:my_item"));
        let metadata = converted.metadata.unwrap();
        assert!(!metadata.hand_animation_on_swap);
        assert!(!metadata.oversized_in_gui);
        assert_eq!(metadata.swap_animation_scale, 2.5);
        assert_eq!(
            converted.model_semantics.get("modern_source"),
            Some(&json!("ItemModel_or_Pack_shortcut_priority"))
        );
    }

    #[test]
    fn convert_models_pulling_shortcut_modern_and_hybrid() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let mut info = default_info();
        info.has_pack = true;
        info.pulling = vec![model_reference("custom:p0"), model_reference("custom:p1")];
        let converted = convert_models(&info, None, "bow", None, ClientMode::Hybrid, &mut context);
        assert_eq!(
            converted.model,
            Some(json!({
                "type": "condition",
                "property": "using_item",
                "on_true": {
                    "type": "range_dispatch",
                    "property": "use_duration",
                    "scale": 0.05,
                    "entries": [
                        {
                            "threshold": 0,
                            "model": {
                                "type": "model",
                                "path": "custom:p0",
                                "tints": [{ "type": "dye", "default": -1 }],
                            },
                        },
                        {
                            "threshold": 0.9,
                            "model": {
                                "type": "model",
                                "path": "custom:p1",
                                "tints": [{ "type": "dye", "default": -1 }],
                            },
                        },
                    ],
                },
                "on_false": {
                    "type": "model",
                    "path": "minecraft:my_item",
                    "tints": [{ "type": "dye", "default": -1 }],
                },
            }))
        );
        assert!(converted.legacy_model.is_some());
        assert_eq!(
            converted.model_semantics.get("pulling_thresholds"),
            Some(&json!([0, 0.9]))
        );
        assert_eq!(converted.model_semantics.get("modern_source"), Some(&json!("Pack")));
        assert_eq!(converted.model_semantics.get("damaged_legacy_only"), Some(&json!(false)));
    }

    #[test]
    fn convert_models_crossbow_select() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let mut info = default_info();
        info.has_pack = true;
        info.pulling = vec![model_reference("custom:p0")];
        info.charged = Some(model_reference("custom:charged"));
        info.firework = Some(model_reference("custom:firework"));
        let converted = convert_models(&info, None, "crossbow", None, ClientMode::Modern, &mut context);
        let model = converted.model.unwrap();
        assert_eq!(model["type"], json!("select"));
        assert_eq!(model["property"], json!("charge_type"));
        assert_eq!(model["cases"][0]["when"], json!("arrow"));
        assert_eq!(model["cases"][1]["when"], json!("rocket"));
        assert_eq!(model["fallback"]["on_true"]["property"], json!("crossbow/pull"));
        assert_eq!(model["fallback"]["on_true"]["scale"], json!(1));
    }

    #[test]
    fn convert_models_condition_properties() {
        let cast = Some(model_reference("custom:cast"));
        let broken = Some(model_reference("custom:broken"));
        let throwing = Some(model_reference("custom:throwing"));
        let blocking = Some(model_reference("custom:blocking"));
        for (variant, property) in [
            (cast.clone(), "fishing_rod/cast"),
            (broken.clone(), "broken"),
            (throwing.clone(), "using_item"),
            (blocking.clone(), "using_item"),
        ] {
            let mut diags = DiagnosticBag::new();
            let mut context = context_with(&mut diags);
            let mut info = default_info();
            info.has_pack = true;
            info.cast = None;
            info.broken = None;
            info.throwing = None;
            info.blocking = None;
            if property == "fishing_rod/cast" {
                info.cast = variant;
            } else if property == "broken" {
                info.broken = variant;
            } else if variant.as_ref() == throwing.as_ref() {
                info.throwing = variant;
            } else {
                info.blocking = variant;
            }
            let converted = convert_models(&info, None, "shield", None, ClientMode::Modern, &mut context);
            assert_eq!(converted.model.unwrap()["property"], json!(property));
        }
    }

    #[test]
    fn convert_models_legacy_mode_uses_base_and_info_metadata() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let mut info = default_info();
        info.has_pack = true;
        info.hand_animation_on_swap = false;
        info.swap_animation_scale = 3.0;
        info.pulling = vec![model_reference("custom:p0")];
        let explicit = jo(json!({ "hand_animation_on_swap": true, "type": "model", "path": "custom:explicit" }));
        let converted = convert_models(&info, Some(&explicit), "bow", None, ClientMode::Legacy, &mut context);
        assert_eq!(converted.model, Some(json!({ "type": "model", "path": "minecraft:my_item" })));
        assert!(converted.legacy_model.is_some());
        let metadata = converted.metadata.unwrap();
        assert!(!metadata.hand_animation_on_swap);
        assert_eq!(metadata.swap_animation_scale, 3.0);
    }

    #[test]
    fn convert_models_damaged_warning_and_semantics() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let mut info = default_info();
        info.has_pack = true;
        info.damaged = vec![model_reference("custom:d0"), model_reference("custom:d1")];
        let converted = convert_models(&info, None, "sword", None, ClientMode::Modern, &mut context);
        assert_eq!(codes(&diags), vec!["NEXO_DAMAGED_MODEL_LEGACY_QUIRK"]);
        assert!(!diags.items[0].lossy);
        assert_eq!(diags.items[0].field.as_deref(), Some("Pack.damaged_models"));
        assert_eq!(converted.model_semantics.get("damaged_legacy_only"), Some(&json!(true)));
        assert!(converted.legacy_model.is_none());
    }

    #[test]
    fn convert_models_shortcut_precedence_over_explicit() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let mut info = default_info();
        info.has_pack = true;
        info.pulling = vec![model_reference("custom:p0")];
        let explicit = jo(json!({ "type": "model", "path": "custom:explicit" }));
        let converted = convert_models(&info, Some(&explicit), "bow", None, ClientMode::Modern, &mut context);
        assert_eq!(codes(&diags), vec!["PACK_SHORTCUT_PRECEDENCE"]);
        assert_eq!(converted.model.unwrap()["type"], json!("condition"));
    }

    #[test]
    fn convert_models_vanilla_tint_inheritance() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let info = {
            let mut info = default_info();
            info.has_pack = true;
            info
        };
        let converted = convert_models(&info, None, "potion", None, ClientMode::Modern, &mut context);
        assert_eq!(
            converted.model,
            Some(json!({
                "type": "model",
                "path": "minecraft:my_item",
                "tints": [{ "type": "potion", "default": -13083194 }],
            }))
        );
        let mut diags2 = DiagnosticBag::new();
        let mut context2 = context_with(&mut diags2);
        let converted2 = convert_models(&info, None, "stone", None, ClientMode::Modern, &mut context2);
        assert_eq!(converted2.model, Some(json!({ "type": "model", "path": "minecraft:my_item" })));
    }

    #[test]
    fn convert_models_dyeable_with_color() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let mut info = default_info();
        info.has_pack = true;
        info.dyeable = Some(model_reference("custom:dyed"));
        let color = json!("#ff0000");
        let converted = convert_models(
            &info,
            None,
            "leather_chestplate",
            Some(&color),
            ClientMode::Modern,
            &mut context,
        );
        assert_eq!(
            converted.model,
            Some(json!({
                "type": "condition",
                "property": "has_component",
                "component": "minecraft:dyed_color",
                "on_true": {
                    "type": "model",
                    "path": "custom:dyed",
                    "tints": [{ "type": "dye", "default": 16_711_680 }],
                },
                "on_false": { "type": "model", "path": "minecraft:my_item" },
            }))
        );
    }

    #[test]
    fn convert_models_player_head_special() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let mut info = pack_info(ModelReference {
            path: "minecraft:my_item".to_string(),
            generation: Some(jo(json!({
                "parent": "minecraft:item/generated",
                "textures": { "particle": "custom:head" },
            }))),
            blueprint: None,
            origin: ModelOrigin::Texture,
        });
        info.has_pack = true;
        let converted = convert_models(&info, None, "player_head", None, ClientMode::Modern, &mut context);
        assert_eq!(
            converted.model,
            Some(json!({
                "type": "special",
                "base": "minecraft:my_item",
                "model": { "type": "player_head" },
                "generation": {
                    "parent": "minecraft:item/generated",
                    "textures": { "particle": "custom:head" },
                },
            }))
        );
    }

    #[test]
    fn convert_models_composite_wraps_primary() {
        let mut diags = DiagnosticBag::new();
        let mut context = context_with(&mut diags);
        let mut info = default_info();
        info.has_pack = true;
        info.blocking = Some(model_reference("custom:blocking"));
        info.composite = vec![model_reference("custom:comp")];
        let converted = convert_models(&info, None, "shield", None, ClientMode::Modern, &mut context);
        let model = converted.model.unwrap();
        assert_eq!(model["type"], json!("composite"));
        assert_eq!(model["models"][0]["property"], json!("using_item"));
        assert_eq!(model["models"][1], json!({ "type": "model", "path": "custom:comp" }));
    }
}

