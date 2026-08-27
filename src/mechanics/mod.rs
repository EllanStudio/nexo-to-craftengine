//! Nexo Mechanics conversion: furniture, custom blocks, behaviors.
//!
//! Port of `legacy/src/mechanics.ts`, split into `furniture` and `block`
//! submodules around the shared vector/quaternion helpers.

pub mod block;
pub mod furniture;

use serde_json::Value;

use crate::diagnostics::{Details, DiagnosticBag};
use crate::json::{get_object, JsonObject};

pub struct MechanicsConversion {
    pub behavior: Vec<JsonObject>,
    pub furniture: Option<JsonObject>,
    pub block: Option<JsonObject>,
    pub semantics: JsonObject,
}

#[derive(Debug, Clone, Default)]
pub struct FurnitureRuntimeSettings {
    pub default_rotatable_on_sneak: Option<bool>,
    pub rotation_gamemodes: Option<Vec<String>>,
}

pub(crate) struct Context<'a> {
    pub source: String,
    pub item: String,
    pub target_id: String,
    pub diagnostics: &'a mut DiagnosticBag,
}

pub(crate) fn detail(context: &Context, field: &str) -> Details {
    Details::new()
        .source(context.source.clone())
        .item(context.item.clone())
        .field(field)
}

/// Mirrors TS `Number(...)` for trimmed scalar tokens. JS `Number("")` is
/// 0, so an empty/whitespace-only token parses to zero rather than failing.
pub(crate) fn parse_js_number(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Some(0.0);
    }
    trimmed.parse::<f64>().ok().filter(|value| value.is_finite())
}

pub(crate) fn parse_number_list(value: Option<&Value>) -> Option<Vec<f64>> {
    match value {
        Some(Value::Number(number)) => number.as_f64().filter(|value| value.is_finite()).map(|value| vec![value]),
        Some(Value::String(text)) => {
            let numbers: Vec<Option<f64>> = text.split(',').map(|part| parse_js_number(part)).collect();
            if numbers.iter().all(Option::is_some) {
                Some(numbers.into_iter().map(Option::unwrap).collect())
            } else {
                None
            }
        }
        Some(Value::Array(entries)) => {
            let numbers: Vec<Option<f64>> = entries
                .iter()
                .map(|entry| match entry {
                    Value::Number(number) => number.as_f64().filter(|value| value.is_finite()),
                    Value::String(text) => parse_js_number(text),
                    _ => None,
                })
                .collect();
            if numbers.iter().all(Option::is_some) {
                Some(numbers.into_iter().map(Option::unwrap).collect())
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(crate) fn split_with_last(value: &str, separator: &str, limit: usize) -> Vec<String> {
    if limit <= 1 {
        return vec![value.to_string()];
    }
    let mut result: Vec<String> = Vec::new();
    let mut rest = value;
    while result.len() < limit - 1 {
        let Some(index) = rest.find(separator) else { break };
        result.push(rest[..index].to_string());
        rest = &rest[index + separator.len()..];
    }
    result.push(rest.to_string());
    result
}

/// Shortest JS-style string for a finite number (1.0 -> "1").
pub(crate) fn js_number_string(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 9.007199254740992e15 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

// NexoYaml.vector3f parses a scalar/string component-by-component and fills
// missing or invalid components with the supplied default. It does not treat a
// scalar as a uniform vector.
pub(crate) fn config_vector(value: Option<&Value>, fallback: f64) -> [f64; 3] {
    let (Some(Value::Number(_)) | Some(Value::String(_))) = value else {
        return [fallback, fallback, fallback];
    };
    let text = match value.unwrap() {
        Value::Number(number) => js_number_string(number.as_f64().unwrap_or(fallback)),
        Value::String(text) => text.clone(),
        _ => unreachable!(),
    };
    let parts = split_with_last(&text, ",", 3);
    let component = |index: usize| -> f64 {
        let raw = parts.get(index).map(|part| part.trim());
        match raw {
            None | Some("") => fallback,
            Some(raw) => parse_js_number(raw).unwrap_or(fallback),
        }
    };
    [component(0), component(1), component(2)]
}

// VectorUtils.vector3fFromString/vectorFromString use per-component zero
// fallbacks and retain only the first three components.
pub(crate) fn compact_vector(value: Option<&str>, fallback: f64) -> [f64; 3] {
    let Some(value) = value else {
        return [fallback, fallback, fallback];
    };
    let cleaned = value.replace(' ', "");
    let parts: Vec<&str> = cleaned.split(',').collect();
    let component = |index: usize| -> f64 {
        match parts.get(index).copied() {
            None | Some("") => fallback,
            Some(raw) => parse_js_number(raw).unwrap_or(fallback),
        }
    };
    [component(0), component(1), component(2)]
}

pub(crate) fn vector_string(value: Option<&Value>, fallback: f64) -> String {
    config_vector(value, fallback)
        .iter()
        .map(|component| js_number_string(*component))
        .collect::<Vec<_>>()
        .join(",")
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Quaternion {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

pub(crate) const IDENTITY: Quaternion = Quaternion { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

pub(crate) fn axis_quaternion(axis: char, degrees: f64) -> Quaternion {
    let half = degrees * std::f64::consts::PI / 360.0;
    let sine = half.sin();
    if axis == 'x' {
        Quaternion { x: sine, y: 0.0, z: 0.0, w: half.cos() }
    } else {
        Quaternion { x: 0.0, y: sine, z: 0.0, w: half.cos() }
    }
}

pub(crate) fn parse_quaternion(value: Option<&Value>, side: &str) -> Quaternion {
    if let Some(Value::Number(number)) = value {
        if let Some(degrees) = number.as_f64().filter(|value| value.is_finite()) {
            return axis_quaternion(if side == "left" { 'y' } else { 'x' }, degrees);
        }
    }
    let Some(Value::String(text)) = value else {
        return IDENTITY;
    };
    let parts = split_with_last(text, ",", 4);
    if parts.len() < 4 {
        return IDENTITY;
    }
    let component = |index: usize, fallback: f64| -> f64 {
        parts.get(index).and_then(|part| parse_js_number(part)).unwrap_or(fallback)
    };
    Quaternion {
        x: component(0, 0.0),
        y: component(1, 0.0),
        z: component(2, 0.0),
        w: component(3, 1.0),
    }
}

pub(crate) fn multiply_quaternion(a: Quaternion, b: Quaternion) -> Quaternion {
    Quaternion {
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
    }
}

pub(crate) fn quaternion_identity(value: Quaternion) -> bool {
    value.x.abs() < 1e-8 && value.y.abs() < 1e-8 && value.z.abs() < 1e-8 && (value.w - 1.0).abs() < 1e-8
}

pub(crate) fn quaternion_string(value: Quaternion) -> String {
    [value.x, value.y, value.z, value.w]
        .iter()
        .map(|part| {
            let rounded: f64 = format!("{:.8}", part).parse().unwrap_or(*part);
            js_number_string(rounded)
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn uniform_scale(value: Option<&Value>, fallback: f64) -> bool {
    let [x, y, z] = config_vector(value, fallback);
    (y - x).abs() < 1e-8 && (z - x).abs() < 1e-8
}

pub fn convert_mechanics(
    config: &JsonObject,
    target_id: &str,
    base_model: Option<&str>,
    diagnostics: &mut DiagnosticBag,
    source: &str,
    item: &str,
    furniture_default_properties: Option<&JsonObject>,
    furniture_runtime: Option<&FurnitureRuntimeSettings>,
) -> MechanicsConversion {
    let mut result = MechanicsConversion {
        behavior: Vec::new(),
        furniture: None,
        block: None,
        semantics: JsonObject::new(),
    };
    let Some(mechanics) = get_object(config, "Mechanics") else {
        return result;
    };
    let mut context = Context {
        source: source.to_string(),
        item: item.to_string(),
        target_id: target_id.to_string(),
        diagnostics,
    };
    if let Some(furniture_section) = get_object(mechanics, "furniture") {
        let converted = furniture::convert_furniture(furniture_section, &mut context, furniture_default_properties, furniture_runtime);
        result.furniture = Some(converted.definition);
        result.behavior.push(converted.behavior);
        result.semantics.insert("furniture".to_string(), Value::Object(converted.semantics));
    }
    let block_types: &[&str] = &["noteblock", "stringblock", "chorusblock"];
    let present: Vec<&str> = block_types
        .iter()
        .copied()
        .filter(|block_type| get_object(mechanics, block_type).is_some())
        .collect();
    if present.len() > 1 {
        context.diagnostics.error(
            "MULTIPLE_CUSTOM_BLOCK_TYPES",
            "An item has more than one Nexo custom block carrier mechanic",
            detail(&context, "Mechanics"),
        );
    }
    if let Some(block_type) = present.first() {
        let mechanic = get_object(mechanics, block_type).unwrap();
        if let Some(converted) = block::convert_block(block_type, mechanic, base_model, &mut context) {
            result.block = Some(converted.definition);
            result.behavior.push(converted.behavior);
            result.semantics.insert("block".to_string(), Value::Object(converted.semantics));
        }
    }
    let known: &[&str] = &["furniture", "noteblock", "stringblock", "chorusblock"];
    let keys: Vec<String> = mechanics.keys().cloned().collect();
    for key in keys {
        if !known.contains(&key.to_lowercase().as_str()) {
            context.diagnostics.warning(
                "ITEM_MECHANIC_UNSUPPORTED",
                &format!("Nexo item mechanic {} was not converted", key),
                detail(&context, &format!("Mechanics.{}", key)).lossy(),
            );
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config_vector_parses_component_wise() {
        assert_eq!(config_vector(Some(&json!("1,2,3")), 0.0), [1.0, 2.0, 3.0]);
        assert_eq!(config_vector(Some(&json!("1,,x")), 9.0), [1.0, 9.0, 9.0]);
        assert_eq!(config_vector(Some(&json!(5)), 1.0), [5.0, 1.0, 1.0]);
    }

    #[test]
    fn split_with_last_keeps_remainder() {
        assert_eq!(split_with_last("a,b,c,d", ",", 3), vec!["a", "b", "c,d"]);
    }

    #[test]
    fn quaternion_roundtrip_matches_identity_detection() {
        let q = parse_quaternion(Some(&json!("0,0,0,1")), "left");
        assert!(quaternion_identity(q));
        let rotated = axis_quaternion('y', 90.0);
        assert!(!quaternion_identity(rotated));
    }

    #[test]
    fn empty_config_yields_empty_conversion() {
        let mut diags = DiagnosticBag::new();
        let config = json!({}).as_object().unwrap().clone();
        let result = convert_mechanics(&config, "ns:item", None, &mut diags, "s", "i", None, None);
        assert!(result.behavior.is_empty());
        assert!(result.furniture.is_none());
    }

    #[test]
    fn unknown_mechanic_warns_lossy() {
        let mut diags = DiagnosticBag::new();
        let config = json!({ "Mechanics": { "portal": {} } }).as_object().unwrap().clone();
        convert_mechanics(&config, "ns:item", None, &mut diags, "s", "i", None, None);
        assert!(diags.items.iter().any(|d| d.code == "ITEM_MECHANIC_UNSUPPORTED" && d.lossy));
    }
}
