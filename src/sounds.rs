//! Nexo sounds.yml conversion.
//!
//! Port of `legacy/src/sounds.ts`.

use serde_json::{json, Value};

use crate::diagnostics::{Details, DiagnosticBag};
use crate::json::{as_string_list, get_boolean, get_number, get_object, get_string, get_value, JsonObject};
use crate::resource_location::normalize_sound_location;

/// JSON number rendered the way TS JSON.stringify would emit it (integral
/// values stay integers: 16.0 -> 16).
fn number_value(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() <= i64::MAX as f64 {
        Value::from(value as i64)
    } else {
        Value::from(value)
    }
}

pub fn convert_sounds(root: &JsonObject, diagnostics: &mut DiagnosticBag, source: &str) -> JsonObject {
    let mut result = JsonObject::new();
    let raw = get_value(root, "sounds");
    let Some(Value::Array(entries)) = raw else {
        if raw.is_some() {
            diagnostics.error(
                "SOUNDS_NOT_LIST",
                "Nexo 1.26 sounds must be a list",
                Details::new().source(source).field("sounds"),
            );
        }
        return result;
    };

    for entry in entries {
        let Value::Object(entry) = entry else {
            diagnostics.error(
                "SOUND_ENTRY_INVALID",
                "Each Nexo sound entry must be a map",
                Details::new().source(source).field("sounds"),
            );
            continue;
        };
        let Some(raw_id) = get_string(entry, "id") else {
            diagnostics.error(
                "SOUND_ID_MISSING",
                "Sound entry has no id",
                Details::new().source(source).field("sounds.id"),
            );
            continue;
        };
        let raw_id = raw_id.to_string();
        let details = Details::new().source(source).field("sounds.id");
        let Some(id) = normalize_sound_location(&raw_id, diagnostics, &details) else {
            continue;
        };

        let mut files = as_string_list(get_value(entry, "sounds"));
        if files.is_empty() {
            if let Some(single) = get_string(entry, "sound") {
                files.push(single.to_string());
            }
        }
        if files.is_empty() {
            diagnostics.error(
                "SOUND_FILES_MISSING",
                "Sound event has neither sound nor sounds",
                Details::new().source(source).field(&raw_id),
            );
            continue;
        }

        let mut converted: Vec<Value> = Vec::new();
        for file in &files {
            let details = Details::new().source(source).field(format!("{}.sound", raw_id));
            let Some(name) = normalize_sound_location(file, diagnostics, &details) else {
                continue;
            };
            converted.push(json!({
                "name": name,
                "stream": get_boolean(entry, "stream", false),
                "preload": get_boolean(entry, "preload", false),
                "volume": number_value(get_number(entry, "volume").unwrap_or(1.0)),
                "pitch": number_value(get_number(entry, "pitch").unwrap_or(1.0)),
                "weight": number_value(get_number(entry, "weight").unwrap_or(1.0)),
                "attenuation_distance": number_value(get_number(entry, "attenuation_distance").unwrap_or(16.0)),
            }));
        }
        result.insert(id, json!({ "sounds": converted }));

        if get_object(entry, "jukebox_playable").is_some() {
            diagnostics.warning(
                "JUKEBOX_SONG_MANUAL",
                "Nexo jukebox_playable registration needs a separate CraftEngine jukebox song/item migration",
                Details::new()
                    .source(source)
                    .field(format!("{}.jukebox_playable", raw_id))
                    .lossy(),
            );
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_sound_entries_with_defaults() {
        let root = json!({
            "sounds": [
                { "id": "custom:hit", "sound": "custom:hit1", "volume": 0.5 }
            ]
        })
        .as_object()
        .unwrap()
        .clone();
        let mut diags = DiagnosticBag::new();
        let result = convert_sounds(&root, &mut diags, "sounds.yml");
        let event = &result["custom:hit"]["sounds"][0];
        assert_eq!(event["name"], "custom:hit1");
        assert_eq!(event["volume"], 0.5);
        assert_eq!(event["attenuation_distance"], 16);
        assert!(!diags.has_errors());
    }

    #[test]
    fn rejects_non_list_sounds() {
        let root = json!({ "sounds": { "id": "x" } }).as_object().unwrap().clone();
        let mut diags = DiagnosticBag::new();
        let result = convert_sounds(&root, &mut diags, "sounds.yml");
        assert!(result.is_empty());
        assert!(diags.has_errors());
    }
}
