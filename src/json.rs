//! JSON value helpers shared by the whole converter.
//!
//! Port of `legacy/src/types.ts`. Key lookups are case-insensitive because
//! Nexo configuration keys are matched that way; `serde_json` is built with
//! `preserve_order` so object key order stays stable for readable output.

use serde_json::Value;

/// Ordered JSON object map (insertion order preserved).
pub type JsonObject = serde_json::Map<String, Value>;

pub fn is_object(value: &Value) -> bool {
    value.is_object()
}

pub fn deep_clone(value: &Value) -> Value {
    value.clone()
}

/// Case-insensitive key lookup, mirroring `findKey` in types.ts.
pub fn find_key(object: &JsonObject, wanted: &str) -> Option<&String> {
    let needle = wanted.to_lowercase();
    object.keys().find(|key| key.to_lowercase() == needle)
}

pub fn has_key(object: &JsonObject, wanted: &str) -> bool {
    find_key(object, wanted).is_some()
}

pub fn get_value<'a>(object: &'a JsonObject, wanted: &str) -> Option<&'a Value> {
    find_key(object, wanted).and_then(|key| object.get(key))
}

pub fn get_object<'a>(object: &'a JsonObject, wanted: &str) -> Option<&'a JsonObject> {
    match get_value(object, wanted) {
        Some(Value::Object(map)) => Some(map),
        _ => None,
    }
}

pub fn get_string<'a>(object: &'a JsonObject, wanted: &str) -> Option<&'a str> {
    match get_value(object, wanted) {
        Some(Value::String(text)) => Some(text.as_str()),
        _ => None,
    }
}

pub fn get_boolean(object: &JsonObject, wanted: &str, fallback: bool) -> bool {
    match get_value(object, wanted) {
        Some(Value::Bool(flag)) => *flag,
        _ => fallback,
    }
}

pub fn get_number(object: &JsonObject, wanted: &str) -> Option<f64> {
    match get_value(object, wanted) {
        Some(Value::Number(number)) => number.as_f64().filter(|value| value.is_finite()),
        _ => None,
    }
}

/// Mirrors `asStringList`: a bare string becomes a one-element list and
/// non-string entries are dropped.
pub fn as_string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(text)) => vec![text.clone()],
        Some(Value::Array(entries)) => entries
            .iter()
            .filter_map(|entry| match entry {
                Value::String(text) => Some(text.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Recursive merge; `override` wins for scalars and arrays, objects merge.
pub fn deep_merge(base: &JsonObject, r#override: &JsonObject) -> JsonObject {
    let mut result = base.clone();
    for (key, value) in r#override {
        let merged = match (result.get(key), value) {
            (Some(Value::Object(prior)), Value::Object(next)) => {
                Value::Object(deep_merge(prior, next))
            }
            _ => value.clone(),
        };
        result.insert(key.clone(), merged);
    }
    result
}

/// Case-insensitive key removal, mirroring `withoutKeys`.
pub fn without_keys(object: &JsonObject, names: &[&str]) -> JsonObject {
    let denied: Vec<String> = names.iter().map(|name| name.to_lowercase()).collect();
    object
        .iter()
        .filter(|(key, _)| !denied.contains(&key.to_lowercase()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Build an object from entries, dropping `None` values.
pub fn compact_object(entries: Vec<(String, Option<Value>)>) -> JsonObject {
    entries
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key, value)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn find_key_is_case_insensitive() {
        let map = json!({ "CustomModelData": 1 }).as_object().unwrap().clone();
        assert_eq!(find_key(&map, "custommodeldata").map(String::as_str), Some("CustomModelData"));
        assert!(find_key(&map, "missing").is_none());
    }

    #[test]
    fn deep_merge_merges_nested_objects() {
        let base = json!({ "a": { "x": 1, "y": 1 }, "b": 1 }).as_object().unwrap().clone();
        let over = json!({ "a": { "y": 2, "z": 3 }, "c": 4 }).as_object().unwrap().clone();
        let merged = Value::Object(deep_merge(&base, &over));
        assert_eq!(merged, json!({ "a": { "x": 1, "y": 2, "z": 3 }, "b": 1, "c": 4 }));
    }

    #[test]
    fn as_string_list_handles_scalar_and_array() {
        assert_eq!(as_string_list(Some(&json!("a"))), vec!["a".to_string()]);
        assert_eq!(as_string_list(Some(&json!(["a", 1, "b"]))), vec!["a".to_string(), "b".to_string()]);
        assert!(as_string_list(None).is_empty());
    }

    #[test]
    fn without_keys_drops_case_insensitively() {
        let map = json!({ "Keep": 1, "Drop": 2, "also_drop": 3 }).as_object().unwrap().clone();
        let kept = without_keys(&map, &["drop", "ALSO_DROP"]);
        assert_eq!(Value::Object(kept), json!({ "Keep": 1 }));
    }
}
