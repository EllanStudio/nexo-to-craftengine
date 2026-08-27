//! Nexo builder-component conversion.
//!
//! Port of `legacy/src/component-builders.ts`. Builder components are the
//! Nexo 1.26 component dialect that maps onto CraftEngine 26.8 codec
//! components. Anything that requires Nexo's runtime registries (custom
//! blocks, custom items, unknown vanilla registry entries) is never guessed:
//! the component is reported as `manual` so the caller can emit a lossy
//! diagnostic instead.
//!
//! Key lookups mirror the TypeScript exactly: the `get_value` helper is
//! case-insensitive (mirroring `findValue`), while direct property access
//! in the TypeScript (`raw.foo`) stays case-sensitive here (`map.get`).

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use crate::data::block_states::block_state_properties;
use crate::data::consumables::consumable_components;
use crate::data::damage_types::{damage_type_tag_values, is_damage_type_id};
use crate::data::registries::{
    is_block_id, is_entity_type_id, is_item_id, is_jukebox_song_id, is_mob_effect_id,
    is_sound_event_id,
};
use crate::diagnostics::{Details, DiagnosticBag};
use crate::json::{get_value, JsonObject};
use crate::resource_location::normalize_location;

/// Outcome of converting one builder component. Mirrors the TS
/// `"converted" | "manual"` status union.
#[derive(Debug, Clone, PartialEq)]
pub enum BuilderStatus {
    Converted,
    Manual,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuilderComponentResult {
    pub status: BuilderStatus,
    pub value: Option<Value>,
    pub reason: Option<String>,
}

const BUILDER_KEYS: &[&str] = &[
    "can_place_on", "can_break", "tool", "jukebox_playable", "use_remainder",
    "death_protection", "consumable", "equippable", "repairable", "weapon",
    "blocks_attacks", "attack_range", "kinetic_weapon", "piercing_weapon",
    "swing_animation", "use_effects",
];

struct Context<'a> {
    diagnostics: &'a mut DiagnosticBag,
    source: &'a str,
    item: &'a str,
    key: &'a str,
    components: &'a JsonObject,
    material: &'a str,
}

fn converted(value: Option<Value>) -> BuilderComponentResult {
    BuilderComponentResult { status: BuilderStatus::Converted, value, reason: None }
}

fn manual(reason: &str) -> BuilderComponentResult {
    BuilderComponentResult {
        status: BuilderStatus::Manual,
        value: None,
        reason: Some(reason.to_string()),
    }
}

/// JS `String(number)` for the values a JSON document can carry.
fn js_number_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() };
    }
    // Integral values print without a fractional part.
    if value.fract() == 0.0 && value.abs() < 9223372036854775808.0 {
        return (value as i64).to_string();
    }
    // JS switches to exponential notation outside [1e-6, 1e21).
    if value.abs() >= 1e21 || value.abs() < 1e-6 {
        let scientific = format!("{:e}", value);
        if let Some((mantissa, exponent)) = scientific.split_once('e') {
            let signed = match exponent.strip_prefix('-') {
                Some(rest) => format!("-{}", rest),
                None => format!("+{}", exponent),
            };
            return format!("{}e{}", mantissa, signed);
        }
    }
    format!("{}", value)
}

/// Emit a TS number the way `JSON.stringify` would: integral values become
/// JSON integers, everything else stays a float.
fn number_value(value: f64) -> Value {
    if value.fract() == 0.0 && value >= -9223372036854775808.0 && value < 9223372036854775808.0 {
        Value::from(value as i64)
    } else {
        Value::from(value)
    }
}

/// The JS whitespace set used by `\s` and `Number(str)` trimming.
fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}'
            | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
            | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// Mirrors JS `Number(str)` for the shapes that can appear in configs:
/// trims JS whitespace, `""` is `0`, and leading/trailing dot forms that
/// Rust's parser rejects are normalized. Unparseable input yields `None`,
/// which behaves like `NaN` at every call site (all filter `is_finite`).
fn parse_js_number(text: &str) -> Option<f64> {
    let trimmed = text.trim_matches(is_js_whitespace);
    if trimmed.is_empty() {
        return Some(0.0);
    }
    let (sign, magnitude) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    if let Some(hex) = magnitude.strip_prefix("0x").or_else(|| magnitude.strip_prefix("0X")) {
        return i64::from_str_radix(hex, 16).ok().map(|value| sign * value as f64);
    }
    if let Some(octal) = magnitude.strip_prefix("0o").or_else(|| magnitude.strip_prefix("0O")) {
        return i64::from_str_radix(octal, 8).ok().map(|value| sign * value as f64);
    }
    if let Some(binary) = magnitude.strip_prefix("0b").or_else(|| magnitude.strip_prefix("0B")) {
        return i64::from_str_radix(binary, 2).ok().map(|value| sign * value as f64);
    }
    // Split off an exponent so mantissas like ".5" or "5." can be fixed.
    let (mantissa, exponent) = match magnitude.find(['e', 'E']) {
        Some(position) => magnitude.split_at(position),
        None => (magnitude, ""),
    };
    let mut normalized = mantissa.to_string();
    if normalized.starts_with('.') {
        normalized.insert(0, '0');
    }
    if normalized.ends_with('.') {
        normalized.push('0');
    }
    normalized.push_str(exponent);
    normalized.parse::<f64>().ok().map(|value| sign * value)
}

/// Mirrors TS `String(value)` for string/number/boolean scalars.
fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => number.as_f64().map(js_number_to_string),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Mirrors the TS `strings` helper: scalars become one-element lists and
/// arrays keep only string/number/boolean entries, dropping empty strings.
fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(entries)) => entries
            .iter()
            .filter_map(scalar_to_string)
            .filter(|text| !text.is_empty())
            .collect(),
        Some(value) => match scalar_to_string(value) {
            Some(text) if !text.is_empty() => vec![text],
            _ => Vec::new(),
        },
        None => Vec::new(),
    }
}

/// Mirrors the TS `sections` helper: arrays keep their object entries, an
/// object that uses one of the direct section keys is a single section, and
/// any other object is treated as a map of named sections.
fn sections(value: Option<&Value>, direct_keys: &[&str]) -> Vec<JsonObject> {
    match value {
        Some(Value::Array(entries)) => entries.iter().filter_map(Value::as_object).cloned().collect(),
        Some(Value::Object(map)) => {
            if direct_keys.iter().any(|key| map.contains_key(*key)) {
                vec![map.clone()]
            } else {
                map.values().filter_map(Value::as_object).cloned().collect()
            }
        }
        _ => Vec::new(),
    }
}

fn finite(value: Option<&Value>, fallback: f64) -> f64 {
    match value {
        Some(Value::Number(number)) => number.as_f64().filter(|value| value.is_finite()).unwrap_or(fallback),
        _ => fallback,
    }
}

fn integer(value: Option<&Value>, fallback: f64) -> f64 {
    finite(value, fallback).trunc()
}

fn boolean(value: Option<&Value>, fallback: bool) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        _ => fallback,
    }
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.min(max).max(min)
}

/// Mirrors TS `??`: falls through on missing or JSON null.
fn nullish<'a>(primary: Option<&'a Value>, fallback: Option<&'a Value>) -> Option<&'a Value> {
    match primary {
        None | Some(Value::Null) => fallback,
        Some(value) => Some(value),
    }
}

static DURATION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // Exact JS \s class; Rust's \s would add U+0085 and miss U+FEFF.
    let ws = "[\t\n\u{b}\u{c}\r\u{20}\u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]";
    Regex::new(&format!(
        r"(?i)^{ws}*(-?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)){ws}*(ms|ticks?|t|s|sec(?:onds?)?|m|min(?:utes?)?|h|hours?)?{ws}*$"
    ))
    .expect("duration pattern must compile")
});

fn duration_seconds(value: Option<&Value>, fallback: f64) -> f64 {
    match value {
        Some(Value::Number(number)) => match number.as_f64().filter(|value| value.is_finite()) {
            Some(seconds) => seconds.max(0.0),
            None => fallback,
        },
        Some(Value::String(text)) => {
            let captures = match DURATION_PATTERN.captures(text) {
                Some(captures) => captures,
                None => return fallback,
            };
            let amount = parse_js_number(&captures[1]).unwrap_or(fallback);
            let unit = captures
                .get(2)
                .map(|unit| unit.as_str().to_lowercase())
                .unwrap_or_else(|| "s".to_string());
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
        _ => fallback,
    }
}

fn duration_ticks(value: Option<&Value>, fallback: f64) -> f64 {
    (duration_seconds(value, fallback / 20.0) * 20.0).trunc().max(0.0)
}

/// Normalize a raw resource-location string the way the TS `resource`
/// helper does: trim, lowercase, then validate with the "minecraft" default
/// namespace and no extension stripping.
fn resource_text(text: &str, ctx: &mut Context, field: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let details = Details::new()
        .source(ctx.source)
        .item(ctx.item)
        .field(format!("Components.{}.{}", ctx.key, field));
    normalize_location(&trimmed.to_lowercase(), ctx.diagnostics, &details, &[], "minecraft")
}

fn resource(value: Option<&Value>, ctx: &mut Context, field: &str) -> Option<String> {
    match value {
        Some(Value::String(text)) => resource_text(text, ctx, field),
        _ => None,
    }
}

/// Sound holder encoding: known vanilla sound events stay bare ids, anything
/// else must be wrapped in `{ sound_id }` for the CraftEngine codec.
fn sound_holder(value: Option<&Value>, ctx: &mut Context, field: &str) -> Option<Value> {
    let id = resource(value, ctx, field)?;
    if is_sound_event_id(&id) {
        Some(Value::String(id))
    } else {
        Some(json!({ "sound_id": id }))
    }
}

fn tagged_resource(value: &str, ctx: &mut Context, field: &str, force_tag: bool) -> Option<String> {
    let tagged = force_tag || value.starts_with('#');
    let input = if tagged { value.strip_prefix('#').unwrap_or(value) } else { value };
    let id = resource_text(input, ctx, field)?;
    Some(if tagged { format!("#{}", id) } else { id })
}

/// Mirrors the TS `findValue` helper (case-insensitive member lookup).
fn find_value<'a>(object: &'a JsonObject, key: &str) -> Option<&'a Value> {
    get_value(object, key)
}

fn registry_id(value: &str) -> String {
    let plain = value.strip_prefix('#').unwrap_or(value).to_lowercase();
    if plain.contains(':') {
        plain
    } else {
        format!("minecraft:{}", plain)
    }
}

fn is_known_item_id(value: &str) -> bool {
    is_item_id(&registry_id(value))
}

fn is_known_block_id(value: &str) -> bool {
    is_block_id(&registry_id(value))
}

fn string_or_list(values: &[String]) -> Value {
    if values.len() == 1 {
        Value::String(values[0].clone())
    } else {
        Value::Array(values.iter().map(|value| Value::String(value.clone())).collect())
    }
}

fn convert_block_predicates(raw: &Value, ctx: &mut Context) -> BuilderComponentResult {
    let entries = sections(Some(raw), &["block", "blocks", "nexo_block", "state"]);
    if entries.is_empty() {
        return manual("需要至少一个 block predicate section");
    }
    let mut predicates: Vec<JsonObject> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if !strings(entry.get("nexo_block")).is_empty() {
            return manual("nexo_block 需要解析运行时自定义方块状态");
        }
        let state_source = entry.get("state").and_then(Value::as_object);
        let mut direct: Vec<String> = Vec::new();
        let mut tags: Vec<String> = Vec::new();
        for block in strings(nullish(entry.get("block"), entry.get("blocks"))) {
            // Unknown block ids are treated as block tags: Nexo resolves them
            // against runtime registries, which the static converter cannot.
            let force_tag = block.starts_with('#') || !is_known_block_id(&block);
            let normalized = match tagged_resource(&block, ctx, &format!("predicates[{}].block", index), force_tag) {
                Some(normalized) => normalized,
                None => return manual("存在无效方块或方块标签 ID"),
            };
            if normalized.starts_with('#') {
                tags.push(normalized);
            } else {
                direct.push(normalized);
            }
        }
        if !direct.is_empty() {
            let mut predicate = JsonObject::new();
            predicate.insert("blocks".to_string(), string_or_list(&direct));
            if let Some(state_source) = state_source {
                let allowed: std::collections::HashSet<&str> = block_state_properties(&direct[0])
                    .map(|properties| properties.iter().map(String::as_str).collect())
                    .unwrap_or_default();
                let mut state = JsonObject::new();
                for (name, value) in state_source {
                    if !allowed.contains(name.as_str()) {
                        ctx.diagnostics.warning(
                            "COMPONENT_BLOCK_STATE_PROPERTY_IGNORED",
                            &format!("Nexo ignores unknown block-state property {} for {}", name, direct[0]),
                            Details::new()
                                .source(ctx.source)
                                .item(ctx.item)
                                .field(format!("Components.{}.predicates[{}].state.{}", ctx.key, index, name)),
                        );
                        continue;
                    }
                    let text = match scalar_to_string(value) {
                        Some(text) => text,
                        None => return manual("state 中包含无法静态编码的非标量属性值"),
                    };
                    state.insert(name.clone(), Value::String(text));
                }
                if !state.is_empty() {
                    predicate.insert("state".to_string(), Value::Object(state));
                }
            }
            predicates.push(predicate);
        }
        for tag in &tags {
            let mut predicate = JsonObject::new();
            predicate.insert("blocks".to_string(), Value::String(tag.clone()));
            predicates.push(predicate);
        }
    }
    if predicates.is_empty() {
        return manual("没有可编码的原版方块或标签");
    }
    if predicates.len() == 1 {
        converted(Some(Value::Object(predicates.remove(0))))
    } else {
        converted(Some(Value::Array(predicates.into_iter().map(Value::Object).collect())))
    }
}

fn convert_tool(raw: &Value, ctx: &mut Context) -> BuilderComponentResult {
    let raw = match raw.as_object() {
        Some(raw) => raw,
        None => return manual("tool 必须是 section"),
    };
    let mut result = JsonObject::new();
    result.insert("rules".to_string(), json!([]));
    result.insert("can_destroy_blocks_in_creative".to_string(), Value::Bool(false));
    let mining_speed = finite(raw.get("default_mining_speed"), 1.0).max(0.0);
    let damage = integer(raw.get("damage_per_block"), 1.0).max(0.0);
    if mining_speed != 1.0 {
        result.insert("default_mining_speed".to_string(), number_value(mining_speed));
    }
    if damage != 1.0 {
        result.insert("damage_per_block".to_string(), number_value(damage));
    }
    let mut output_rules: Vec<JsonObject> = Vec::new();
    let rule_sections = sections(raw.get("rules"), &["material", "materials", "tag", "tags", "speed", "correct_for_drops"]);
    for (index, rule) in rule_sections.iter().enumerate() {
        let speed = finite(rule.get("speed"), 1.0);
        if speed <= 0.0 {
            return manual("tool rule speed 必须大于 0 才能通过 1.21.11 codec");
        }
        let correct = boolean(rule.get("correct_for_drops"), false);
        let mut materials: Vec<String> = Vec::new();
        for material in strings(nullish(rule.get("material"), rule.get("materials"))) {
            if !is_known_block_id(&material) {
                ctx.diagnostics.warning(
                    "COMPONENT_TOOL_BLOCK_INVALID",
                    &format!("Nexo ignores a tool material that is not a Minecraft 1.21.11 block: {}", material),
                    Details::new()
                        .source(ctx.source)
                        .item(ctx.item)
                        .field(format!("Components.tool.rules[{}].materials", index)),
                );
                continue;
            }
            if let Some(normalized) = resource_text(&material, ctx, &format!("rules[{}].materials", index)) {
                materials.push(normalized);
            }
        }
        if !materials.is_empty() {
            let mut output = JsonObject::new();
            output.insert("blocks".to_string(), string_or_list(&materials));
            output.insert("speed".to_string(), number_value(speed));
            output.insert("correct_for_drops".to_string(), Value::Bool(correct));
            output_rules.push(output);
        }
        for tag in strings(nullish(rule.get("tag"), rule.get("tags"))) {
            if let Some(normalized) = tagged_resource(&tag, ctx, &format!("rules[{}].tags", index), true) {
                let mut output = JsonObject::new();
                output.insert("blocks".to_string(), Value::String(normalized));
                output.insert("speed".to_string(), number_value(speed));
                output.insert("correct_for_drops".to_string(), Value::Bool(correct));
                output_rules.push(output);
            }
        }
    }
    result.insert("rules".to_string(), Value::Array(output_rules.into_iter().map(Value::Object).collect()));
    converted(Some(Value::Object(result)))
}

fn convert_jukebox(raw: &Value, ctx: &mut Context) -> BuilderComponentResult {
    let song_value = match raw.as_object() {
        Some(map) => nullish(map.get("song"), map.get("song_key")),
        None => Some(raw),
    };
    let song = match resource(song_value, ctx, "song") {
        Some(song) => song,
        None => return manual("jukebox_playable 缺少可编码的 song key"),
    };
    if song.starts_with("minecraft:") && !is_jukebox_song_id(&song) {
        return manual("未知的 vanilla jukebox song 需要运行时 registry");
    }
    converted(Some(Value::String(song)))
}

fn convert_use_remainder(raw: &Value, ctx: &mut Context) -> BuilderComponentResult {
    if let Value::String(text) = raw {
        if !is_known_item_id(text) {
            return manual("use_remainder 的 minecraft_type 不是 1.21.11 item registry entry");
        }
        return match resource_text(text, ctx, "minecraft_type") {
            Some(id) => converted(Some(json!({ "id": id, "count": 1 }))),
            None => manual("use_remainder 的物品 ID 无效"),
        };
    }
    let raw = match raw.as_object() {
        Some(raw) => raw,
        None => return manual("use_remainder 必须是 section"),
    };
    if raw.contains_key("nexo_item")
        || raw.contains_key("crucible_item")
        || raw.contains_key("mmoitems_id")
        || raw.contains_key("mmoitems_type")
        || raw.contains_key("minecraft_item")
    {
        return manual("自定义或序列化 ItemStack 余留物需要运行时物品注册表");
    }
    let minecraft_type = match raw.get("minecraft_type") {
        Some(Value::String(text)) => text,
        _ => return manual("仅有效的 Minecraft 1.21.11 minecraft_type 余留物可安全静态转换"),
    };
    if !is_known_item_id(minecraft_type) {
        return manual("仅有效的 Minecraft 1.21.11 minecraft_type 余留物可安全静态转换");
    }
    let id = match resource_text(minecraft_type, ctx, "minecraft_type") {
        Some(id) => id,
        None => return manual("仅 minecraft_type 余留物可安全静态转换"),
    };
    let count = clamp(integer(raw.get("amount"), 1.0), 1.0, 99.0);
    converted(Some(json!({ "id": id, "count": number_value(count) })))
}

struct EffectsResult {
    effects: Vec<JsonObject>,
    unknown: Vec<String>,
}

fn convert_effects(raw: Option<&Value>, ctx: &mut Context, field: &str) -> EffectsResult {
    let raw = match raw.and_then(Value::as_object) {
        Some(raw) => raw,
        None => return EffectsResult { effects: Vec::new(), unknown: Vec::new() },
    };
    let mut output: Vec<JsonObject> = Vec::new();
    const KNOWN: &[&str] = &["apply_effects", "remove_effects", "clear_all_effects", "teleport_randomly", "play_sound"];
    let mut unknown: Vec<String> = raw
        .keys()
        .filter(|key| !KNOWN.contains(&key.to_lowercase().as_str()))
        .cloned()
        .collect();
    if let Some(apply_raw) = find_value(raw, "apply_effects").and_then(Value::as_object) {
        for (effect_name, effect_value) in apply_raw {
            let effect = match effect_value.as_object() {
                Some(effect) => effect,
                None => continue,
            };
            let id = match resource_text(effect_name, ctx, &format!("{}.APPLY_EFFECTS.{}", field, effect_name)) {
                Some(id) => id,
                None => continue,
            };
            if !is_mob_effect_id(&id) {
                ctx.diagnostics.warning(
                    "COMPONENT_EFFECT_UNKNOWN_IGNORED",
                    &format!("Nexo ignores unknown mob effect {}", id),
                    Details::new()
                        .source(ctx.source)
                        .item(ctx.item)
                        .field(format!("Components.{}.{}.APPLY_EFFECTS.{}", ctx.key, field, effect_name)),
                );
                continue;
            }
            let mut instance = JsonObject::new();
            instance.insert("id".to_string(), Value::String(id));
            instance.insert("amplifier".to_string(), number_value(integer(effect.get("amplifier"), 0.0)));
            instance.insert("duration".to_string(), number_value(duration_ticks(effect.get("duration"), 0.0)));
            instance.insert("ambient".to_string(), Value::Bool(boolean(effect.get("ambient"), true)));
            instance.insert("show_particles".to_string(), Value::Bool(boolean(effect.get("show_particles"), true)));
            instance.insert("show_icon".to_string(), Value::Bool(boolean(effect.get("show_icon"), true)));
            let probability = clamp(finite(effect.get("probability"), 1.0), 0.0, 1.0);
            let mut entry = JsonObject::new();
            entry.insert("type".to_string(), Value::String("minecraft:apply_effects".to_string()));
            entry.insert("effects".to_string(), Value::Array(vec![Value::Object(instance)]));
            entry.insert("probability".to_string(), number_value(probability));
            output.push(entry);
        }
    }
    let mut removed: Vec<String> = Vec::new();
    let remove_list = match find_value(raw, "remove_effects") {
        Some(value @ Value::Array(_)) => Some(value),
        _ => None,
    };
    for effect_name in strings(remove_list) {
        match resource_text(&effect_name, ctx, &format!("{}.REMOVE_EFFECTS", field)) {
            Some(id) if is_mob_effect_id(&id) => removed.push(id),
            Some(id) => {
                ctx.diagnostics.warning(
                    "COMPONENT_EFFECT_UNKNOWN_IGNORED",
                    &format!("Nexo ignores unknown mob effect {}", id),
                    Details::new()
                        .source(ctx.source)
                        .item(ctx.item)
                        .field(format!("Components.{}.{}.REMOVE_EFFECTS", ctx.key, field)),
                );
            }
            None => {}
        }
    }
    if !removed.is_empty() {
        let mut entry = JsonObject::new();
        entry.insert("type".to_string(), Value::String("minecraft:remove_effects".to_string()));
        entry.insert("effects".to_string(), string_or_list(&removed));
        output.push(entry);
    }
    if find_value(raw, "clear_all_effects").is_some() {
        let mut entry = JsonObject::new();
        entry.insert("type".to_string(), Value::String("minecraft:clear_all_effects".to_string()));
        output.push(entry);
    }
    if let Some(teleport_raw) = find_value(raw, "teleport_randomly").and_then(Value::as_object) {
        let diameter = finite(teleport_raw.get("diameter"), 16.0);
        if diameter <= 0.0 {
            unknown.push("TELEPORT_RANDOMLY.diameter 必须大于 0".to_string());
        } else {
            let mut entry = JsonObject::new();
            entry.insert("type".to_string(), Value::String("minecraft:teleport_randomly".to_string()));
            entry.insert("diameter".to_string(), number_value(diameter));
            output.push(entry);
        }
    }
    if let Some(sound_raw) = find_value(raw, "play_sound").and_then(Value::as_object) {
        let sound_id = resource(
            nullish(sound_raw.get("sound"), sound_raw.get("sound_id")),
            ctx,
            &format!("{}.PLAY_SOUND.sound", field),
        );
        match sound_id {
            Some(id) if is_sound_event_id(&id) => {
                let mut entry = JsonObject::new();
                entry.insert("type".to_string(), Value::String("minecraft:play_sound".to_string()));
                entry.insert("sound".to_string(), Value::String(id));
                output.push(entry);
            }
            Some(id) => {
                ctx.diagnostics.warning(
                    "COMPONENT_SOUND_UNKNOWN_IGNORED",
                    &format!("Nexo ignores unknown sound event {}", id),
                    Details::new()
                        .source(ctx.source)
                        .item(ctx.item)
                        .field(format!("Components.{}.{}.PLAY_SOUND.sound", ctx.key, field)),
                );
            }
            None => {}
        }
    }
    EffectsResult { effects: output, unknown }
}

fn convert_death_protection(raw: &Value, ctx: &mut Context) -> BuilderComponentResult {
    let raw = match raw.as_object() {
        Some(raw) => raw,
        None => return manual("death_protection 必须是 section"),
    };
    let converted_effects = convert_effects(raw.get("death_effects"), ctx, "death_effects");
    if !converted_effects.unknown.is_empty() {
        return manual(&format!("包含未知 death effect: {}", converted_effects.unknown.join(", ")));
    }
    let mut result = JsonObject::new();
    result.insert(
        "death_effects".to_string(),
        Value::Array(converted_effects.effects.into_iter().map(Value::Object).collect()),
    );
    converted(Some(Value::Object(result)))
}

fn convert_consumable(raw: &Value, ctx: &mut Context) -> BuilderComponentResult {
    let raw = match raw.as_object() {
        Some(raw) => raw,
        None => return manual("consumable 必须是 section"),
    };
    // Nexo inherits the official consumable baseline of the backing item.
    let mut result: JsonObject = consumable_components(&registry_id(ctx.material))
        .cloned()
        .unwrap_or_default();
    if raw.contains_key("consume_duration") || raw.contains_key("consume_seconds") {
        let seconds = if raw.contains_key("consume_duration") {
            duration_seconds(raw.get("consume_duration"), 1.6)
        } else {
            finite(raw.get("consume_seconds"), 1.6).max(0.0)
        };
        result.insert("consume_seconds".to_string(), number_value(seconds));
    }
    if raw.contains_key("animation") {
        let animation_text = match raw.get("animation") {
            Some(Value::String(text)) => text.to_lowercase(),
            _ => "eat".to_string(),
        };
        const ANIMATIONS: &[&str] = &[
            "none", "eat", "drink", "block", "bow", "spear", "crossbow", "spyglass",
            "toot_horn", "brush", "bundle",
        ];
        let animation = if ANIMATIONS.contains(&animation_text.as_str()) {
            animation_text
        } else {
            "eat".to_string()
        };
        result.insert("animation".to_string(), Value::String(animation));
    }
    if raw.contains_key("consume_particles") || raw.contains_key("has_consume_particles") {
        let flag = boolean(nullish(raw.get("consume_particles"), raw.get("has_consume_particles")), true);
        result.insert("has_consume_particles".to_string(), Value::Bool(flag));
    }
    if raw.contains_key("sound") {
        if let Some(sound) = sound_holder(raw.get("sound"), ctx, "sound") {
            result.insert("sound".to_string(), sound);
        }
    }
    if raw.contains_key("effects") || raw.contains_key("on_consume_effects") {
        let converted_effects = convert_effects(nullish(raw.get("effects"), raw.get("on_consume_effects")), ctx, "effects");
        if !converted_effects.unknown.is_empty() {
            return manual(&format!("包含未知 consume effect: {}", converted_effects.unknown.join(", ")));
        }
        if !converted_effects.effects.is_empty() {
            result.insert(
                "on_consume_effects".to_string(),
                Value::Array(converted_effects.effects.into_iter().map(Value::Object).collect()),
            );
        } else {
            result.remove("on_consume_effects");
        }
    }
    converted(Some(Value::Object(result)))
}

fn convert_equippable(raw: &Value, ctx: &mut Context) -> BuilderComponentResult {
    let raw = match raw.as_object() {
        Some(raw) => raw,
        None => return manual("equippable 必须是 section"),
    };
    let source_slot = match raw.get("slot") {
        Some(Value::String(slot)) => slot.to_lowercase(),
        _ => return manual("equippable.slot 缺失"),
    };
    let slot = match source_slot.as_str() {
        "hand" => "mainhand".to_string(),
        "off_hand" => "offhand".to_string(),
        _ => source_slot,
    };
    const SLOTS: &[&str] = &["mainhand", "offhand", "feet", "legs", "chest", "head", "body", "saddle"];
    if !SLOTS.contains(&slot.as_str()) {
        return manual("equippable.slot 不是 Minecraft 1.21.11 的有效槽位");
    }
    let mut result = JsonObject::new();
    result.insert("slot".to_string(), Value::String(slot));
    let mut allowed: Vec<String> = Vec::new();
    for entity in strings(nullish(raw.get("allowed_entity_types"), raw.get("allowed_entity_type"))) {
        if let Some(id) = resource_text(&entity, ctx, "allowed_entity_types") {
            if !is_entity_type_id(&id) {
                return manual(&format!("allowed_entity_types 包含未知的 1.21.11 entity type: {}", id));
            }
            allowed.push(id);
        }
    }
    if !allowed.is_empty() {
        result.insert("allowed_entities".to_string(), string_or_list(&allowed));
    }
    if let Some(asset) = resource(raw.get("asset_id"), ctx, "asset_id") {
        result.insert("asset_id".to_string(), Value::String(asset));
    }
    if let Some(overlay) = resource(raw.get("camera_overlay"), ctx, "camera_overlay") {
        result.insert("camera_overlay".to_string(), Value::String(overlay));
    }
    let glider_like = boolean(ctx.components.get("glider"), ctx.material != "elytra");
    let default_equip_sound = if glider_like {
        "minecraft:item.armor.equip_elytra"
    } else {
        "minecraft:item.armor.equip_generic"
    };
    let equip_sound = sound_holder(raw.get("equip_sound"), ctx, "equip_sound")
        .unwrap_or_else(|| Value::String(default_equip_sound.to_string()));
    if equip_sound != Value::String("minecraft:item.armor.equip_generic".to_string()) {
        result.insert("equip_sound".to_string(), equip_sound);
    }
    if let Some(shear_sound) = sound_holder(nullish(raw.get("shear_sound"), raw.get("shearing_sound")), ctx, "shear_sound") {
        if shear_sound != Value::String("minecraft:item.shears.snip".to_string()) {
            result.insert("shearing_sound".to_string(), shear_sound);
        }
    }
    let flag_defaults: &[(&str, &str, bool, bool)] = &[
        ("dispensable", "dispensable", true, true),
        ("swappable", "swappable", true, true),
        ("damage_on_hurt", "damage_on_hurt", glider_like, true),
        ("equip_on_interact", "equip_on_interact", false, false),
        ("can_be_sheared", "can_be_sheared", ctx.item.contains("harness"), false),
    ];
    for (source_key, target_key, nexo_default, codec_default) in flag_defaults {
        let value = boolean(raw.get(*source_key), *nexo_default);
        if value != *codec_default {
            result.insert(target_key.to_string(), Value::Bool(value));
        }
    }
    converted(Some(Value::Object(result)))
}

fn convert_repairable(raw: &Value, ctx: &mut Context) -> BuilderComponentResult {
    let mut direct: Vec<String> = Vec::new();
    let mut tags: Vec<String> = Vec::new();
    for entry in strings(Some(raw)) {
        let force_tag = entry.starts_with('#') || !is_known_item_id(&entry);
        if let Some(id) = tagged_resource(&entry, ctx, "items", force_tag) {
            if id.starts_with('#') {
                tags.push(id);
            } else {
                direct.push(id);
            }
        }
    }
    if direct.is_empty() && tags.is_empty() {
        return manual("repairable 没有可解析的原版物品或标签");
    }
    if tags.len() > 1 || (!tags.is_empty() && !direct.is_empty()) {
        return manual("多个标签或标签与物品混合需要展开运行时 item registry");
    }
    let items = if !tags.is_empty() {
        Value::String(tags[0].clone())
    } else {
        string_or_list(&direct)
    };
    let mut result = JsonObject::new();
    result.insert("items".to_string(), items);
    converted(Some(Value::Object(result)))
}

fn convert_weapon(raw: &Value) -> BuilderComponentResult {
    let raw = match raw.as_object() {
        Some(raw) => raw,
        None => return manual("weapon 必须是 section"),
    };
    let mut result = JsonObject::new();
    let damage = integer(nullish(raw.get("damage_per_attack"), raw.get("item_damage_per_attack")), 1.0).max(0.0);
    let disable = duration_seconds(nullish(raw.get("disable_blocking"), raw.get("disable_blocking_for_seconds")), 0.0);
    if damage != 1.0 {
        result.insert("item_damage_per_attack".to_string(), number_value(damage));
    }
    if disable != 0.0 {
        result.insert("disable_blocking_for_seconds".to_string(), number_value(disable));
    }
    converted(Some(Value::Object(result)))
}

fn convert_blocks_attacks(raw: &Value, ctx: &mut Context) -> BuilderComponentResult {
    let raw = match raw.as_object() {
        Some(raw) => raw,
        None => return manual("blocks_attacks 必须是 section"),
    };
    let mut result = JsonObject::new();
    let delay = duration_seconds(nullish(raw.get("block_delay"), raw.get("block_delay_seconds")), 0.0);
    let cooldown = finite(raw.get("disable_cooldown_scale"), 1.0).max(0.0);
    if delay != 0.0 {
        result.insert("block_delay_seconds".to_string(), number_value(delay));
    }
    if cooldown != 1.0 {
        result.insert("disable_cooldown_scale".to_string(), number_value(cooldown));
    }
    if let Some(block_sound) = sound_holder(raw.get("block_sound"), ctx, "block_sound") {
        result.insert("block_sound".to_string(), block_sound);
    }
    if let Some(disable_sound) = sound_holder(nullish(raw.get("disable_sound"), raw.get("disabled_sound")), ctx, "disable_sound") {
        result.insert("disabled_sound".to_string(), disable_sound);
    }
    if let Some(Value::String(bypassed_by)) = raw.get("bypassed_by") {
        if let Some(bypassed) = tagged_resource(bypassed_by, ctx, "bypassed_by", true) {
            result.insert("bypassed_by".to_string(), Value::String(bypassed));
        }
    }
    if let Some(item_damage) = raw.get("item_damage").and_then(Value::as_object) {
        let threshold = finite(item_damage.get("threshold"), 0.0).max(0.0);
        let base = finite(item_damage.get("base"), 1.0);
        let factor = finite(item_damage.get("factor"), 1.0);
        let mut damage_object = JsonObject::new();
        damage_object.insert("threshold".to_string(), number_value(threshold));
        damage_object.insert("base".to_string(), number_value(base));
        damage_object.insert("factor".to_string(), number_value(factor));
        result.insert("item_damage".to_string(), Value::Object(damage_object));
    }
    let mut reductions: Vec<JsonObject> = Vec::new();
    let reduction_sections = sections(raw.get("damage_reductions"), &["base", "factor", "horizontal_blocking", "type", "types"]);
    for (index, reduction) in reduction_sections.iter().enumerate() {
        let mut encoded = JsonObject::new();
        encoded.insert("base".to_string(), number_value(finite(reduction.get("base"), 1.0)));
        encoded.insert("factor".to_string(), number_value(finite(reduction.get("factor"), 1.0)));
        let angle = finite(
            nullish(reduction.get("horizontal_blocking"), reduction.get("horizontal_blocking_angle")),
            90.0,
        )
        .max(0.0);
        if angle <= 0.0 {
            return manual("damage reduction horizontal_blocking 必须大于 0 才能通过 1.21.11 codec");
        }
        if angle != 90.0 {
            encoded.insert("horizontal_blocking_angle".to_string(), number_value(angle));
        }
        let mut direct_types: Vec<String> = Vec::new();
        let mut type_tags: Vec<String> = Vec::new();
        for damage_type in strings(nullish(reduction.get("type"), reduction.get("types"))) {
            let id = match resource_text(damage_type.strip_prefix('#').unwrap_or(&damage_type), ctx, &format!("damage_reductions[{}].type", index)) {
                Some(id) => id,
                None => continue,
            };
            if damage_type.starts_with('#') || damage_type_tag_values(&id).is_some() {
                type_tags.push(format!("#{}", id));
            } else if is_damage_type_id(&id) {
                direct_types.push(id);
            } else {
                return manual(&format!("damage_reductions.type 包含未知的运行时 damage type: {}", id));
            }
        }
        if type_tags.len() > 1 || (!type_tags.is_empty() && !direct_types.is_empty()) {
            return manual("多个 damage type 标签或标签与具体类型混合需要展开运行时 registry");
        }
        if type_tags.len() == 1 {
            encoded.insert("type".to_string(), Value::String(type_tags[0].clone()));
        } else if !direct_types.is_empty() {
            encoded.insert("type".to_string(), string_or_list(&direct_types));
        }
        reductions.push(encoded);
    }
    if !reductions.is_empty() {
        result.insert("damage_reductions".to_string(), Value::Array(reductions.into_iter().map(Value::Object).collect()));
    }
    converted(Some(Value::Object(result)))
}

fn parse_range(
    raw: &JsonObject,
    min_key: &str,
    max_key: &str,
    combined_key: &str,
    fallback_min: f64,
    fallback_max: f64,
) -> (f64, f64) {
    let mut min = fallback_min;
    let mut max = fallback_max;
    match raw.get(combined_key) {
        Some(Value::Number(number)) => {
            if let Some(value) = number.as_f64().filter(|value| value.is_finite()) {
                max = value;
            }
        }
        Some(Value::String(combined)) => {
            let parts: Vec<&str> = combined.split("..").collect();
            let parsed_combined = parse_js_number(combined).filter(|value| value.is_finite());
            if parts.len() == 2 {
                let low = parse_js_number(parts[0]).filter(|value| value.is_finite());
                let high = parse_js_number(parts[1]).filter(|value| value.is_finite());
                if let (Some(low), Some(high)) = (low, high) {
                    min = low;
                    max = high;
                } else if let Some(value) = parsed_combined {
                    max = value;
                }
            } else if let Some(value) = parsed_combined {
                max = value;
            }
        }
        _ => {}
    }
    if let Some(Value::Number(number)) = raw.get(min_key) {
        if let Some(value) = number.as_f64().filter(|value| value.is_finite()) {
            min = value;
        }
    }
    if let Some(Value::Number(number)) = raw.get(max_key) {
        if let Some(value) = number.as_f64().filter(|value| value.is_finite()) {
            max = value;
        }
    }
    (clamp(min, 0.0, 64.0), clamp(max, 0.0, 64.0))
}

fn convert_attack_range(raw: &Value) -> BuilderComponentResult {
    let raw = match raw.as_object() {
        Some(raw) => raw,
        None => return manual("attack_range 必须是 section"),
    };
    let (min_reach, max_reach) = parse_range(raw, "min_reach", "max_reach", "reach", 0.0, 3.0);
    let (min_creative, max_creative) = parse_range(raw, "min_creative_reach", "max_creative_reach", "creative_reach", 0.0, 5.0);
    let mut result = JsonObject::new();
    if min_reach != 0.0 {
        result.insert("min_reach".to_string(), number_value(min_reach));
    }
    if max_reach != 3.0 {
        result.insert("max_reach".to_string(), number_value(max_reach));
    }
    if min_creative != 0.0 {
        result.insert("min_creative_reach".to_string(), number_value(min_creative));
    }
    if max_creative != 5.0 {
        result.insert("max_creative_reach".to_string(), number_value(max_creative));
    }
    let margin = clamp(finite(raw.get("hitbox_margin"), 0.3), 0.0, 1.0);
    let factor = clamp(finite(raw.get("mob_factor"), 1.0), 0.0, 2.0);
    if margin != 0.3 {
        result.insert("hitbox_margin".to_string(), number_value(margin));
    }
    if factor != 1.0 {
        result.insert("mob_factor".to_string(), number_value(factor));
    }
    converted(Some(Value::Object(result)))
}

fn kinetic_condition(raw: Option<&Value>) -> Option<JsonObject> {
    let raw = raw.and_then(Value::as_object)?;
    let duration = duration_ticks(nullish(raw.get("max_duration"), raw.get("max_duration_ticks")), 0.0);
    let mut result = JsonObject::new();
    result.insert("max_duration_ticks".to_string(), number_value(duration));
    let speed = finite(raw.get("min_speed"), 0.0);
    let relative = finite(raw.get("min_relative_speed"), 0.0);
    if speed != 0.0 {
        result.insert("min_speed".to_string(), number_value(speed));
    }
    if relative != 0.0 {
        result.insert("min_relative_speed".to_string(), number_value(relative));
    }
    Some(result)
}

fn convert_kinetic(raw: &Value, ctx: &mut Context) -> BuilderComponentResult {
    let raw = match raw.as_object() {
        Some(raw) => raw,
        None => return manual("kinetic_weapon 必须是 section"),
    };
    let mut result = JsonObject::new();
    let contact = duration_ticks(nullish(raw.get("contact_cooldown"), raw.get("contact_cooldown_ticks")), 10.0);
    let delay = duration_ticks(nullish(raw.get("delay"), raw.get("delay_ticks")), 0.0);
    let movement = finite(raw.get("forward_movement"), 0.0);
    let multiplier = finite(raw.get("damage_multiplier"), 1.0);
    if contact != 10.0 {
        result.insert("contact_cooldown_ticks".to_string(), number_value(contact));
    }
    if delay != 0.0 {
        result.insert("delay_ticks".to_string(), number_value(delay));
    }
    if movement != 0.0 {
        result.insert("forward_movement".to_string(), number_value(movement));
    }
    if multiplier != 1.0 {
        result.insert("damage_multiplier".to_string(), number_value(multiplier));
    }
    if let Some(sound) = sound_holder(raw.get("sound"), ctx, "sound") {
        result.insert("sound".to_string(), sound);
    }
    if let Some(hit_sound) = sound_holder(raw.get("hit_sound"), ctx, "hit_sound") {
        result.insert("hit_sound".to_string(), hit_sound);
    }
    for key in ["dismount_conditions", "knockback_conditions", "damage_conditions"] {
        if let Some(condition) = kinetic_condition(raw.get(key)) {
            result.insert(key.to_string(), Value::Object(condition));
        }
    }
    converted(Some(Value::Object(result)))
}

fn convert_piercing(raw: &Value, ctx: &mut Context) -> BuilderComponentResult {
    let raw = match raw.as_object() {
        Some(raw) => raw,
        None => return manual("piercing_weapon 必须是 section"),
    };
    let mut result = JsonObject::new();
    if !boolean(raw.get("deals_knockback"), true) {
        result.insert("deals_knockback".to_string(), Value::Bool(false));
    }
    if boolean(raw.get("dismounts"), false) {
        result.insert("dismounts".to_string(), Value::Bool(true));
    }
    if let Some(sound) = sound_holder(raw.get("sound"), ctx, "sound") {
        result.insert("sound".to_string(), sound);
    }
    if let Some(hit_sound) = sound_holder(raw.get("hit_sound"), ctx, "hit_sound") {
        result.insert("hit_sound".to_string(), hit_sound);
    }
    converted(Some(Value::Object(result)))
}

fn convert_swing(raw: &Value) -> BuilderComponentResult {
    let raw = match raw.as_object() {
        Some(raw) => raw,
        None => return manual("swing_animation 必须是 section"),
    };
    let mut result = JsonObject::new();
    let type_text = match raw.get("type") {
        Some(Value::String(text)) => text.to_lowercase(),
        _ => "whack".to_string(),
    };
    let swing_type = if matches!(type_text.as_str(), "none" | "whack" | "stab") {
        type_text
    } else {
        "whack".to_string()
    };
    let duration = duration_ticks(raw.get("duration"), 6.0);
    if duration <= 0.0 {
        return manual("swing_animation.duration 必须至少为 1 tick");
    }
    if swing_type != "whack" {
        result.insert("type".to_string(), Value::String(swing_type));
    }
    if duration != 6.0 {
        result.insert("duration".to_string(), number_value(duration));
    }
    converted(Some(Value::Object(result)))
}

fn convert_use_effects(raw: &Value) -> BuilderComponentResult {
    let raw = match raw.as_object() {
        Some(raw) => raw,
        None => return manual("use_effects 必须是 section"),
    };
    let mut result = JsonObject::new();
    let sprint = boolean(raw.get("can_sprint"), false);
    let vibrations = boolean(raw.get("interact_vibrations"), true);
    let speed = clamp(finite(raw.get("speed_multiplier"), 0.2), 0.0, 1.0);
    if sprint {
        result.insert("can_sprint".to_string(), Value::Bool(true));
    }
    if !vibrations {
        result.insert("interact_vibrations".to_string(), Value::Bool(false));
    }
    if speed != 0.2 {
        result.insert("speed_multiplier".to_string(), number_value(speed));
    }
    converted(Some(Value::Object(result)))
}

pub fn convert_nexo_builder_component(
    key: &str,
    raw: &Value,
    diagnostics: &mut DiagnosticBag,
    source: &str,
    item: &str,
    components: &JsonObject,
    material: &str,
) -> Option<BuilderComponentResult> {
    if !BUILDER_KEYS.contains(&key) {
        return None;
    }
    let mut context = Context { diagnostics, source, item, key, components, material };
    Some(match key {
        "can_place_on" | "can_break" => convert_block_predicates(raw, &mut context),
        "tool" => convert_tool(raw, &mut context),
        "jukebox_playable" => convert_jukebox(raw, &mut context),
        "use_remainder" => convert_use_remainder(raw, &mut context),
        "death_protection" => convert_death_protection(raw, &mut context),
        "consumable" => convert_consumable(raw, &mut context),
        "equippable" => convert_equippable(raw, &mut context),
        "repairable" => convert_repairable(raw, &mut context),
        "weapon" => convert_weapon(raw),
        "blocks_attacks" => convert_blocks_attacks(raw, &mut context),
        "attack_range" => convert_attack_range(raw),
        "kinetic_weapon" => convert_kinetic(raw, &mut context),
        "piercing_weapon" => convert_piercing(raw, &mut context),
        "swing_animation" => convert_swing(raw),
        "use_effects" => convert_use_effects(raw),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn convert_with(
        key: &str,
        raw: Value,
        components: JsonObject,
        material: &str,
        item: &str,
    ) -> (Option<BuilderComponentResult>, DiagnosticBag) {
        let mut diagnostics = DiagnosticBag::new();
        let result =
            convert_nexo_builder_component(key, &raw, &mut diagnostics, "items.yml", item, &components, material);
        (result, diagnostics)
    }

    fn convert(key: &str, raw: Value) -> (Option<BuilderComponentResult>, DiagnosticBag) {
        convert_with(key, raw, JsonObject::new(), "paper", "test_item")
    }

    fn converted_value(result: Option<BuilderComponentResult>) -> Value {
        let result = result.expect("builder key should be handled");
        assert_eq!(result.status, BuilderStatus::Converted, "expected converted status");
        result.value.expect("converted result should carry a value")
    }

    fn manual_reason(result: Option<BuilderComponentResult>) -> String {
        let result = result.expect("builder key should be handled");
        assert_eq!(result.status, BuilderStatus::Manual, "expected manual status");
        result.reason.expect("manual result should carry a reason")
    }

    #[test]
    fn unknown_key_is_not_handled() {
        let (result, diagnostics) = convert("custom_data", json!({}));
        assert!(result.is_none());
        assert_eq!(diagnostics.items.len(), 0);
    }

    #[test]
    fn block_predicate_single_block() {
        let (result, diagnostics) = convert("can_place_on", json!({ "block": "stone" }));
        assert_eq!(converted_value(result), json!({ "blocks": "minecraft:stone" }));
        assert_eq!(diagnostics.items.len(), 0);
    }

    #[test]
    fn block_predicate_direct_list_before_tags() {
        let (result, _) = convert("can_break", json!({ "blocks": ["stone", "#minecraft:logs", "dirt"] }));
        assert_eq!(
            converted_value(result),
            json!([
                { "blocks": ["minecraft:stone", "minecraft:dirt"] },
                { "blocks": "#minecraft:logs" },
            ])
        );
    }

    #[test]
    fn block_predicate_unknown_block_becomes_tag() {
        let (result, _) = convert("can_place_on", json!({ "block": "custom_pack:my_block" }));
        assert_eq!(converted_value(result), json!({ "blocks": "#custom_pack:my_block" }));
    }

    #[test]
    fn block_predicate_state_filtering_and_stringification() {
        let (result, diagnostics) = convert(
            "can_place_on",
            json!({ "block": "oak_stairs", "state": { "facing": "north", "bogus": 1, "half": true } }),
        );
        assert_eq!(
            converted_value(result),
            json!({ "blocks": "minecraft:oak_stairs", "state": { "facing": "north", "half": "true" } })
        );
        assert_eq!(diagnostics.items.len(), 1);
        let diagnostic = &diagnostics.items[0];
        assert_eq!(diagnostic.code, "COMPONENT_BLOCK_STATE_PROPERTY_IGNORED");
        assert_eq!(
            diagnostic.message,
            "Nexo ignores unknown block-state property bogus for minecraft:oak_stairs"
        );
        assert_eq!(
            diagnostic.field.as_deref(),
            Some("Components.can_place_on.predicates[0].state.bogus")
        );
    }

    #[test]
    fn block_predicate_state_number_stringified() {
        let (result, _) = convert("can_place_on", json!({ "block": "zombie_head", "state": { "rotation": 3 } }));
        assert_eq!(
            converted_value(result),
            json!({ "blocks": "minecraft:zombie_head", "state": { "rotation": "3" } })
        );
    }

    #[test]
    fn block_predicate_nexo_block_is_manual() {
        let (result, _) = convert("can_place_on", json!({ "nexo_block": "custom" }));
        assert_eq!(manual_reason(result), "nexo_block 需要解析运行时自定义方块状态");
    }

    #[test]
    fn block_predicate_empty_is_manual() {
        let (result, _) = convert("can_place_on", json!({}));
        assert_eq!(manual_reason(result), "需要至少一个 block predicate section");
    }

    #[test]
    fn block_predicate_invalid_location_is_manual_with_error() {
        let (result, diagnostics) = convert("can_place_on", json!({ "block": "Bad Block!" }));
        assert_eq!(manual_reason(result), "存在无效方块或方块标签 ID");
        assert!(diagnostics.has_errors());
        assert_eq!(diagnostics.items[0].code, "INVALID_RESOURCE_LOCATION");
    }

    #[test]
    fn block_predicate_non_scalar_state_is_manual() {
        let (result, _) = convert("can_place_on", json!({ "block": "oak_stairs", "state": { "facing": ["north"] } }));
        assert_eq!(manual_reason(result), "state 中包含无法静态编码的非标量属性值");
    }

    #[test]
    fn tool_conversion_keeps_key_order_and_warns_unknown_material() {
        let (result, diagnostics) = convert(
            "tool",
            json!({
                "default_mining_speed": 5,
                "damage_per_block": 2,
                "rules": [{
                    "materials": ["stone", "unknown_thing"],
                    "tags": ["#minecraft:logs"],
                    "speed": 8,
                    "correct_for_drops": true,
                }],
            }),
        );
        let value = converted_value(result);
        assert_eq!(
            value,
            json!({
                "rules": [
                    { "blocks": "minecraft:stone", "speed": 8, "correct_for_drops": true },
                    { "blocks": "#minecraft:logs", "speed": 8, "correct_for_drops": true },
                ],
                "can_destroy_blocks_in_creative": false,
                "default_mining_speed": 5,
                "damage_per_block": 2,
            })
        );
        let keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec!["rules", "can_destroy_blocks_in_creative", "default_mining_speed", "damage_per_block"]
        );
        assert_eq!(diagnostics.items.len(), 1);
        assert_eq!(diagnostics.items[0].code, "COMPONENT_TOOL_BLOCK_INVALID");
        assert_eq!(diagnostics.items[0].field.as_deref(), Some("Components.tool.rules[0].materials"));
    }

    #[test]
    fn tool_defaults_are_omitted() {
        let (result, _) = convert("tool", json!({ "rules": [] }));
        assert_eq!(converted_value(result), json!({ "rules": [], "can_destroy_blocks_in_creative": false }));
    }

    #[test]
    fn tool_zero_speed_is_manual() {
        let (result, _) = convert("tool", json!({ "rules": [{ "materials": ["stone"], "speed": 0 }] }));
        assert_eq!(manual_reason(result), "tool rule speed 必须大于 0 才能通过 1.21.11 codec");
    }

    #[test]
    fn tool_non_object_is_manual() {
        let (result, _) = convert("tool", json!("stone"));
        assert_eq!(manual_reason(result), "tool 必须是 section");
    }

    #[test]
    fn tool_named_rule_sections() {
        let (result, _) = convert(
            "tool",
            json!({ "rules": { "fast": { "material": "stone", "speed": 4 }, "slow": { "material": "dirt", "speed": 0.5 } } }),
        );
        assert_eq!(
            converted_value(result),
            json!({
                "rules": [
                    { "blocks": "minecraft:stone", "speed": 4, "correct_for_drops": false },
                    { "blocks": "minecraft:dirt", "speed": 0.5, "correct_for_drops": false },
                ],
                "can_destroy_blocks_in_creative": false,
            })
        );
    }

    #[test]
    fn tool_single_rule_object_with_direct_keys() {
        let (result, _) = convert("tool", json!({ "rules": { "material": "stone", "speed": 4 } }));
        assert_eq!(
            converted_value(result),
            json!({
                "rules": [{ "blocks": "minecraft:stone", "speed": 4, "correct_for_drops": false }],
                "can_destroy_blocks_in_creative": false,
            })
        );
    }

    #[test]
    fn jukebox_known_song() {
        let (result, _) = convert("jukebox_playable", json!("pigstep"));
        assert_eq!(converted_value(result), json!("minecraft:pigstep"));
    }

    #[test]
    fn jukebox_section_song_key() {
        let (result, _) = convert("jukebox_playable", json!({ "song_key": "pigstep" }));
        assert_eq!(converted_value(result), json!("minecraft:pigstep"));
    }

    #[test]
    fn jukebox_unknown_vanilla_song_is_manual() {
        let (result, _) = convert("jukebox_playable", json!({ "song": "minecraft:nope" }));
        assert_eq!(manual_reason(result), "未知的 vanilla jukebox song 需要运行时 registry");
    }

    #[test]
    fn jukebox_custom_namespace_is_allowed() {
        let (result, _) = convert("jukebox_playable", json!("mypack:disc"));
        assert_eq!(converted_value(result), json!("mypack:disc"));
    }

    #[test]
    fn jukebox_missing_song_is_manual() {
        let (result, _) = convert("jukebox_playable", json!({}));
        assert_eq!(manual_reason(result), "jukebox_playable 缺少可编码的 song key");
    }

    #[test]
    fn use_remainder_string_form() {
        let (result, _) = convert("use_remainder", json!("apple"));
        assert_eq!(converted_value(result), json!({ "id": "minecraft:apple", "count": 1 }));
    }

    #[test]
    fn use_remainder_unknown_string_is_manual() {
        let (result, _) = convert("use_remainder", json!("custom:thing"));
        assert_eq!(manual_reason(result), "use_remainder 的 minecraft_type 不是 1.21.11 item registry entry");
    }

    #[test]
    fn use_remainder_count_is_clamped() {
        let (result, _) = convert("use_remainder", json!({ "minecraft_type": "apple", "amount": 200 }));
        assert_eq!(converted_value(result), json!({ "id": "minecraft:apple", "count": 99 }));
    }

    #[test]
    fn use_remainder_custom_item_is_manual() {
        let (result, _) = convert("use_remainder", json!({ "nexo_item": "custom" }));
        assert_eq!(manual_reason(result), "自定义或序列化 ItemStack 余留物需要运行时物品注册表");
    }

    #[test]
    fn use_remainder_unknown_type_is_manual() {
        let (result, _) = convert("use_remainder", json!({ "minecraft_type": "custom:thing" }));
        assert_eq!(manual_reason(result), "仅有效的 Minecraft 1.21.11 minecraft_type 余留物可安全静态转换");
    }

    #[test]
    fn death_protection_applies_effects() {
        let (result, diagnostics) = convert(
            "death_protection",
            json!({ "death_effects": { "apply_effects": { "speed": { "duration": "5s", "amplifier": 1 } } } }),
        );
        assert_eq!(
            converted_value(result),
            json!({
                "death_effects": [{
                    "type": "minecraft:apply_effects",
                    "effects": [{
                        "id": "minecraft:speed",
                        "amplifier": 1,
                        "duration": 100,
                        "ambient": true,
                        "show_particles": true,
                        "show_icon": true,
                    }],
                    "probability": 1,
                }],
            })
        );
        assert_eq!(diagnostics.items.len(), 0);
    }

    #[test]
    fn death_protection_remove_and_clear_effects() {
        let (result, diagnostics) = convert(
            "death_protection",
            json!({ "death_effects": { "remove_effects": ["speed", "custom:nope"], "clear_all_effects": true } }),
        );
        assert_eq!(
            converted_value(result),
            json!({
                "death_effects": [
                    { "type": "minecraft:remove_effects", "effects": "minecraft:speed" },
                    { "type": "minecraft:clear_all_effects" },
                ],
            })
        );
        assert_eq!(diagnostics.items.len(), 1);
        assert_eq!(diagnostics.items[0].code, "COMPONENT_EFFECT_UNKNOWN_IGNORED");
    }

    #[test]
    fn death_protection_teleport_default_diameter() {
        let (result, _) = convert("death_protection", json!({ "death_effects": { "teleport_randomly": {} } }));
        assert_eq!(
            converted_value(result),
            json!({ "death_effects": [{ "type": "minecraft:teleport_randomly", "diameter": 16 }] })
        );
    }

    #[test]
    fn death_protection_unknown_key_is_manual() {
        let (result, _) = convert("death_protection", json!({ "death_effects": { "explode": true } }));
        assert_eq!(manual_reason(result), "包含未知 death effect: explode");
    }

    #[test]
    fn death_protection_teleport_invalid_diameter_is_manual() {
        let (result, _) = convert(
            "death_protection",
            json!({ "death_effects": { "teleport_randomly": { "diameter": 0 } } }),
        );
        assert_eq!(manual_reason(result), "包含未知 death effect: TELEPORT_RANDOMLY.diameter 必须大于 0");
    }

    #[test]
    fn death_protection_non_object_is_manual() {
        let (result, _) = convert("death_protection", json!(true));
        assert_eq!(manual_reason(result), "death_protection 必须是 section");
    }

    #[test]
    fn consumable_inherits_vanilla_baseline() {
        let (result, _) = convert_with("consumable", json!({}), JsonObject::new(), "dried_kelp", "test_item");
        assert_eq!(converted_value(result), json!({ "consume_seconds": 0.8 }));
    }

    #[test]
    fn consumable_consume_duration_string() {
        let (result, _) =
            convert_with("consumable", json!({ "consume_duration": "2s" }), JsonObject::new(), "dried_kelp", "test_item");
        assert_eq!(converted_value(result), json!({ "consume_seconds": 2 }));
    }

    #[test]
    fn consumable_empty_effects_strip_baseline() {
        let (result, _) = convert_with("consumable", json!({ "effects": {} }), JsonObject::new(), "chicken", "test_item");
        assert_eq!(converted_value(result), json!({}));
    }

    #[test]
    fn consumable_baseline_kept_when_effects_absent() {
        let (result, _) = convert_with("consumable", json!({}), JsonObject::new(), "chicken", "test_item");
        let value = converted_value(result);
        assert!(value.get("on_consume_effects").is_some());
    }

    #[test]
    fn consumable_unknown_consume_effect_is_manual() {
        let (result, _) =
            convert_with("consumable", json!({ "effects": { "custom_hook": true } }), JsonObject::new(), "apple", "test_item");
        assert_eq!(manual_reason(result), "包含未知 consume effect: custom_hook");
    }

    #[test]
    fn consumable_animation_validation() {
        let (result, _) =
            convert_with("consumable", json!({ "animation": "DRINK" }), JsonObject::new(), "apple", "test_item");
        assert_eq!(converted_value(result), json!({ "animation": "drink" }));
        let (result, _) =
            convert_with("consumable", json!({ "animation": "fly" }), JsonObject::new(), "apple", "test_item");
        assert_eq!(converted_value(result), json!({ "animation": "eat" }));
    }

    #[test]
    fn consumable_custom_sound_is_wrapped() {
        let (result, _) =
            convert_with("consumable", json!({ "sound": "mypack:custom_sound" }), JsonObject::new(), "apple", "test_item");
        assert_eq!(converted_value(result), json!({ "sound": { "sound_id": "mypack:custom_sound" } }));
    }

    #[test]
    fn consumable_vanilla_sound_stays_bare() {
        let (result, _) =
            convert_with("consumable", json!({ "sound": "entity.generic.eat" }), JsonObject::new(), "apple", "test_item");
        assert_eq!(converted_value(result), json!({ "sound": "minecraft:entity.generic.eat" }));
    }

    #[test]
    fn consumable_unknown_play_sound_warns() {
        let (result, diagnostics) = convert_with(
            "consumable",
            json!({ "effects": { "play_sound": { "sound": "nope_sound" } } }),
            JsonObject::new(),
            "apple",
            "test_item",
        );
        assert_eq!(converted_value(result), json!({}));
        assert_eq!(diagnostics.items.len(), 1);
        assert_eq!(diagnostics.items[0].code, "COMPONENT_SOUND_UNKNOWN_IGNORED");
        assert_eq!(diagnostics.items[0].message, "Nexo ignores unknown sound event minecraft:nope_sound");
        assert_eq!(diagnostics.items[0].field.as_deref(), Some("Components.consumable.effects.PLAY_SOUND.sound"));
    }

    #[test]
    fn consumable_unknown_apply_effect_warns_case_insensitively() {
        let (result, diagnostics) = convert_with(
            "consumable",
            json!({ "effects": { "APPLY_EFFECTS": { "custom:effect": { "duration": 5 } } } }),
            JsonObject::new(),
            "apple",
            "test_item",
        );
        assert_eq!(converted_value(result), json!({}));
        assert_eq!(diagnostics.items.len(), 1);
        assert_eq!(diagnostics.items[0].code, "COMPONENT_EFFECT_UNKNOWN_IGNORED");
        assert_eq!(
            diagnostics.items[0].field.as_deref(),
            Some("Components.consumable.effects.APPLY_EFFECTS.custom:effect")
        );
    }

    #[test]
    fn consumable_non_object_is_manual() {
        let (result, _) = convert("consumable", json!(5));
        assert_eq!(manual_reason(result), "consumable 必须是 section");
    }

    #[test]
    fn equippable_slot_alias_and_defaults() {
        // Non-elytra materials default to glider-like, so the elytra equip
        // sound differs from the codec default and must be emitted.
        let (result, _) = convert("equippable", json!({ "slot": "HAND" }));
        assert_eq!(
            converted_value(result),
            json!({ "slot": "mainhand", "equip_sound": "minecraft:item.armor.equip_elytra" })
        );
    }

    #[test]
    fn equippable_invalid_slot_is_manual() {
        let (result, _) = convert("equippable", json!({ "slot": "back" }));
        assert_eq!(manual_reason(result), "equippable.slot 不是 Minecraft 1.21.11 的有效槽位");
    }

    #[test]
    fn equippable_missing_slot_is_manual() {
        let (result, _) = convert("equippable", json!({}));
        assert_eq!(manual_reason(result), "equippable.slot 缺失");
    }

    #[test]
    fn equippable_elytra_material_defaults() {
        let (result, _) = convert_with("equippable", json!({ "slot": "chest" }), JsonObject::new(), "elytra", "test_item");
        assert_eq!(converted_value(result), json!({ "slot": "chest", "damage_on_hurt": false }));
    }

    #[test]
    fn equippable_glider_component_switches_equip_sound() {
        let components = json!({ "glider": true }).as_object().unwrap().clone();
        let (result, _) = convert_with("equippable", json!({ "slot": "head" }), components, "paper", "test_item");
        assert_eq!(
            converted_value(result),
            json!({ "slot": "head", "equip_sound": "minecraft:item.armor.equip_elytra" })
        );
    }

    #[test]
    fn equippable_harness_item_defaults_to_shearing() {
        let (result, _) = convert_with("equippable", json!({ "slot": "body" }), JsonObject::new(), "paper", "wolf_harness");
        assert_eq!(
            converted_value(result),
            json!({ "slot": "body", "equip_sound": "minecraft:item.armor.equip_elytra", "can_be_sheared": true })
        );
    }

    #[test]
    fn equippable_unknown_entity_is_manual() {
        let (result, _) = convert("equippable", json!({ "slot": "head", "allowed_entity_types": ["custom:mob"] }));
        assert_eq!(manual_reason(result), "allowed_entity_types 包含未知的 1.21.11 entity type: custom:mob");
    }

    #[test]
    fn equippable_default_shear_sound_is_omitted() {
        let (result, _) = convert("equippable", json!({ "slot": "body", "shear_sound": "minecraft:item.shears.snip" }));
        assert_eq!(
            converted_value(result),
            json!({ "slot": "body", "equip_sound": "minecraft:item.armor.equip_elytra" })
        );
    }

    #[test]
    fn repairable_direct_items() {
        let (result, _) = convert("repairable", json!(["apple", "minecraft:diamond"]));
        assert_eq!(converted_value(result), json!({ "items": ["minecraft:apple", "minecraft:diamond"] }));
    }

    #[test]
    fn repairable_single_tag() {
        let (result, _) = convert("repairable", json!(["#minecraft:planks"]));
        assert_eq!(converted_value(result), json!({ "items": "#minecraft:planks" }));
    }

    #[test]
    fn repairable_unknown_item_becomes_tag() {
        let (result, _) = convert("repairable", json!(["custom:ingot"]));
        assert_eq!(converted_value(result), json!({ "items": "#custom:ingot" }));
    }

    #[test]
    fn repairable_mixed_tags_and_items_is_manual() {
        let (result, _) = convert("repairable", json!(["apple", "#minecraft:planks"]));
        assert_eq!(manual_reason(result), "多个标签或标签与物品混合需要展开运行时 item registry");
    }

    #[test]
    fn repairable_empty_is_manual() {
        let (result, _) = convert("repairable", json!([]));
        assert_eq!(manual_reason(result), "repairable 没有可解析的原版物品或标签");
    }

    #[test]
    fn weapon_defaults_emit_empty_object() {
        let (result, _) = convert("weapon", json!({}));
        assert_eq!(converted_value(result), json!({}));
    }

    #[test]
    fn weapon_values() {
        let (result, _) = convert("weapon", json!({ "damage_per_attack": 5, "disable_blocking_for_seconds": 2 }));
        assert_eq!(
            converted_value(result),
            json!({ "item_damage_per_attack": 5, "disable_blocking_for_seconds": 2 })
        );
    }

    #[test]
    fn blocks_attacks_full_conversion() {
        let (result, diagnostics) = convert(
            "blocks_attacks",
            json!({
                "block_delay": "0.5s",
                "disable_cooldown_scale": 0.5,
                "bypassed_by": "bypasses_armor",
                "item_damage": { "threshold": 2, "base": 1, "factor": 0.5 },
                "damage_reductions": [{ "base": 2, "factor": 0.5, "types": ["#minecraft:bypasses_armor"] }],
            }),
        );
        assert_eq!(
            converted_value(result),
            json!({
                "block_delay_seconds": 0.5,
                "disable_cooldown_scale": 0.5,
                "bypassed_by": "#minecraft:bypasses_armor",
                "item_damage": { "threshold": 2, "base": 1, "factor": 0.5 },
                "damage_reductions": [{ "base": 2, "factor": 0.5, "type": "#minecraft:bypasses_armor" }],
            })
        );
        assert_eq!(diagnostics.items.len(), 0);
    }

    #[test]
    fn blocks_attacks_direct_damage_type() {
        let (result, _) = convert("blocks_attacks", json!({ "damage_reductions": [{ "types": ["minecraft:fall"] }] }));
        assert_eq!(
            converted_value(result),
            json!({ "damage_reductions": [{ "base": 1, "factor": 1, "type": "minecraft:fall" }] })
        );
    }

    #[test]
    fn blocks_attacks_unknown_damage_type_is_manual() {
        let (result, _) = convert("blocks_attacks", json!({ "damage_reductions": [{ "types": ["minecraft:nope"] }] }));
        assert_eq!(manual_reason(result), "damage_reductions.type 包含未知的运行时 damage type: minecraft:nope");
    }

    #[test]
    fn blocks_attacks_zero_angle_is_manual() {
        let (result, _) = convert("blocks_attacks", json!({ "damage_reductions": [{ "horizontal_blocking": 0 }] }));
        assert_eq!(manual_reason(result), "damage reduction horizontal_blocking 必须大于 0 才能通过 1.21.11 codec");
    }

    #[test]
    fn attack_range_combined_string() {
        let (result, _) = convert("attack_range", json!({ "reach": "2..4", "hitbox_margin": 0.5 }));
        assert_eq!(converted_value(result), json!({ "min_reach": 2, "max_reach": 4, "hitbox_margin": 0.5 }));
    }

    #[test]
    fn attack_range_clamps_to_64() {
        let (result, _) = convert("attack_range", json!({ "reach": "100..200" }));
        assert_eq!(converted_value(result), json!({ "min_reach": 64, "max_reach": 64 }));
    }

    #[test]
    fn attack_range_defaults_emit_empty_object() {
        let (result, _) = convert("attack_range", json!({}));
        assert_eq!(converted_value(result), json!({}));
    }

    #[test]
    fn attack_range_explicit_keys_override_combined() {
        let (result, _) = convert("attack_range", json!({ "reach": 5, "min_reach": 1 }));
        assert_eq!(converted_value(result), json!({ "min_reach": 1, "max_reach": 5 }));
    }

    #[test]
    fn kinetic_defaults_emit_empty_object() {
        let (result, _) = convert("kinetic_weapon", json!({}));
        assert_eq!(converted_value(result), json!({}));
    }

    #[test]
    fn kinetic_cooldown_and_conditions() {
        let (result, _) = convert(
            "kinetic_weapon",
            json!({ "contact_cooldown": "1s", "damage_conditions": { "max_duration": "2t", "min_speed": 0.5 } }),
        );
        assert_eq!(
            converted_value(result),
            json!({
                "contact_cooldown_ticks": 20,
                "damage_conditions": { "max_duration_ticks": 2, "min_speed": 0.5 },
            })
        );
    }

    #[test]
    fn piercing_defaults_emit_empty_object() {
        let (result, _) = convert("piercing_weapon", json!({}));
        assert_eq!(converted_value(result), json!({}));
    }

    #[test]
    fn piercing_flags() {
        let (result, _) = convert("piercing_weapon", json!({ "deals_knockback": false, "dismounts": true }));
        assert_eq!(converted_value(result), json!({ "deals_knockback": false, "dismounts": true }));
    }

    #[test]
    fn swing_type_and_duration() {
        let (result, _) = convert("swing_animation", json!({ "type": "STAB", "duration": "0.5s" }));
        assert_eq!(converted_value(result), json!({ "type": "stab", "duration": 10 }));
    }

    #[test]
    fn swing_defaults_emit_empty_object() {
        let (result, _) = convert("swing_animation", json!({}));
        assert_eq!(converted_value(result), json!({}));
    }

    #[test]
    fn swing_zero_duration_is_manual() {
        let (result, _) = convert("swing_animation", json!({ "duration": 0 }));
        assert_eq!(manual_reason(result), "swing_animation.duration 必须至少为 1 tick");
    }

    #[test]
    fn use_effects_values() {
        let (result, _) = convert(
            "use_effects",
            json!({ "can_sprint": true, "interact_vibrations": false, "speed_multiplier": 0.5 }),
        );
        assert_eq!(
            converted_value(result),
            json!({ "can_sprint": true, "interact_vibrations": false, "speed_multiplier": 0.5 })
        );
    }

    #[test]
    fn use_effects_defaults_emit_empty_object() {
        let (result, _) = convert("use_effects", json!({}));
        assert_eq!(converted_value(result), json!({}));
    }

    #[test]
    fn duration_parsing_units() {
        assert_eq!(duration_seconds(Some(&json!("500ms")), 0.0), 0.5);
        assert_eq!(duration_seconds(Some(&json!("2 minutes")), 0.0), 120.0);
        assert_eq!(duration_seconds(Some(&json!("1.5s")), 0.0), 1.5);
        assert_eq!(duration_seconds(Some(&json!("2 TICKS")), 0.0), 0.1);
        assert_eq!(duration_seconds(Some(&json!("abc")), 9.0), 9.0);
        assert_eq!(duration_seconds(Some(&json!(5)), 0.0), 5.0);
        assert_eq!(duration_seconds(Some(&json!(-3)), 0.0), 0.0);
        assert_eq!(duration_ticks(Some(&json!("1s")), 0.0), 20.0);
        assert_eq!(duration_ticks(Some(&json!("3t")), 0.0), 3.0);
        assert_eq!(duration_ticks(None, 10.0), 10.0);
    }

    #[test]
    fn js_number_parsing_matches_js() {
        assert_eq!(parse_js_number(""), Some(0.0));
        assert_eq!(parse_js_number(" 5 "), Some(5.0));
        assert_eq!(parse_js_number(".5"), Some(0.5));
        assert_eq!(parse_js_number("5."), Some(5.0));
        assert_eq!(parse_js_number("-.5"), Some(-0.5));
        assert_eq!(parse_js_number("5.e2"), Some(500.0));
        assert_eq!(parse_js_number("0x10"), Some(16.0));
        assert_eq!(parse_js_number("abc"), None);
    }

    #[test]
    fn number_stringification_matches_js() {
        assert_eq!(js_number_to_string(1.0), "1");
        assert_eq!(js_number_to_string(1.5), "1.5");
        assert_eq!(js_number_to_string(-0.0), "0");
        assert_eq!(js_number_to_string(1e21), "1e+21");
        assert_eq!(js_number_to_string(1e-7), "1e-7");
    }

    #[test]
    fn integer_values_serialize_as_json_integers() {
        let (result, _) = convert("weapon", json!({ "damage_per_attack": 3.0 }));
        let value = converted_value(result);
        assert_eq!(value.get("item_damage_per_attack"), Some(&json!(3)));
        assert_eq!(serde_json::to_string(&value).unwrap(), r#"{"item_damage_per_attack":3}"#);
    }
}

