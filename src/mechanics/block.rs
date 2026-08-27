//! Custom block conversion: noteblock / stringblock / chorusblock.
//!
//! Port of `mapBlockSounds` and `convertBlock` (legacy/src/mechanics.ts,
//! lines 894-950).

use serde_json::{json, Number, Value};

use crate::json::{get_boolean, get_number, get_object, get_value, JsonObject};
use crate::resource_location::normalize_sound_location;

use super::furniture::{is_simple_self_loot, nested_number, nested_sound};
use super::{detail, Context};

pub(crate) struct BlockConversion {
    pub definition: JsonObject,
    pub behavior: JsonObject,
    pub semantics: JsonObject,
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

fn map_block_sounds(section: Option<&JsonObject>, context: &mut Context) -> Option<JsonObject> {
    let section = section?;
    // (event, default volume, default pitch); iteration order matches TS.
    let defaults: &[(&str, f64, f64)] = &[
        ("place", 1.0, 0.8),
        ("break", 1.0, 0.8),
        ("hit", 0.25, 0.5),
        ("step", 0.15, 1.0),
        ("fall", 0.5, 0.75),
    ];
    let mut result = JsonObject::new();
    for (event, volume, pitch) in defaults {
        // TS `if (!sound) continue;` skips empty strings too.
        let Some(sound) = nested_sound(section, event).filter(|sound| !sound.is_empty()) else { continue };
        let details = detail(context, &format!("Mechanics.block.block_sounds.{}", event));
        let id = normalize_sound_location(&sound, context.diagnostics, &details).unwrap_or(sound);
        result.insert(
            (*event).to_string(),
            json!({
                "id": id,
                "volume": json_number(nested_number(section, event, "volume", *volume)),
                "pitch": json_number(nested_number(section, event, "pitch", *pitch)),
            }),
        );
    }
    if result.is_empty() { None } else { Some(result) }
}

pub(crate) fn convert_block(
    block_type: &str,
    mechanic: &JsonObject,
    base_model: Option<&str>,
    context: &mut Context,
) -> Option<BlockConversion> {
    // TS `if (!baseModel)` treats an empty model string as missing too.
    let Some(base_model) = base_model.filter(|model| !model.is_empty()) else {
        let details = detail(context, "Pack.model");
        context.diagnostics.error(
            "BLOCK_MODEL_MISSING",
            "Custom block has no resolvable Pack model; its block definition and block_item behavior were suppressed",
            details,
        );
        return None;
    };
    let mut auto_state = if block_type == "noteblock" {
        "note_block"
    } else if block_type == "chorusblock" {
        "chorus"
    } else {
        "tripwire"
    };
    if block_type == "stringblock" && get_boolean(mechanic, "is_tall", false) {
        auto_state = "tripwire";
        let details = detail(context, "Mechanics.stringblock.is_tall").lossy();
        context.diagnostics.warning(
            "STRINGBLOCK_TALL_MANUAL",
            "Nexo tall stringblock placement spans states/blocks and cannot be recreated by copying custom_variation",
            details,
        );
    }
    let mut definition = JsonObject::new();
    definition.insert(
        "state".to_string(),
        json!({ "auto_state": auto_state, "model": base_model }),
    );
    let mut settings = JsonObject::new();
    if let Some(hardness) = get_number(mechanic, "hardness") {
        settings.insert("hardness".to_string(), json_number(hardness));
    }
    if let Some(resistance) = get_number(mechanic, "resistance") {
        settings.insert("resistance".to_string(), json_number(resistance));
    }
    if let Some(sounds) = map_block_sounds(get_object(mechanic, "block_sounds"), context) {
        settings.insert("sounds".to_string(), Value::Object(sounds));
    }
    if !settings.is_empty() {
        definition.insert("settings".to_string(), Value::Object(settings));
    }
    let drop_section = get_object(mechanic, "drop");
    if is_simple_self_loot(drop_section, &context.item) {
        definition.insert(
            "loot".to_string(),
            json!({
                "pools": [{
                    "rolls": 1,
                    "conditions": [{ "type": "survives_explosion" }],
                    "entries": [{ "type": "item", "item": context.target_id }],
                }],
            }),
        );
    } else {
        let details = detail(context, &format!("Mechanics.{}.drop", block_type)).lossy();
        context.diagnostics.warning(
            "CUSTOM_BLOCK_DROP_MANUAL",
            "A non-self Nexo block drop cannot be represented by CraftEngine's default self template; loot was omitted instead of producing an incorrect extra self drop",
            details,
        );
    }
    if get_value(mechanic, "custom_variation").is_some() {
        let details = detail(context, &format!("Mechanics.{}.custom_variation", block_type));
        context.diagnostics.info(
            "BLOCK_VARIATION_REALLOCATED",
            "Nexo custom_variation was intentionally not copied; CraftEngine allocates carrier states independently",
            details,
        );
    }
    for key in ["directional", "farmblock", "light", "tall", "breaking", "clickActions"] {
        if get_value(mechanic, key).is_some() {
            let details = detail(context, &format!("Mechanics.{}.{}", block_type, key)).lossy();
            context.diagnostics.warning(
                "CUSTOM_BLOCK_FEATURE_MANUAL",
                &format!("Nexo custom-block feature {} needs explicit CraftEngine reconstruction", key),
                details,
            );
        }
    }
    Some(BlockConversion {
        definition,
        behavior: json!({ "type": "block_item", "block": context.target_id })
            .as_object()
            .unwrap()
            .clone(),
        semantics: json!({ "carrier": auto_state, "nexo_variation_copied": false })
            .as_object()
            .unwrap()
            .clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::DiagnosticBag;

    fn convert(
        block_type: &str,
        mechanic: &JsonObject,
        base_model: Option<&str>,
        diags: &mut DiagnosticBag,
    ) -> Option<BlockConversion> {
        let mut context = Context {
            source: "s".to_string(),
            item: "i".to_string(),
            target_id: "ns:t".to_string(),
            diagnostics: diags,
        };
        convert_block(block_type, mechanic, base_model, &mut context)
    }

    #[test]
    fn missing_model_suppresses_block() {
        let mut diags = DiagnosticBag::new();
        let mechanic = json!({}).as_object().unwrap().clone();
        assert!(convert("noteblock", &mechanic, None, &mut diags).is_none());
        assert!(diags.items.iter().any(|d| d.code == "BLOCK_MODEL_MISSING"));
    }

    #[test]
    fn noteblock_converts_settings_self_loot_and_flags() {
        let mut diags = DiagnosticBag::new();
        let mechanic = json!({ "hardness": 2, "custom_variation": 5, "light": 10 })
            .as_object()
            .unwrap()
            .clone();
        let converted = convert("noteblock", &mechanic, Some("ns:model"), &mut diags).unwrap();
        let definition = Value::Object(converted.definition);
        assert_eq!(definition["state"], json!({ "auto_state": "note_block", "model": "ns:model" }));
        assert_eq!(definition["settings"]["hardness"], json!(2));
        assert_eq!(definition["loot"]["pools"][0]["entries"][0]["item"], json!("ns:t"));
        assert_eq!(converted.behavior.get("block"), Some(&json!("ns:t")));
        assert_eq!(converted.semantics.get("carrier"), Some(&json!("note_block")));
        assert!(diags.items.iter().any(|d| d.code == "BLOCK_VARIATION_REALLOCATED" && !d.lossy));
        assert!(diags.items.iter().any(|d| d.code == "CUSTOM_BLOCK_FEATURE_MANUAL" && d.lossy));
    }

    #[test]
    fn tall_stringblock_warns_and_non_self_drop_omits_loot() {
        let mut diags = DiagnosticBag::new();
        let mechanic = json!({
            "is_tall": true,
            "drop": { "loots": [{ "nexo_item": "other" }] },
        })
        .as_object()
        .unwrap()
        .clone();
        let converted = convert("stringblock", &mechanic, Some("ns:model"), &mut diags).unwrap();
        assert_eq!(converted.semantics.get("carrier"), Some(&json!("tripwire")));
        assert!(!converted.definition.contains_key("loot"));
        assert!(diags.items.iter().any(|d| d.code == "STRINGBLOCK_TALL_MANUAL" && d.lossy));
        assert!(diags.items.iter().any(|d| d.code == "CUSTOM_BLOCK_DROP_MANUAL" && d.lossy));
    }
}
