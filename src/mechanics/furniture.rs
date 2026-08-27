//! Furniture mechanic conversion (Nexo `Mechanics.furniture` → CraftEngine).
//!
//! Port of `legacy/src/mechanics.ts` lines 138–893. Display transform,
//! Interaction/Shulker/Ghast/Barrier hitboxes, seats, lights, sounds,
//! placement/rotation rules and loot are mapped with Nexo's exact defaults
//! and bounds; unrepresentable Nexo behavior becomes lossy diagnostics
//! instead of guesses.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Number, Value};

use crate::diagnostics::Details;
use crate::json::{
    as_string_list, deep_merge, get_boolean, get_number, get_object, get_string, get_value,
    JsonObject,
};
use crate::resource_location::normalize_sound_location;

use super::{
    compact_vector, config_vector, detail, js_number_string, multiply_quaternion,
    parse_number_list, parse_quaternion, quaternion_identity, quaternion_string, split_with_last,
    uniform_scale, vector_string, Context, FurnitureRuntimeSettings,
};

pub(crate) const MAX_FURNITURE_BARRIER_HITBOXES: usize = 4096;

/// Mirrors TS `Number(...)` for scalar tokens (empty/NaN/infinite → None).
fn parse_js_number(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// JSON number formatted like JS `JSON.stringify`: integers without ".0".
fn json_number(value: f64) -> Value {
    if !value.is_finite() {
        return Value::Null;
    }
    if value.fract() == 0.0 && value.abs() < 9.007199254740992e15 {
        return Value::Number(Number::from(value as i64));
    }
    Value::Number(Number::from_f64(value).expect("finite"))
}

/// JS `Number(x.toFixed(8))`: round to eight decimal places.
fn round8(value: f64) -> f64 {
    format!("{:.8}", value).parse().unwrap_or(value)
}

/// JS `String(value)` for the scalar JSON values produced here.
fn js_string_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => js_number_string(number.as_f64().unwrap_or(0.0)),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn is_safe_integer(value: f64) -> bool {
    value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0
}

/// `-?[0-9]+` (JS `\d` is ASCII-only).
fn is_integer_literal(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn parse_integer(value: Option<&str>) -> f64 {
    let normalized = value.map(|text| text.replace(' ', "")).unwrap_or_default();
    if is_integer_literal(&normalized) {
        normalized.parse::<f64>().unwrap_or(0.0)
    } else {
        0.0
    }
}

/// JS `a..b` range token; anything else collapses to a single point.
fn parse_range(raw: &str) -> (f64, f64) {
    let normalized = raw.replace(' ', "");
    if let Some(index) = normalized.find("..") {
        let low = &normalized[..index];
        let high = &normalized[index + 2..];
        if is_integer_literal(low) && is_integer_literal(high) {
            return (low.parse().unwrap_or(0.0), high.parse().unwrap_or(0.0));
        }
    }
    let point = parse_integer(Some(&normalized));
    (point, point)
}

fn split_words(value: &str) -> Vec<String> {
    value.trim().split_whitespace().map(str::to_string).collect()
}

// Nexo rotates furniture offsets as (x*cos + z*sin, x*sin - z*cos), while
// CraftEngine's local furniture coordinates use the opposite X/Z basis.
fn nexo_position(value: Option<&str>) -> String {
    let [x, y, z] = compact_vector(value, 0.0);
    [-x, y, -z]
        .iter()
        .map(|part| js_number_string(*part))
        .collect::<Vec<_>>()
        .join(",")
}

fn nexo_seat(value: &str) -> String {
    // Nexo's seats list is parsed as a plain Vector; there is no seat-yaw token.
    nexo_position(Some(value))
}

fn finite_token(value: Option<&str>, fallback: f64) -> f64 {
    match value {
        None => fallback,
        Some(text) if text.trim().is_empty() => fallback,
        Some(text) => parse_js_number(text).unwrap_or(fallback),
    }
}

fn first_word_and_rest(value: &str) -> (String, Option<String>) {
    let trimmed = value.trim();
    let Some(index) = trimmed.find(|c: char| c.is_whitespace()) else {
        let first = if trimmed.is_empty() { "0,0,0" } else { trimmed };
        return (first.to_string(), None);
    };
    let rest = trimmed[index..].trim();
    (trimmed[..index].to_string(), if rest.is_empty() { None } else { Some(rest.to_string()) })
}

fn can_recompose_fixed_quarter_turn(properties: Option<&JsonObject>) -> bool {
    let Some(properties) = properties else { return true };
    let [translation_x, _, translation_z] = config_vector(get_value(properties, "translation"), 0.0);
    if translation_x.abs() >= 1e-8 || translation_z.abs() >= 1e-8 {
        return false;
    }
    quaternion_identity(parse_quaternion(get_value(properties, "left_rotation"), "left"))
        && quaternion_identity(parse_quaternion(get_value(properties, "right_rotation"), "right"))
}

fn map_element(properties: Option<&JsonObject>, context: &mut Context) -> JsonObject {
    let transform = properties
        .and_then(|properties| get_string(properties, "display_transform"))
        .map(str::to_lowercase)
        .unwrap_or_else(|| "none".to_string());
    let default_scale = if transform == "fixed" { 0.5 } else { 1.0 };
    let mut element = JsonObject::new();
    element.insert("type".to_string(), json!("item_display"));
    element.insert("item".to_string(), Value::String(context.target_id.clone()));
    // Nexo stores the placed source item's color and applies it to the display
    // stack. CraftEngine's tint source performs the equivalent component copy.
    element.insert("tint_source".to_string(), json!(["minecraft:dyed_color"]));
    element.insert(
        "translation".to_string(),
        Value::String(vector_string(properties.and_then(|properties| get_value(properties, "translation")), 0.0)),
    );
    element.insert(
        "scale".to_string(),
        Value::String(vector_string(properties.and_then(|properties| get_value(properties, "scale")), default_scale)),
    );
    element.insert("display_transform".to_string(), Value::String(transform.clone()));
    element.insert(
        "billboard".to_string(),
        Value::String(
            properties
                .and_then(|properties| get_string(properties, "tracking_rotation"))
                .map(str::to_lowercase)
                .unwrap_or_else(|| "fixed".to_string()),
        ),
    );
    let Some(properties) = properties else { return element };
    let left = parse_quaternion(get_value(properties, "left_rotation"), "left");
    let right = parse_quaternion(get_value(properties, "right_rotation"), "right");
    if !quaternion_identity(left) || !quaternion_identity(right) {
        let combined = multiply_quaternion(left, right);
        element.insert("rotation".to_string(), Value::String(quaternion_string(combined)));
        if !quaternion_identity(right) && !uniform_scale(get_value(properties, "scale"), default_scale) {
            context.diagnostics.warning(
                "FURNITURE_RIGHT_ROTATION_NON_UNIFORM",
                "Nexo applies Minecraft Translation*LeftRotation*Scale*RightRotation, but CraftEngine exposes one pre-scale rotation; moving a non-identity right rotation before non-uniform scale is not exact",
                detail(context, "Mechanics.furniture.properties").lossy(),
            );
        }
    }
    for key in ["view_range", "shadow_strength", "shadow_radius", "glow_color"] {
        if let Some(value) = get_value(properties, key) {
            element.insert(key.to_string(), value.clone());
        }
    }
    if let Some(brightness) = get_object(properties, "brightness") {
        element.insert("brightness".to_string(), Value::Object(brightness.clone()));
    }
    for unsupported in ["display_width", "display_height", "delay", "cullable"] {
        if get_value(properties, unsupported).is_some() {
            context.diagnostics.warning(
                "FURNITURE_DISPLAY_PROPERTY_UNSUPPORTED",
                &format!("CraftEngine 26.8 item-display furniture has no equivalent for Nexo {}", unsupported),
                detail(context, &format!("Mechanics.furniture.properties.{}", unsupported)).lossy(),
            );
        }
    }
    element
}

fn parse_interaction(value: &str, _context: &mut Context) -> JsonObject {
    let (position, size) = first_word_and_rest(value);
    let (width_raw, height_raw) = match &size {
        None => (None, None),
        Some(size) => match size.find(',') {
            // Kotlin substringAfter uses the whole source as its missing-delimiter
            // value, so a single size number means equal width and height.
            None => (Some(size.as_str()), Some(size.as_str())),
            Some(comma) => (Some(&size[..comma]), Some(&size[comma + 1..])),
        },
    };
    let width = finite_token(width_raw, 1.0);
    let height = finite_token(height_raw, 1.0);
    let mut hitbox = JsonObject::new();
    hitbox.insert("type".to_string(), json!("interaction"));
    hitbox.insert("position".to_string(), Value::String(nexo_position(Some(&position))));
    hitbox.insert("width".to_string(), json_number(width));
    hitbox.insert("height".to_string(), json_number(height));
    hitbox.insert("interactive".to_string(), Value::Bool(true));
    hitbox.insert("blocks_building".to_string(), Value::Bool(true));
    hitbox.insert("can_use_item_on".to_string(), Value::Bool(true));
    hitbox.insert("can_be_hit_by_projectile".to_string(), Value::Bool(true));
    hitbox.insert("invisible".to_string(), Value::Bool(false));
    hitbox
}

const BUKKIT_BLOCK_FACES: &[&str] = &[
    "NORTH", "EAST", "SOUTH", "WEST", "UP", "DOWN", "NORTH_EAST", "NORTH_WEST", "SOUTH_EAST",
    "SOUTH_WEST", "WEST_NORTH_WEST", "NORTH_NORTH_WEST", "NORTH_NORTH_EAST", "EAST_NORTH_EAST",
    "EAST_SOUTH_EAST", "SOUTH_SOUTH_EAST", "SOUTH_SOUTH_WEST", "WEST_SOUTH_WEST", "SELF",
];
const CE_DIRECTIONS: &[&str] = &["NORTH", "EAST", "SOUTH", "WEST", "UP", "DOWN"];

fn parse_shulker(value: &str, context: &mut Context) -> JsonObject {
    let words = split_words(value);
    let position = words.first().map(String::as_str).unwrap_or("0,0,0");
    let scale = finite_token(words.get(1).map(String::as_str), 1.0);
    let raw_length = finite_token(words.get(2).map(String::as_str), 1.0);
    let length = raw_length.clamp(1.0, 2.0);
    let raw_direction = words.get(3).map(String::as_str);
    let direction = match raw_direction {
        Some(raw) if CE_DIRECTIONS.contains(&raw) => raw.to_lowercase(),
        _ => "down".to_string(),
    };
    if let Some(raw) = raw_direction {
        if BUKKIT_BLOCK_FACES.contains(&raw) && !CE_DIRECTIONS.contains(&raw) {
            context.diagnostics.warning(
                "SHULKER_DIRECTION_UNSUPPORTED",
                "Nexo accepts diagonal/SELF BlockFace directions that CraftEngine's shulker hitbox cannot represent; DOWN was used",
                detail(context, "Mechanics.furniture.hitbox.shulkers").lossy(),
            );
        }
    }
    let visible_raw = words.get(4).or_else(|| words.get(3)).map(String::as_str).unwrap_or("false");
    let visible = visible_raw.to_lowercase() == "true";
    let peek = (100.0 / std::f64::consts::PI * (3.0 - 2.0 * length).clamp(-1.0, 1.0).acos()).round();
    if visible {
        context.diagnostics.warning(
            "SHULKER_VISIBLE_UNSUPPORTED",
            "CraftEngine 26.8 always makes the Shulker entity invisible; its invisible key only affects an optional Interaction entity",
            detail(context, "Mechanics.furniture.hitbox.shulkers").lossy(),
        );
    }
    let mut hitbox = JsonObject::new();
    hitbox.insert("type".to_string(), json!("shulker"));
    hitbox.insert("position".to_string(), Value::String(nexo_position(Some(position))));
    hitbox.insert("scale".to_string(), json_number(scale));
    hitbox.insert("peek".to_string(), json_number(peek));
    hitbox.insert("direction".to_string(), Value::String(direction));
    hitbox.insert("interaction_entity".to_string(), Value::Bool(false));
    hitbox.insert("interactive".to_string(), Value::Bool(true));
    hitbox.insert("blocks_building".to_string(), Value::Bool(true));
    hitbox.insert("can_use_item_on".to_string(), Value::Bool(true));
    hitbox.insert("can_be_hit_by_projectile".to_string(), Value::Bool(true));
    hitbox.insert("invisible".to_string(), Value::Bool(false));
    hitbox
}

fn parse_ghast(value: &str, context: &mut Context) -> JsonObject {
    let words = split_words(value);
    let rotation = finite_token(words.get(2).map(String::as_str), 0.0);
    // Kotlin toBooleanStrictOrNull is case-sensitive. When rotation is omitted,
    // the final true/false token is also inspected as the visibility shorthand.
    let visible = words.last().map(String::as_str).unwrap_or("") == "true";
    if rotation != 0.0 {
        context.diagnostics.warning(
            "GHAST_ROTATION_UNSUPPORTED",
            "CraftEngine happy_ghast hitbox has no rotation setting",
            detail(context, "Mechanics.furniture.hitbox.ghasts").lossy(),
        );
    }
    if visible {
        context.diagnostics.warning(
            "GHAST_VISIBLE_UNSUPPORTED",
            "CraftEngine happy_ghast hitbox has no Nexo-compatible visible debug state",
            detail(context, "Mechanics.furniture.hitbox.ghasts").lossy(),
        );
    }
    let mut hitbox = JsonObject::new();
    hitbox.insert("type".to_string(), json!("happy_ghast"));
    hitbox.insert("position".to_string(), Value::String(nexo_position(words.first().map(String::as_str))));
    hitbox.insert("scale".to_string(), json_number(finite_token(words.get(1).map(String::as_str), 0.25)));
    hitbox.insert("hard_collision".to_string(), Value::Bool(true));
    hitbox.insert("blocks_building".to_string(), Value::Bool(true));
    hitbox.insert("can_use_item_on".to_string(), Value::Bool(true));
    hitbox.insert("can_be_hit_by_projectile".to_string(), Value::Bool(true));
    hitbox
}

fn parse_barrier_position(value: &str) -> String {
    if value == "origin" {
        return nexo_position(Some("0,0,0"));
    }
    let parts: Vec<&str> = value.split(',').collect();
    let coords = [
        parse_integer(parts.first().copied()),
        parse_integer(parts.get(1).copied()),
        parse_integer(parts.get(2).copied()),
    ];
    let joined = coords.iter().map(|coord| js_number_string(*coord)).collect::<Vec<_>>().join(",");
    nexo_position(Some(&joined))
}

fn parse_barrier_positions(value: &str, context: &mut Context) -> Vec<String> {
    if value == "origin" {
        return vec![nexo_position(Some("0,0,0"))];
    }
    if !value.contains("..") {
        return vec![parse_barrier_position(value)];
    }
    let parts = split_with_last(value, ",", 3);
    if parts.len() < 3 {
        context.diagnostics.error(
            "BARRIER_RANGE_INVALID",
            &format!("Nexo barrier range must contain x,y,z coordinates: {}", value),
            detail(context, "Mechanics.furniture.hitbox.barriers"),
        );
        return Vec::new();
    }
    let ranges = [parse_range(&parts[0]), parse_range(&parts[1]), parse_range(&parts[2])];
    let (xs, ys, zs) = (ranges[0], ranges[1], ranges[2]);
    let span = |range: (f64, f64)| (range.1 - range.0 + 1.0).max(0.0);
    let cardinality = span(xs) * span(ys) * span(zs);
    let endpoints = [xs.0, xs.1, ys.0, ys.1, zs.0, zs.1];
    if !endpoints.iter().all(|endpoint| is_safe_integer(*endpoint))
        || !is_safe_integer(cardinality)
        || cardinality > MAX_FURNITURE_BARRIER_HITBOXES as f64
    {
        context.diagnostics.error(
            "BARRIER_RANGE_TOO_LARGE",
            &format!(
                "Nexo barrier range exceeds the safe {}-position limit: {}",
                MAX_FURNITURE_BARRIER_HITBOXES, value
            ),
            detail(context, "Mechanics.furniture.hitbox.barriers"),
        );
        return Vec::new();
    }
    let mut positions = Vec::new();
    let mut x = xs.0;
    while x <= xs.1 {
        let mut y = ys.0;
        while y <= ys.1 {
            let mut z = zs.0;
            while z <= zs.1 {
                let joined = format!("{},{},{}", js_number_string(x), js_number_string(y), js_number_string(z));
                positions.push(nexo_position(Some(&joined)));
                z += 1.0;
            }
            y += 1.0;
        }
        x += 1.0;
    }
    positions
}

fn hitbox_values(section: &JsonObject, singular: &str) -> Vec<String> {
    let primary = as_string_list(get_value(section, singular));
    if !primary.is_empty() {
        primary
    } else {
        as_string_list(get_value(section, &format!("{}s", singular)))
    }
}

fn barrier_hitbox(position: &str) -> JsonObject {
    let mut hitbox = JsonObject::new();
    // scale 1 + peek 0 is an exact axis-aligned 1×1×1 hard collider in CE.
    // It intentionally stays an entity-backed collider: CE has no declarative
    // owner-tracked virtual-block hitbox type equivalent to Nexo's packets.
    hitbox.insert("type".to_string(), json!("shulker"));
    hitbox.insert("position".to_string(), Value::String(position.to_string()));
    hitbox.insert("scale".to_string(), json_number(1.0));
    hitbox.insert("peek".to_string(), json_number(0.0));
    hitbox.insert("direction".to_string(), json!("up"));
    hitbox.insert("_nexo_barrier".to_string(), Value::Bool(true));
    hitbox.insert("interaction_entity".to_string(), Value::Bool(false));
    hitbox.insert("interactive".to_string(), Value::Bool(true));
    hitbox.insert("blocks_building".to_string(), Value::Bool(true));
    hitbox.insert("can_use_item_on".to_string(), Value::Bool(true));
    hitbox.insert("can_be_hit_by_projectile".to_string(), Value::Bool(true));
    hitbox
}

fn parse_legacy_hitbox(value: &str, context: &mut Context) -> Vec<JsonObject> {
    let words = split_words(value);
    let Some(kind) = words.last() else { return Vec::new() };
    let body = words[..words.len() - 1].join(" ");
    match kind.as_str() {
        "B" | "BARRIER" => vec![barrier_hitbox(&parse_barrier_position(&body))],
        "I" | "INTERACTION" => vec![parse_interaction(&body, context)],
        "S" | "SHULKER" => vec![parse_shulker(&body, context)],
        "G" | "GHAST" => vec![parse_ghast(&body, context)],
        _ => Vec::new(),
    }
}

fn map_hitboxes(raw_hitbox: Option<&Value>, seats: &[String], context: &mut Context) -> Vec<Value> {
    let mut result: Vec<Value> = Vec::new();
    match raw_hitbox {
        None => {
            // Nexo adds a default 1x1 Interaction only when the hitbox key is absent.
            result.push(Value::Object(parse_interaction("0,0,0", context)));
        }
        Some(Value::Object(raw)) => {
            for value in hitbox_values(raw, "interaction") {
                result.push(Value::Object(parse_interaction(&value, context)));
            }
            for value in hitbox_values(raw, "shulker") {
                result.push(Value::Object(parse_shulker(&value, context)));
            }
            for value in hitbox_values(raw, "ghast") {
                result.push(Value::Object(parse_ghast(&value, context)));
            }
            let mut barrier_positions: Vec<String> = Vec::new();
            for value in hitbox_values(raw, "barrier") {
                let parsed = parse_barrier_positions(&value, context);
                if barrier_positions.len() + parsed.len() > MAX_FURNITURE_BARRIER_HITBOXES {
                    context.diagnostics.error(
                        "BARRIER_COUNT_TOO_LARGE",
                        &format!(
                            "Combined Nexo barrier positions exceed the safe {}-position limit",
                            MAX_FURNITURE_BARRIER_HITBOXES
                        ),
                        detail(context, "Mechanics.furniture.hitbox.barriers"),
                    );
                    barrier_positions.clear();
                    break;
                }
                barrier_positions.extend(parsed);
            }
            for position in &barrier_positions {
                result.push(Value::Object(barrier_hitbox(position)));
            }
        }
        Some(other) => {
            for value in as_string_list(Some(other)) {
                result.extend(parse_legacy_hitbox(&value, context).into_iter().map(Value::Object));
            }
        }
    }
    let seat_positions: Vec<String> = seats.iter().map(|seat| nexo_seat(seat)).collect();
    if !seat_positions.is_empty() && !result.is_empty() {
        // CE mounts only seats owned by the hitbox that received the click. Put
        // the same root-relative seats on every converted hitbox so an outer
        // shulker or another visible part cannot hide the mount action. CE
        // deduplicates equal seat positions across hitboxes into one runtime
        // Seat instance.
        for hitbox in result.iter_mut() {
            if let Some(object) = hitbox.as_object_mut() {
                object.insert(
                    "seats".to_string(),
                    Value::Array(seat_positions.iter().map(|position| Value::String(position.clone())).collect()),
                );
            }
        }
    } else {
        // An explicit empty/invalid Nexo hitbox still has its 0.1x0.1 seat
        // entities. Keep tiny CE proxies only for this no-clickable-hitbox
        // fallback case.
        for position in &seat_positions {
            let mut proxy = JsonObject::new();
            proxy.insert("type".to_string(), json!("interaction"));
            proxy.insert("position".to_string(), Value::String(position.clone()));
            proxy.insert("width".to_string(), json_number(0.1));
            proxy.insert("height".to_string(), json_number(0.1));
            proxy.insert("interactive".to_string(), Value::Bool(true));
            proxy.insert("blocks_building".to_string(), Value::Bool(false));
            proxy.insert("can_use_item_on".to_string(), Value::Bool(false));
            proxy.insert("can_be_hit_by_projectile".to_string(), Value::Bool(false));
            proxy.insert("_nexo_seat_proxy".to_string(), Value::Bool(true));
            proxy.insert("seats".to_string(), Value::Array(vec![Value::String(position.clone())]));
            result.push(Value::Object(proxy));
        }
    }
    result
}

struct FurnitureLightMapping {
    lights: Vec<JsonObject>,
    toggleable: bool,
}

fn parse_light_level(raw: Option<&str>, ranged: bool) -> f64 {
    let normalized = raw.map(str::trim).unwrap_or("");
    let parsed = if is_integer_literal(normalized) {
        normalized.parse::<f64>().unwrap_or(15.0)
    } else {
        15.0
    };
    parsed.clamp(if ranged { 1.0 } else { 0.0 }, 15.0)
}

fn parse_light_positions(value: &str, context: &mut Context) -> Vec<JsonObject> {
    let (raw_position, raw_level) = first_word_and_rest(value);
    let coordinate = if raw_position == "origin" { "0,0,0".to_string() } else { raw_position };
    let ranged = coordinate.contains("..");
    let level = parse_light_level(raw_level.as_deref(), ranged);
    if level == 0.0 {
        return Vec::new();
    }
    if !ranged {
        let mut light = JsonObject::new();
        light.insert("position".to_string(), Value::String(parse_barrier_position(&coordinate)));
        light.insert("level".to_string(), json_number(level));
        return vec![light];
    }
    let parts = split_with_last(&coordinate, ",", 3);
    if parts.len() < 3 {
        context.diagnostics.error(
            "FURNITURE_LIGHT_RANGE_INVALID",
            &format!("Nexo light range must contain x,y,z coordinates: {}", value),
            detail(context, "Mechanics.furniture.lights.lights"),
        );
        return Vec::new();
    }
    let ranges = [parse_range(&parts[0]), parse_range(&parts[1]), parse_range(&parts[2])];
    let (xs, ys, zs) = (ranges[0], ranges[1], ranges[2]);
    let span = |range: (f64, f64)| (range.1 - range.0 + 1.0).max(0.0);
    let count = span(xs) * span(ys) * span(zs);
    if count > 4096.0 {
        context.diagnostics.error(
            "FURNITURE_LIGHT_RANGE_TOO_LARGE",
            "Nexo light range expands to more than 4096 positions",
            detail(context, "Mechanics.furniture.lights.lights"),
        );
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut x = xs.0;
    while x <= xs.1 {
        let mut y = ys.0;
        while y <= ys.1 {
            let mut z = zs.0;
            while z <= zs.1 {
                let joined = format!("{},{},{}", js_number_string(x), js_number_string(y), js_number_string(z));
                let mut light = JsonObject::new();
                light.insert("position".to_string(), Value::String(nexo_position(Some(&joined))));
                light.insert("level".to_string(), json_number(level));
                result.push(light);
                z += 1.0;
            }
            y += 1.0;
        }
        x += 1.0;
    }
    result
}

fn map_furniture_lights(
    furniture: &JsonObject,
    hitboxes: &[Value],
    context: &mut Context,
) -> Option<FurnitureLightMapping> {
    let section = get_object(furniture, "lights")?;
    let barrier_positions: HashSet<String> = hitboxes
        .iter()
        .filter_map(|entry| {
            let object = entry.as_object()?;
            if object.get("_nexo_barrier") == Some(&Value::Bool(true)) {
                object.get("position").map(js_string_of)
            } else {
                None
            }
        })
        .collect();
    let parsed: Vec<JsonObject> = as_string_list(get_value(section, "lights"))
        .iter()
        .flat_map(|value| parse_light_positions(value, context))
        .collect();
    let lights: Vec<JsonObject> = parsed
        .iter()
        .filter(|light| {
            light
                .get("position")
                .map(|position| !barrier_positions.contains(&js_string_of(position)))
                .unwrap_or(true)
        })
        .cloned()
        .collect();
    if lights.len() < parsed.len() {
        context.diagnostics.info(
            "NEXO_LIGHT_BARRIER_OVERLAP_IGNORED",
            "Nexo ignores light blocks that overlap its barrier hitboxes; the same overlapping lights were omitted",
            detail(context, "Mechanics.furniture.lights.lights"),
        );
    }
    if get_value(section, "toggled_model").is_some() || get_value(section, "toggled_item_model").is_some() {
        context.diagnostics.warning(
            "FURNITURE_TOGGLED_LIGHT_MODEL_UNSUPPORTED",
            "CraftEngine can toggle the light state, but Nexo's alternate toggled display item needs a separately converted item model",
            detail(context, "Mechanics.furniture.lights").lossy(),
        );
    }
    if lights.is_empty() {
        return None;
    }
    Some(FurnitureLightMapping { lights, toggleable: get_boolean(section, "toggleable", false) })
}

/// TS 477-483: a literal `event.property` key wins over the nested section.
pub(crate) fn nested_number(section: &JsonObject, event: &str, property: &str, fallback: f64) -> f64 {
    if let Some(literal) = get_number(section, &format!("{}.{}", event, property)) {
        return literal;
    }
    match get_object(section, event) {
        Some(nested) => get_number(nested, property).unwrap_or(fallback),
        None => fallback,
    }
}

pub(crate) fn nested_sound(section: &JsonObject, event: &str) -> Option<String> {
    if let Some(sound) = get_string(section, &format!("{}_sound", event)) {
        return Some(sound.to_string());
    }
    if let Some(sound) = get_string(section, &format!("{}.sound", event)) {
        return Some(sound.to_string());
    }
    get_object(section, event).and_then(|nested| get_string(nested, "sound").map(str::to_string))
}

fn map_furniture_sounds(section: Option<&JsonObject>, context: &mut Context) -> Option<JsonObject> {
    let section = section?;
    let defaults: [(&str, f64, f64); 3] = [("place", 1.0, 0.8), ("break", 1.0, 0.8), ("hit", 0.25, 0.5)];
    let mut sounds = JsonObject::new();
    for (event, volume_fallback, pitch_fallback) in defaults {
        // TS `if (!sound) continue;` skips empty strings too.
        let Some(sound) = nested_sound(section, event).filter(|sound| !sound.is_empty()) else { continue };
        let details = detail(context, &format!("Mechanics.furniture.block_sounds.{}", event));
        let id = normalize_sound_location(&sound, context.diagnostics, &details).unwrap_or(sound);
        let mut entry = JsonObject::new();
        entry.insert("id".to_string(), Value::String(id));
        entry.insert("volume".to_string(), json_number(nested_number(section, event, "volume", volume_fallback)));
        entry.insert("pitch".to_string(), json_number(nested_number(section, event, "pitch", pitch_fallback)));
        sounds.insert(event.to_string(), Value::Object(entry));
    }
    // TS truthiness: empty step/fall strings do not trigger the warning.
    let has_step = nested_sound(section, "step").is_some_and(|sound| !sound.is_empty());
    let has_fall = nested_sound(section, "fall").is_some_and(|sound| !sound.is_empty());
    if has_step || has_fall {
        context.diagnostics.warning(
            "FURNITURE_STEP_FALL_SOUND_UNSUPPORTED",
            "CraftEngine furniture settings have no equivalent step/fall trigger",
            detail(context, "Mechanics.furniture.block_sounds").lossy(),
        );
    }
    if sounds.is_empty() { None } else { Some(sounds) }
}

struct PlacementMapping {
    variants: Vec<String>,
    rules: JsonObject,
    has_limited: bool,
    floor: bool,
    roof: bool,
    wall: bool,
    rotation_step: f64,
}

fn map_placement(furniture: &JsonObject, context: &mut Context) -> PlacementMapping {
    let limited = get_object(furniture, "limited_placing");
    // Nexo 1.26 computes anyRestrictions with nested Bukkit defaults: floor
    // defaults to roof, roof defaults to wall, and wall defaults false. This
    // deliberately preserves edge cases such as floor:false + roof:true, where
    // an unspecified wall still defaults to enabled.
    let any_restrictions = match limited {
        Some(limited) => get_boolean(limited, "floor", get_boolean(limited, "roof", get_boolean(limited, "wall", false))),
        None => false,
    };
    let enabled = |key: &str| -> bool {
        match limited {
            Some(limited) => get_boolean(limited, key, !any_restrictions),
            None => true,
        }
    };
    let floor = enabled("floor");
    let roof = enabled("roof");
    let wall = enabled("wall");
    let pairs: [(bool, &str); 3] = [(floor, "ground"), (roof, "ceiling"), (wall, "wall")];
    let variants: Vec<String> = pairs
        .iter()
        .filter(|(allowed, _)| *allowed)
        .map(|(_, ce)| ce.to_string())
        .collect();
    let restricted = get_string(furniture, "restricted_rotation")
        .map(str::to_uppercase)
        .unwrap_or_else(|| "STRICT".to_string());
    // Nexo 1.26 initially quantizes NONE and STRICT to the same eight Bukkit
    // Rotation values. VERY_STRICT removes the diagonal values.
    let rotation = if restricted == "VERY_STRICT" { "four" } else { "eight" };
    if !matches!(restricted.as_str(), "VERY_STRICT" | "STRICT" | "NONE") {
        context.diagnostics.warning(
            "RESTRICTED_ROTATION_UNKNOWN",
            "Unknown Nexo restricted_rotation; STRICT/eight used",
            detail(context, "Mechanics.furniture.restricted_rotation").lossy(),
        );
    }
    let mut rules = JsonObject::new();
    for variant in &variants {
        rules.insert(variant.clone(), json!({ "rotation": rotation, "alignment": "center" }));
    }
    if let Some(limited) = limited {
        let unsupported_keys = ["type", "block_types", "block_tags", "nexo_blocks", "radius_limitation", "world"];
        if unsupported_keys.iter().any(|key| get_value(limited, key).is_some()) {
            context.diagnostics.warning(
                "LIMITED_PLACING_CONDITIONS_UNSUPPORTED",
                "Block allow/deny lists, worlds, and radius restrictions need CraftEngine conditions or an API extension",
                detail(context, "Mechanics.furniture.limited_placing").lossy(),
            );
        }
    }
    // Nexo placement uses 4/8 facings, while a later rotatable click advances
    // by half that placement interval (VERY_STRICT=45°, otherwise 22.5°).
    let rotation_step = if restricted == "VERY_STRICT" { 45.0 } else { 22.5 };
    PlacementMapping { variants, rules, has_limited: limited.is_some(), floor, roof, wall, rotation_step }
}

struct RotatableMapping {
    enabled: bool,
    on_sneak: bool,
    degree: f64,
    conditions: Vec<JsonObject>,
}

fn map_rotatable(
    furniture: &JsonObject,
    placement: &PlacementMapping,
    runtime: Option<&FurnitureRuntimeSettings>,
) -> RotatableMapping {
    let raw = get_value(furniture, "rotatable");
    let mut enabled = false;
    let mut on_sneak = runtime.and_then(|settings| settings.default_rotatable_on_sneak).unwrap_or(false);
    match raw {
        Some(Value::Bool(flag)) => enabled = *flag,
        Some(Value::Object(raw)) => {
            // Bukkit ConfigurationSection#getBoolean has a false default for
            // both nested keys. Nexo only applies the factory default to
            // scalar booleans.
            enabled = get_boolean(raw, "rotatable", false);
            on_sneak = get_boolean(raw, "on_sneak", false);
        }
        _ => {}
    }
    if !enabled {
        return RotatableMapping { enabled: false, on_sneak, degree: placement.rotation_step, conditions: Vec::new() };
    }
    // Nexo compares the configured strings directly with Bukkit GameMode.name();
    // preserve case and unknown values rather than normalizing them into matches.
    let modes: Vec<String> = runtime
        .and_then(|settings| settings.rotation_gamemodes.clone())
        .unwrap_or_else(|| vec!["SURVIVAL".to_string(), "CREATIVE".to_string()]);
    let mut conditions: Vec<JsonObject> = Vec::new();
    let mut sneak_condition = JsonObject::new();
    sneak_condition.insert(
        "type".to_string(),
        Value::String((if on_sneak { "expression" } else { "!expression" }).to_string()),
    );
    sneak_condition.insert("expression".to_string(), json!("<arg:player.is_sneaking>"));
    conditions.push(sneak_condition);
    // Nexo uses an empty list to disable player rotation in every game mode.
    // An any_of with zero terms means true in CE, so emit an always-false equals.
    let term_modes: Vec<String> = if modes.is_empty() {
        vec!["__NEXO_NO_GAMEMODE__".to_string()]
    } else {
        modes
    };
    let terms: Vec<Value> = term_modes
        .iter()
        .map(|mode| json!({ "type": "equals", "value1": "<arg:player.gamemode>", "value2": mode }))
        .collect();
    let mut any_of = JsonObject::new();
    any_of.insert("type".to_string(), json!("any_of"));
    any_of.insert("terms".to_string(), Value::Array(terms));
    conditions.push(any_of);
    RotatableMapping { enabled: true, on_sneak, degree: placement.rotation_step, conditions }
}

fn shifted_vector(value: Option<&Value>, offset: [f64; 3]) -> String {
    let original = parse_number_list(value).unwrap_or_else(|| vec![0.0, 0.0, 0.0]);
    (0..3)
        .map(|index| {
            let component = original.get(index).copied().unwrap_or(0.0) + offset[index];
            js_number_string(round8(component))
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn shifted_seat(value: &str, offset: [f64; 3]) -> String {
    let words = split_words(value);
    let first = words.first().map(|word| Value::String(word.clone()));
    let position = shifted_vector(first.as_ref(), offset);
    let mut parts = vec![position];
    parts.extend(words.iter().skip(1).cloned());
    parts.join(" ")
}

fn number_equals(value: Option<&Value>, expected: f64) -> bool {
    matches!(value, Some(Value::Number(number)) if number.as_f64() == Some(expected))
}

fn omit_default_hitbox_fields(hitbox: &mut JsonObject) {
    // CraftEngine 26.8 already applies these parser defaults. Keeping only
    // values that differ follows the reference converter and avoids noisy
    // boilerplate.
    for key in ["interactive", "blocks_building", "can_use_item_on", "can_be_hit_by_projectile"] {
        if matches!(hitbox.get(key), Some(Value::Bool(true))) {
            hitbox.remove(key);
        }
    }
    if matches!(hitbox.get("invisible"), Some(Value::Bool(false))) {
        hitbox.remove("invisible");
    }
    match hitbox.get("type").and_then(Value::as_str) {
        Some("interaction") => {
            if number_equals(hitbox.get("width"), 1.0) {
                hitbox.remove("width");
            }
            if number_equals(hitbox.get("height"), 1.0) {
                hitbox.remove("height");
            }
        }
        Some("shulker") => {
            if number_equals(hitbox.get("scale"), 1.0) {
                hitbox.remove("scale");
            }
            if number_equals(hitbox.get("peek"), 0.0) {
                hitbox.remove("peek");
            }
            if matches!(hitbox.get("direction"), Some(Value::String(direction)) if direction == "up") {
                hitbox.remove("direction");
            }
            if matches!(hitbox.get("interaction_entity"), Some(Value::Bool(true))) {
                hitbox.remove("interaction_entity");
            }
        }
        Some("happy_ghast") => {
            if number_equals(hitbox.get("scale"), 1.0) {
                hitbox.remove("scale");
            }
            if matches!(hitbox.get("hard_collision"), Some(Value::Bool(true))) {
                hitbox.remove("hard_collision");
            }
        }
        _ => {}
    }
}

fn shifted_hitboxes(
    hitboxes: &[Value],
    base_offset: [f64; 3],
    interaction_offset: [f64; 3],
    barrier_offset: [f64; 3],
    seat_entity_offset: [f64; 3],
    seat_player_offset: [f64; 3],
) -> Vec<Value> {
    hitboxes
        .iter()
        .map(|raw| {
            let Some(object) = raw.as_object() else {
                return raw.clone();
            };
            let mut hitbox = object.clone();
            let seat_proxy = matches!(hitbox.get("_nexo_seat_proxy"), Some(Value::Bool(true)));
            let nexo_barrier = matches!(hitbox.get("_nexo_barrier"), Some(Value::Bool(true)));
            hitbox.remove("_nexo_seat_proxy");
            hitbox.remove("_nexo_barrier");
            let offset = if seat_proxy {
                seat_entity_offset
            } else if nexo_barrier {
                barrier_offset
            } else if hitbox.get("type").and_then(Value::as_str) == Some("interaction") {
                interaction_offset
            } else {
                base_offset
            };
            let position = shifted_vector(hitbox.get("position"), offset);
            hitbox.insert("position".to_string(), Value::String(position));
            let shifted_seats = match hitbox.get("seats") {
                Some(Value::Array(seats)) => Some(Value::Array(
                    seats
                        .iter()
                        .map(|seat| match seat {
                            Value::String(text) => Value::String(shifted_seat(text, seat_player_offset)),
                            other => other.clone(),
                        })
                        .collect(),
                )),
                _ => None,
            };
            if let Some(seats) = shifted_seats {
                hitbox.insert("seats".to_string(), seats);
            }
            omit_default_hitbox_fields(&mut hitbox);
            Value::Object(hitbox)
        })
        .collect()
}

// CE documents every light position relative to the furniture root. Nexo uses
// its spawned base ItemDisplay as the light origin, so translate that source
// origin into each CE placement root before emitting the documented positions.
fn shifted_furniture_lights(lights: &[JsonObject], offset: [f64; 3]) -> Vec<JsonObject> {
    lights
        .iter()
        .map(|raw_light| {
            let mut light = raw_light.clone();
            let position = shifted_vector(light.get("position"), offset);
            light.insert("position".to_string(), Value::String(position));
            light
        })
        .collect()
}

/// TS 660-667: an absent drop config or a single guaranteed self-drop needs
/// no loot-table conditions.
pub(crate) fn is_simple_self_loot(drop: Option<&JsonObject>, source_item: &str) -> bool {
    let Some(drop) = drop else { return true };
    let Some(Value::Array(loots)) = get_value(drop, "loots") else { return false };
    if loots.len() != 1 {
        return false;
    }
    let Some(Value::Object(entry)) = loots.first() else { return false };
    get_string(entry, "nexo_item") == Some(source_item)
        && get_number(entry, "probability").unwrap_or(1.0) == 1.0
        && get_number(entry, "amount").unwrap_or(1.0) == 1.0
}

fn map_furniture_loot(drop: Option<&JsonObject>, context: &mut Context) -> Option<JsonObject> {
    if is_simple_self_loot(drop, &context.item) {
        let mut entry = JsonObject::new();
        entry.insert("type".to_string(), json!("furniture_item"));
        entry.insert("item".to_string(), Value::String(context.target_id.clone()));
        let mut pool = JsonObject::new();
        pool.insert("rolls".to_string(), json!(1));
        pool.insert("entries".to_string(), Value::Array(vec![Value::Object(entry)]));
        let mut loot = JsonObject::new();
        loot.insert("pools".to_string(), Value::Array(vec![Value::Object(pool)]));
        return Some(loot);
    }
    if let Some(drop) = drop {
        if matches!(get_value(drop, "loots"), Some(Value::Array(loots)) if loots.is_empty()) {
            return None;
        }
    }
    context.diagnostics.warning(
        "FURNITURE_LOOT_COMPLEX",
        "Complex Nexo probability/tool/silk-touch loot needs manual CraftEngine loot-table conditions",
        detail(context, "Mechanics.furniture.drop").lossy(),
    );
    None
}

/// Shared FIXED pitch/yaw handling of the ground and ceiling variants.
fn apply_fixed_orientation(variant_element: &mut JsonObject, pitch: f64, yaw_half_turn: bool, recompose: bool) {
    if pitch != 0.0 && yaw_half_turn && recompose {
        // Yπ·X(pitch)·M·Yπ = X(-pitch)·(Yπ·M)·Yπ. For Nexo's common M=T_y·S
        // transform this is runtime-identical while remaining correctly
        // oriented in CE tooling instead of being vertically inverted.
        variant_element.insert("pitch".to_string(), json_number(-pitch));
        variant_element.insert("rotation".to_string(), Value::String("0,1,0,0".to_string()));
    } else {
        if pitch != 0.0 {
            variant_element.insert("pitch".to_string(), json_number(pitch));
        }
        if yaw_half_turn {
            variant_element.insert("yaw".to_string(), json_number(-180.0));
        }
    }
}

pub(crate) struct FurnitureConversion {
    pub definition: JsonObject,
    pub behavior: JsonObject,
    pub semantics: JsonObject,
}

pub(crate) fn convert_furniture(
    furniture: &JsonObject,
    context: &mut Context,
    default_properties: Option<&JsonObject>,
    runtime: Option<&FurnitureRuntimeSettings>,
) -> FurnitureConversion {
    let placement = map_placement(furniture, context);
    let rotatable = map_rotatable(furniture, &placement, runtime);
    let merged_properties;
    let properties: Option<&JsonObject> = if let Some(defaults) = default_properties {
        let local = get_object(furniture, "properties").cloned().unwrap_or_default();
        merged_properties = deep_merge(defaults, &local);
        Some(&merged_properties)
    } else {
        get_object(furniture, "properties")
    };
    let mut element = map_element(properties, context);
    if let Some(placed_item) = get_string(furniture, "item").filter(|item| !item.is_empty()) {
        let namespace = match context.target_id.find(':') {
            Some(separator) => context.target_id[..separator].to_string(),
            // String.prototype.slice(0, -1) drops the last character when the
            // target id has no colon.
            None => {
                let mut shortened = context.target_id.clone();
                shortened.pop();
                shortened
            }
        };
        let placed_item = placed_item.to_lowercase();
        let item = if placed_item.contains(':') {
            placed_item
        } else {
            format!("{}:{}", namespace, placed_item)
        };
        element.insert("item".to_string(), Value::String(item));
    }
    if get_string(furniture, "item_model").map_or(false, |model| !model.is_empty()) {
        context.diagnostics.warning(
            "FURNITURE_ITEM_MODEL_OVERRIDE",
            "Nexo furniture item_model changes the placed stack independently; CraftEngine needs a dedicated display item for an exact equivalent",
            detail(context, "Mechanics.furniture.item_model").lossy(),
        );
    }
    let seats = as_string_list(get_value(furniture, "seats"));
    let hitboxes = map_hitboxes(get_value(furniture, "hitbox"), &seats, context);
    let light_mapping = map_furniture_lights(furniture, &hitboxes, context);
    let mut variants = JsonObject::new();
    // Light positions follow each explicit ground/ceiling/wall anchor directly.
    let mut variant_offsets: HashMap<String, [f64; 3]> = HashMap::new();
    let fixed = element.get("display_transform").and_then(Value::as_str) == Some("fixed");
    let scale = config_vector(properties.and_then(|properties| get_value(properties, "scale")), if fixed { 0.5 } else { 1.0 });
    let offset_against_blocks = properties.map_or(true, |properties| get_boolean(properties, "offset_against_blocks", true));
    let translation = config_vector(properties.and_then(|properties| get_value(properties, "translation")), 0.0);
    let recompose_fixed_quarter_turn = fixed && can_recompose_fixed_quarter_turn(properties);
    let has_ordinary_interaction = hitboxes.iter().any(|entry| {
        entry.as_object().map_or(false, |object| {
            object.get("type").and_then(Value::as_str) == Some("interaction")
                && object.get("_nexo_seat_proxy") != Some(&Value::Bool(true))
        })
    });
    if offset_against_blocks && has_ordinary_interaction && translation[1].abs() > 1e-8 {
        context.diagnostics.warning(
            "FURNITURE_INTERACTION_PARTIAL_TRANSLATION_DYNAMIC",
            "Nexo conditionally removes display translation.y from Interaction hitboxes above partial-height support; the concise CraftEngine base variant preserves the ordinary local hitbox offset",
            detail(context, "Mechanics.furniture.properties.translation").lossy(),
        );
    }
    for variant in placement.variants.iter() {
        let mut variant_element = element.clone();
        let offset: [f64; 3];
        let mut barrier_offset = [0.0_f64, 0.0, 0.0];
        if variant == "ground" {
            // FIXED always uses Nexo's block-center helper on an UP face, even
            // when limited_placing is absent. Other transforms use the block's
            // full center.
            offset = [0.0, if fixed { 0.0 } else { 0.5 }, 0.0];
            let pitch = if fixed && placement.has_limited && placement.floor { -90.0 } else { 0.0 };
            let yaw_half_turn = fixed && (!placement.has_limited || placement.roof);
            apply_fixed_orientation(&mut variant_element, pitch, yaw_half_turn, recompose_fixed_quarter_turn);
        } else if variant == "ceiling" {
            offset = [0.0, if placement.has_limited && placement.roof { -0.01 } else { -0.5 }, 0.0];
            // A Nexo Barrier is the target block cell. CE's shulker position is
            // its bottom-center, one full block below a ceiling click plane.
            barrier_offset = [0.0, -1.0, 0.0];
            let pitch = if fixed && placement.has_limited && placement.roof { 90.0 } else { 0.0 };
            let yaw_half_turn = fixed && (!placement.has_limited || placement.roof);
            apply_fixed_orientation(&mut variant_element, pitch, yaw_half_turn, recompose_fixed_quarter_turn);
        } else {
            // CE roots wall furniture on the hit plane. Nexo roots it in the
            // target cell, moving a FIXED display toward the wall whenever no
            // solid support exists below; Nexo performs this before
            // offset_against_blocks is checked.
            let wall_visual_z = if fixed && placement.has_limited && placement.wall {
                round8(0.5 - 0.98 * scale[1])
            } else {
                0.5
            };
            offset = [0.0, 0.0, wall_visual_z];
            // Nexo Barrier coordinates are block-cell locations, independent
            // from the ItemDisplay's wall translation. Keep them at target-cell
            // center.
            barrier_offset = [0.0, -0.5, 0.5];
        }
        // Nexo's packet-backed Interaction origin is the ItemDisplay location
        // minus 0.5Y, plus the display translation component rotated onto
        // world Y. CE's Interaction position is likewise the bottom-center of
        // its AABB.
        let interaction_translation_y = if fixed {
            if variant == "ceiling" { -translation[2] } else { translation[2] }
        } else {
            translation[1]
        };
        let interaction_offset = [offset[0], offset[1] - 0.5 + interaction_translation_y, offset[2]];
        if offset.iter().any(|part| *part != 0.0) {
            variant_element.insert("position".to_string(), Value::String(shifted_vector(None, offset)));
        }
        let seat_entity_offset = [offset[0], offset[1] + translation[1], offset[2]];
        // Nexo spawns its seat Interaction at the configured Y. CE's BukkitSeat
        // unconditionally adds 0.6 before spawning the vehicle, so subtract
        // exactly 0.6 here to keep the final riding anchor at Nexo's configured
        // height.
        let seat_player_offset = [offset[0], offset[1] + translation[1] - 0.6, offset[2]];
        if variant == "ground" {
            barrier_offset = offset;
        }
        let mut variant_value = JsonObject::new();
        variant_value.insert("elements".to_string(), Value::Array(vec![Value::Object(variant_element)]));
        variant_value.insert(
            "hitboxes".to_string(),
            Value::Array(shifted_hitboxes(&hitboxes, offset, interaction_offset, barrier_offset, seat_entity_offset, seat_player_offset)),
        );
        variants.insert(variant.clone(), Value::Object(variant_value));
        // Nexo's light origin is this placement's base ItemDisplay location,
        // while CE's documented light origin is the furniture root. Record the
        // explicit source-origin -> CE-root translation independently from
        // element parsing.
        variant_offsets.insert(variant.clone(), offset);
    }
    if !seats.is_empty() && (translation[0].abs() > 1e-8 || translation[2].abs() > 1e-8) {
        context.diagnostics.warning(
            "FURNITURE_SEAT_HORIZONTAL_TRANSLATION",
            "Nexo adds display translation to seats in world axes, which cannot be represented for every rotated CraftEngine placement",
            detail(context, "Mechanics.furniture.seats").lossy(),
        );
    }
    if placement.wall && !(fixed && placement.has_limited && placement.wall) {
        context.diagnostics.warning(
            "FURNITURE_WALL_YAW_DIFFERENCE",
            "CraftEngine wall furniture faces the clicked wall, while this Nexo configuration keeps the player-derived yaw",
            detail(context, "Mechanics.furniture.limited_placing.wall").lossy(),
        );
    }
    // Nexo's support-derived horizontal click is an alternate input path to the
    // same ground/ceiling state. CE reaches that state natively by clicking the
    // UP/DOWN support face, so no extra wall variant (and no lossy warning) is
    // emitted; adding one would create unsupported floating placements.
    if placement.wall && fixed && !placement.has_limited {
        context.diagnostics.warning(
            "FURNITURE_WALL_VERTICAL_OFFSET_DYNAMIC",
            "Nexo moves unrestricted FIXED wall furniture down by half a block when the target has solid support below; CraftEngine cannot make this vertical position support-dependent",
            detail(context, "Mechanics.furniture.properties.display_transform").lossy(),
        );
    }
    let mut settings = JsonObject::new();
    settings.insert("item".to_string(), Value::String(context.target_id.clone()));
    if let Some(sounds) = map_furniture_sounds(get_object(furniture, "block_sounds"), context) {
        settings.insert("sounds".to_string(), Value::Object(sounds));
    }
    let mut right_click_functions: Vec<JsonObject> = Vec::new();
    let mut toggleable_light = false;
    let mut glowing_behavior: Option<JsonObject> = None;
    if let Some(light_mapping) = &light_mapping {
        let mut config_context = JsonObject::new();
        config_context.insert("config_file".to_string(), json!("plugins/CraftEngine/config.yml"));
        config_context.insert("setting".to_string(), json!("furniture.light-system.enable"));
        config_context.insert("required_value".to_string(), Value::Bool(true));
        config_context.insert(
            "wiki".to_string(),
            json!("https://xiao-momi.github.io/craft-engine-wiki/configuration/furniture/behaviors/glowing_furniture"),
        );
        context.diagnostics.warning(
            "CRAFTENGINE_FURNITURE_LIGHT_SYSTEM_REQUIRED",
            "CraftEngine glowing_furniture requires furniture.light-system.enable: true in the server's config.yml",
            Details::new().context(config_context),
        );
        let mut lit_variants = JsonObject::new();
        let original_names: Vec<String> = variants.keys().cloned().collect();
        for name in &original_names {
            let offset = variant_offsets.get(name).copied().unwrap_or([0.0, 0.0, 0.0]);
            let lights: Vec<Value> = shifted_furniture_lights(&light_mapping.lights, offset)
                .into_iter()
                .map(Value::Object)
                .collect();
            lit_variants.insert(name.clone(), Value::Array(lights));
        }
        let first_lights: Option<Value> = lit_variants.values().next().cloned();
        let uniform_lights = first_lights.as_ref().map_or(false, |first| {
            let first_json = serde_json::to_string(first).unwrap_or_default();
            lit_variants
                .values()
                .all(|lights| serde_json::to_string(lights).unwrap_or_default() == first_json)
        });
        let mut glowing = JsonObject::new();
        glowing.insert("type".to_string(), json!("glowing_furniture"));
        // Follow the Wiki's canonical forms: `lights` for one/uniform lit state
        // and `variants` only when positions differ or unlit variants must stay
        // dark.
        if !light_mapping.toggleable && uniform_lights {
            glowing.insert("lights".to_string(), first_lights.expect("at least one lit variant"));
        } else {
            glowing.insert("variants".to_string(), Value::Object(lit_variants));
        }
        // Match CraftEngine's shipped default:candelabrum configuration. The
        // parser accepts behavior(s), but the official default pack uses this
        // plural key with a mapping for one behavior.
        glowing_behavior = Some(glowing);
        if light_mapping.toggleable {
            toggleable_light = true;
            let mut cases: Vec<Value> = Vec::new();
            for name in &original_names {
                let unlit = format!("{}_unlit", name);
                let base = variants.get(name).expect("variant present").clone();
                variants.insert(unlit.clone(), base);
                cases.push(json!({ "when": name, "functions": [{ "type": "set_furniture_variant", "variant": unlit }] }));
                cases.push(json!({ "when": unlit, "functions": [{ "type": "set_furniture_variant", "variant": name }] }));
            }
            let mut when = JsonObject::new();
            when.insert("type".to_string(), json!("when"));
            when.insert("source".to_string(), json!("<arg:furniture.variant>"));
            when.insert("cases".to_string(), Value::Array(cases));
            right_click_functions.push(when);
            if seats.is_empty() {
                let mut tick = JsonObject::new();
                tick.insert("type".to_string(), json!("update_interaction_tick"));
                right_click_functions.push(tick);
            } else {
                // Sneaking never enters Nexo's seat branch; consume that
                // interaction after toggling so CE does not forward the held
                // item to the hitbox.
                let mut tick = JsonObject::new();
                tick.insert("type".to_string(), json!("update_interaction_tick"));
                tick.insert(
                    "conditions".to_string(),
                    json!([{ "type": "expression", "expression": "<arg:player.is_sneaking>" }]),
                );
                right_click_functions.push(tick);
            }
        }
    }
    if rotatable.enabled {
        // Nexo treats an allowed rotation as the winning interaction even when
        // the new orientation collides. Mark it handled synchronously before CE
        // starts its collision-aware asynchronous move, and never retry another
        // angle.
        if !(toggleable_light && seats.is_empty()) {
            let mut tick = JsonObject::new();
            tick.insert("type".to_string(), json!("update_interaction_tick"));
            tick.insert(
                "conditions".to_string(),
                Value::Array(rotatable.conditions.iter().map(|condition| Value::Object(condition.clone())).collect()),
            );
            right_click_functions.push(tick);
        }
        let mut rotate = JsonObject::new();
        rotate.insert("type".to_string(), json!("rotate_furniture"));
        rotate.insert("degree".to_string(), json_number(rotatable.degree));
        rotate.insert(
            "conditions".to_string(),
            Value::Array(rotatable.conditions.iter().map(|condition| Value::Object(condition.clone())).collect()),
        );
        right_click_functions.push(rotate);
    }
    let mut events: Vec<Value> = Vec::new();
    if !right_click_functions.is_empty() {
        let mut event = JsonObject::new();
        event.insert("on".to_string(), json!("right_click"));
        event.insert(
            "functions".to_string(),
            Value::Array(right_click_functions.into_iter().map(Value::Object).collect()),
        );
        events.push(Value::Object(event));
    }
    let loot = map_furniture_loot(get_object(furniture, "drop"), context);
    let mut definition = JsonObject::new();
    definition.insert("settings".to_string(), Value::Object(settings));
    definition.insert("variants".to_string(), Value::Object(variants));
    if let Some(glowing) = glowing_behavior {
        definition.insert("behaviors".to_string(), Value::Object(glowing));
    }
    if !events.is_empty() {
        definition.insert("events".to_string(), Value::Array(events));
    }
    if let Some(loot) = loot {
        definition.insert("loot".to_string(), Value::Object(loot));
    }
    let unsupported = [
        "storage", "jukebox", "farmland_required", "evolution", "modelengine_id", "clickActions",
        "blocklocker", "waterloggable", "beds", "door", "states", "connectable", "placements",
        "light", "text_entities", "text_display",
    ];
    for key in unsupported {
        if get_value(furniture, key).is_some() {
            context.diagnostics.warning(
                "FURNITURE_MECHANIC_UNSUPPORTED",
                &format!("Nexo furniture mechanic {} requires manual or API migration", key),
                detail(context, &format!("Mechanics.furniture.{}", key)).lossy(),
            );
        }
    }
    let mut semantics = JsonObject::new();
    semantics.insert(
        "placements".to_string(),
        Value::Array(placement.variants.iter().map(|variant| Value::String(variant.clone())).collect()),
    );
    semantics.insert(
        "collision_types".to_string(),
        Value::Array(
            hitboxes
                .iter()
                .map(|entry| match entry.as_object() {
                    Some(object) => Value::String(
                        object.get("type").and_then(Value::as_str).unwrap_or("interaction").to_string(),
                    ),
                    None => Value::String("unknown".to_string()),
                })
                .collect(),
        ),
    );
    if let Some(light_mapping) = &light_mapping {
        semantics.insert("lights".to_string(), json_number(light_mapping.lights.len() as f64));
        semantics.insert("toggleable_light".to_string(), Value::Bool(light_mapping.toggleable));
    }
    semantics.insert("rotatable".to_string(), Value::Bool(rotatable.enabled));
    if rotatable.enabled {
        semantics.insert("rotation_on_sneak".to_string(), Value::Bool(rotatable.on_sneak));
        semantics.insert("rotation_degree".to_string(), json_number(rotatable.degree));
    }
    let mut behavior = JsonObject::new();
    behavior.insert("type".to_string(), json!("furniture_item"));
    behavior.insert("furniture".to_string(), Value::String(context.target_id.clone()));
    behavior.insert("rules".to_string(), Value::Object(placement.rules));
    FurnitureConversion { definition, behavior, semantics }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::DiagnosticBag;

    fn context(diagnostics: &mut DiagnosticBag) -> Context<'_> {
        Context {
            source: "test.yml".to_string(),
            item: "TEST".to_string(),
            target_id: "ns:target".to_string(),
            diagnostics,
        }
    }

    #[test]
    fn barrier_range_expands_with_flipped_xz() {
        let mut diagnostics = DiagnosticBag::new();
        let mut ctx = context(&mut diagnostics);
        assert_eq!(parse_barrier_positions("1..2,0,0", &mut ctx), vec!["-1,0,0".to_string(), "-2,0,0".to_string()]);
        assert_eq!(parse_barrier_positions("origin", &mut ctx), vec!["0,0,0".to_string()]);
    }

    #[test]
    fn minimal_furniture_converts_three_placements() {
        let mut diagnostics = DiagnosticBag::new();
        let mut ctx = context(&mut diagnostics);
        let furniture = json!({}).as_object().unwrap().clone();
        let converted = convert_furniture(&furniture, &mut ctx, None, None);
        let variants = converted.definition.get("variants").and_then(Value::as_object).unwrap();
        assert_eq!(variants.keys().collect::<Vec<_>>(), vec!["ground", "ceiling", "wall"]);
        assert_eq!(converted.behavior.get("type"), Some(&json!("furniture_item")));
        assert!(converted.definition.get("loot").is_some());
        assert!(diagnostics.items.iter().any(|item| item.code == "FURNITURE_WALL_YAW_DIFFERENCE"));
    }

    #[test]
    fn nested_lookups_and_simple_self_loot() {
        let section = json!({ "place.volume": 2, "break": { "sound": "block.stone.break" } })
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(nested_number(&section, "place", "volume", 1.0), 2.0);
        assert_eq!(nested_number(&section, "place", "pitch", 0.8), 0.8);
        assert_eq!(nested_sound(&section, "break").as_deref(), Some("block.stone.break"));
        assert!(is_simple_self_loot(None, "TEST"));
        let drop = json!({ "loots": [{ "nexo_item": "TEST" }] }).as_object().unwrap().clone();
        assert!(is_simple_self_loot(Some(&drop), "TEST"));
        let complex = json!({ "loots": [{ "nexo_item": "TEST", "probability": 0.5 }] }).as_object().unwrap().clone();
        assert!(!is_simple_self_loot(Some(&complex), "TEST"));
    }
}
