//! Nexo recipe conversion.
//!
//! Port of `legacy/src/recipes.ts`.

use serde_json::{json, Value};

use crate::diagnostics::{Details, DiagnosticBag};
use crate::json::{get_number, get_object, get_string, get_value, JsonObject};
use crate::resource_location::normalize_location;

/// Nexo recipe type, mirrored from the TS string union. The legacy converter
/// iterates `RECIPE_TYPES` in this order and reads `recipes/<type>/` folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NexoRecipeType {
    Shaped,
    Shapeless,
    Furnace,
    Blasting,
    Smoking,
    Campfire,
    Stonecutting,
    Brewing,
}

impl NexoRecipeType {
    /// All recipe types in legacy `RECIPE_TYPES` order.
    pub const ALL: [NexoRecipeType; 8] = [
        Self::Shaped,
        Self::Shapeless,
        Self::Furnace,
        Self::Blasting,
        Self::Smoking,
        Self::Campfire,
        Self::Stonecutting,
        Self::Brewing,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "shaped" => Some(Self::Shaped),
            "shapeless" => Some(Self::Shapeless),
            "furnace" => Some(Self::Furnace),
            "blasting" => Some(Self::Blasting),
            "smoking" => Some(Self::Smoking),
            "campfire" => Some(Self::Campfire),
            "stonecutting" => Some(Self::Stonecutting),
            "brewing" => Some(Self::Brewing),
            _ => None,
        }
    }

    /// The Nexo recipe directory name (the TS union string).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shaped => "shaped",
            Self::Shapeless => "shapeless",
            Self::Furnace => "furnace",
            Self::Blasting => "blasting",
            Self::Smoking => "smoking",
            Self::Campfire => "campfire",
            Self::Stonecutting => "stonecutting",
            Self::Brewing => "brewing",
        }
    }

    /// Vanilla/CraftEngine type string written to `converted.type`.
    fn vanilla_type(self) -> &'static str {
        match self {
            Self::Shaped => "shaped",
            Self::Shapeless => "shapeless",
            Self::Furnace => "smelting",
            Self::Blasting => "blasting",
            Self::Smoking => "smoking",
            Self::Campfire => "campfire_cooking",
            Self::Stonecutting => "stonecutting",
            Self::Brewing => "brewing",
        }
    }
}

struct RecipeContext<'a> {
    namespace: &'a str,
    source: &'a str,
    id: &'a str,
    diagnostics: &'a mut DiagnosticBag,
}

fn detail(context: &RecipeContext, field: impl Into<String>, lossy: bool) -> Details {
    let mut details = Details::new().source(context.source).item(context.id).field(field);
    if lossy {
        details = details.lossy();
    }
    details
}

/// Serialize a number the way JSON.stringify prints JS numbers: integral
/// values become integers instead of `1.0`-style floats.
fn number_json(value: f64) -> Value {
    if value.fract() == 0.0 && value >= i64::MIN as f64 && value < 9223372036854775808.0 {
        json!(value as i64)
    } else {
        json!(value)
    }
}

fn normalize_item(value: &str, context: &mut RecipeContext, field: &str, custom: bool) -> Option<String> {
    let default_namespace = if custom { context.namespace } else { "minecraft" };
    let details = detail(context, field, false);
    normalize_location(&value.to_lowercase(), context.diagnostics, &details, &[], default_namespace)
}

fn choice(section: &JsonObject, context: &mut RecipeContext, field: &str) -> Option<String> {
    if let Some(nexo) = get_string(section, "nexo_item").filter(|value| !value.is_empty()) {
        return normalize_item(nexo, context, &format!("{}.nexo_item", field), true);
    }
    let crucible = get_string(section, "crucible_item").map_or(false, |value| !value.is_empty());
    let mmoitems = get_string(section, "mmoitems_id").map_or(false, |value| !value.is_empty());
    if crucible || mmoitems {
        context.diagnostics.warning(
            "RECIPE_EXACT_EXTERNAL_CHOICE",
            "Crucible/MMOItems ExactChoice has priority in Nexo and cannot be reconstructed as a plain CraftEngine item id",
            detail(context, field, true),
        );
        return None;
    }
    if let Some(minecraft) = get_string(section, "minecraft_type").filter(|value| !value.is_empty()) {
        return normalize_item(minecraft, context, &format!("{}.minecraft_type", field), false);
    }
    if let Some(tag) = get_string(section, "tag").filter(|value| !value.is_empty()) {
        let stripped = tag.strip_prefix('#').unwrap_or(tag);
        let location = normalize_item(stripped, context, &format!("{}.tag", field), false);
        return location.map(|location| format!("#{}", location));
    }
    if get_value(section, "nexo_tag").is_some() {
        context.diagnostics.warning(
            "RECIPE_NEXO_TAG_UNEXPANDED",
            "Nexo expands nexo_tag into multiple recipes before loading; tag definitions are required for an exact conversion",
            detail(context, field, true),
        );
        return None;
    }
    if get_value(section, "minecraft_item").is_some() {
        context.diagnostics.warning(
            "RECIPE_EXACT_SERIALIZED_ITEM",
            "A serialized Bukkit ItemStack ExactChoice cannot be reconstructed as a plain CraftEngine item id",
            detail(context, field, true),
        );
        return None;
    }
    context.diagnostics.error(
        "RECIPE_CHOICE_MISSING",
        "Recipe choice has no supported nexo_item, minecraft_type, or tag",
        detail(context, field, false),
    );
    None
}

fn result(section: Option<&JsonObject>, context: &mut RecipeContext) -> Option<JsonObject> {
    let Some(section) = section else {
        context.diagnostics.error("RECIPE_RESULT_MISSING", "Recipe has no result section", detail(context, "result", false));
        return None;
    };
    let id = choice(section, context, "result")?;
    if id.starts_with('#') {
        return None;
    }
    let mut output = JsonObject::new();
    output.insert("id".to_string(), Value::String(id));
    output.insert("count".to_string(), number_json(get_number(section, "amount").unwrap_or(1.0)));
    Some(output)
}

fn category(section: &JsonObject) -> Option<String> {
    get_string(section, "category").map(|value| value.to_lowercase())
}

fn common(section: &JsonObject, context: &mut RecipeContext) -> Option<JsonObject> {
    let output = result(get_object(section, "result"), context)?;
    let mut converted = JsonObject::new();
    converted.insert("result".to_string(), Value::Object(output));
    if let Some(group) = get_string(section, "group") {
        converted.insert("group".to_string(), Value::String(group.to_string()));
    }
    if let Some(category) = category(section).filter(|category| !category.is_empty()) {
        converted.insert("category".to_string(), Value::String(category));
    }
    if get_string(section, "permission").map_or(false, |permission| !permission.is_empty()) {
        context.diagnostics.warning(
            "RECIPE_PERMISSION_MANUAL",
            "Nexo recipe permission has no direct built-in CraftEngine recipe field",
            detail(context, "permission", true),
        );
    }
    Some(converted)
}

pub fn convert_recipe(
    recipe_type: NexoRecipeType,
    id: &str,
    section: &JsonObject,
    namespace: &str,
    diagnostics: &mut DiagnosticBag,
    source: &str,
) -> Option<JsonObject> {
    let mut context = RecipeContext { namespace, source, id, diagnostics };
    let mut converted = common(section, &mut context)?;
    converted.insert("type".to_string(), Value::String(recipe_type.vanilla_type().to_string()));
    match recipe_type {
        NexoRecipeType::Shaped => {
            let shape_values = match get_value(section, "shape") {
                Some(Value::Array(entries)) => entries,
                _ => {
                    context.diagnostics.error(
                        "SHAPED_PATTERN_INVALID",
                        "Nexo shaped recipe shape must be a string list",
                        detail(&context, "shape", false),
                    );
                    return None;
                }
            };
            if !shape_values.iter().all(|entry| entry.is_string()) {
                context.diagnostics.error(
                    "SHAPED_PATTERN_INVALID",
                    "Nexo shaped recipe shape must be a string list",
                    detail(&context, "shape", false),
                );
                return None;
            }
            let shape: Vec<String> = shape_values
                .iter()
                .map(|entry| entry.as_str().expect("validated string").to_string())
                .collect();
            converted.insert("pattern".to_string(), json!(shape));
            let Some(ingredients) = get_object(section, "ingredients") else {
                return None;
            };
            let mut mapped = JsonObject::new();
            for (symbol, raw_choice) in ingredients {
                let Value::Object(raw_choice) = raw_choice else { continue; };
                if let Some(value) = choice(raw_choice, &mut context, &format!("ingredients.{}", symbol)) {
                    let key = symbol.chars().next().map(|character| character.to_string()).unwrap_or_default();
                    mapped.insert(key, Value::String(value));
                }
            }
            let mut used_symbols: Vec<char> = Vec::new();
            for row in &shape {
                for symbol in row.chars() {
                    if symbol != ' ' && !used_symbols.contains(&symbol) {
                        used_symbols.push(symbol);
                    }
                }
            }
            let missing_symbols: Vec<String> = used_symbols
                .iter()
                .filter(|symbol| !mapped.contains_key(&symbol.to_string()))
                .map(|symbol| symbol.to_string())
                .collect();
            if !missing_symbols.is_empty() {
                context.diagnostics.error(
                    "SHAPED_INGREDIENT_MISSING",
                    &format!(
                        "CraftEngine rejects pattern symbols without ingredient mappings: {}",
                        missing_symbols.join(", ")
                    ),
                    detail(&context, "ingredients", false),
                );
                return None;
            }
            converted.insert("ingredients".to_string(), Value::Object(mapped));
        }
        NexoRecipeType::Shapeless => {
            let raw = get_value(section, "ingredients");
            let entries: Vec<&Value> = match raw {
                Some(Value::Array(list)) => list.iter().collect(),
                Some(Value::Object(map)) => map.values().collect(),
                _ => Vec::new(),
            };
            let mut list: Vec<Value> = Vec::new();
            for (index, entry) in entries.into_iter().enumerate() {
                let Value::Object(entry) = entry else { continue; };
                let Some(value) = choice(entry, &mut context, &format!("ingredients.{}", index)) else {
                    continue;
                };
                let amount = get_number(entry, "amount").unwrap_or(1.0).trunc().clamp(1.0, 9.0) as i64;
                for _ in 0..amount {
                    list.push(Value::String(value.clone()));
                }
            }
            converted.insert("ingredients".to_string(), Value::Array(list));
        }
        NexoRecipeType::Furnace | NexoRecipeType::Blasting | NexoRecipeType::Smoking | NexoRecipeType::Campfire => {
            let Some(input) = get_object(section, "input") else { return None; };
            let Some(ingredient) = choice(input, &mut context, "input") else { return None; };
            converted.insert("ingredient".to_string(), Value::String(ingredient));
            converted.insert("experience".to_string(), number_json(get_number(section, "experience").unwrap_or(0.0)));
            converted.insert("time".to_string(), number_json(get_number(section, "cookingTime").unwrap_or(0.0)));
        }
        NexoRecipeType::Stonecutting => {
            let Some(input) = get_object(section, "input") else { return None; };
            let Some(ingredient) = choice(input, &mut context, "input") else { return None; };
            converted.insert("ingredient".to_string(), Value::String(ingredient));
            converted.shift_remove("category");
        }
        NexoRecipeType::Brewing => {
            let input = get_object(section, "input");
            let reagent = get_object(section, "ingredient");
            let (Some(input), Some(reagent)) = (input, reagent) else { return None; };
            let container = choice(input, &mut context, "input");
            let ingredient = choice(reagent, &mut context, "ingredient");
            let (Some(container), Some(ingredient)) = (container, ingredient) else { return None; };
            converted.insert("container".to_string(), Value::String(container));
            converted.insert("ingredient".to_string(), Value::String(ingredient));
            converted.shift_remove("group");
            converted.shift_remove("category");
        }
    }
    Some(converted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_section(value: Value) -> JsonObject {
        value.as_object().unwrap().clone()
    }

    fn convert(recipe_type: NexoRecipeType, value: Value) -> (Option<JsonObject>, DiagnosticBag) {
        let mut diagnostics = DiagnosticBag::new();
        let section = make_section(value);
        let converted = convert_recipe(recipe_type, "demo", &section, "ns", &mut diagnostics, "recipes/demo.yml");
        (converted, diagnostics)
    }

    fn codes(bag: &DiagnosticBag) -> Vec<&str> {
        bag.items.iter().map(|item| item.code.as_str()).collect()
    }

    #[test]
    fn recipe_type_names_and_mapping() {
        assert_eq!(NexoRecipeType::ALL.len(), 8);
        assert_eq!(NexoRecipeType::parse("furnace"), Some(NexoRecipeType::Furnace));
        assert_eq!(NexoRecipeType::parse("bogus"), None);
        for recipe_type in NexoRecipeType::ALL {
            assert_eq!(NexoRecipeType::parse(recipe_type.as_str()), Some(recipe_type));
        }
        assert_eq!(NexoRecipeType::Furnace.vanilla_type(), "smelting");
        assert_eq!(NexoRecipeType::Campfire.vanilla_type(), "campfire_cooking");
    }

    #[test]
    fn shaped_happy_path_keeps_ts_key_order() {
        let (converted, diagnostics) = convert(
            NexoRecipeType::Shaped,
            json!({
                "result": { "nexo_item": "Sword", "amount": 2 },
                "group": "weapons",
                "category": "Tools",
                "permission": "craft.sword",
                "shape": ["AA", " B"],
                "ingredients": {
                    "A": { "nexo_item": "Steel" },
                    "B": { "minecraft_type": "stick" }
                }
            }),
        );
        let converted = converted.expect("shaped recipe converts");
        let keys: Vec<&String> = converted.keys().collect();
        assert_eq!(
            keys,
            vec!["result", "group", "category", "type", "pattern", "ingredients"]
        );
        assert_eq!(converted["result"], json!({ "id": "ns:sword", "count": 2 }));
        assert_eq!(converted["type"], "shaped");
        assert_eq!(converted["category"], "tools");
        assert_eq!(converted["pattern"], json!(["AA", " B"]));
        assert_eq!(converted["ingredients"], json!({ "A": "ns:steel", "B": "minecraft:stick" }));
        assert_eq!(codes(&diagnostics), vec!["RECIPE_PERMISSION_MANUAL"]);
    }

    #[test]
    fn shaped_rejects_invalid_pattern_and_missing_symbols() {
        let (converted, diagnostics) = convert(
            NexoRecipeType::Shaped,
            json!({ "result": { "nexo_item": "x" }, "shape": "AA" }),
        );
        assert!(converted.is_none());
        assert_eq!(codes(&diagnostics), vec!["SHAPED_PATTERN_INVALID"]);

        let (converted, diagnostics) = convert(
            NexoRecipeType::Shaped,
            json!({ "result": { "nexo_item": "x" }, "shape": ["A", 5] }),
        );
        assert!(converted.is_none());
        assert_eq!(codes(&diagnostics), vec!["SHAPED_PATTERN_INVALID"]);

        let (converted, diagnostics) = convert(
            NexoRecipeType::Shaped,
            json!({
                "result": { "nexo_item": "x" },
                "shape": ["AX"],
                "ingredients": { "A": { "nexo_item": "a" }, "Q": "not-an-object" }
            }),
        );
        assert!(converted.is_none());
        assert_eq!(codes(&diagnostics), vec!["SHAPED_INGREDIENT_MISSING"]);
        assert!(diagnostics.items[0].message.ends_with("X"));
    }

    #[test]
    fn cooking_types_map_and_default_experience_time() {
        for (recipe_type, expected) in [
            (NexoRecipeType::Furnace, "smelting"),
            (NexoRecipeType::Blasting, "blasting"),
            (NexoRecipeType::Smoking, "smoking"),
            (NexoRecipeType::Campfire, "campfire_cooking"),
        ] {
            let (converted, diagnostics) = convert(
                recipe_type,
                json!({ "result": { "nexo_item": "out" }, "input": { "minecraft_type": "iron_ore" } }),
            );
            let converted = converted.expect("cooking recipe converts");
            assert_eq!(converted["type"], expected);
            assert_eq!(converted["ingredient"], "minecraft:iron_ore");
            assert_eq!(converted["experience"], 0);
            assert_eq!(converted["time"], 0);
            assert!(!diagnostics.has_errors());
        }
    }

    #[test]
    fn cooking_keeps_raw_experience_and_time() {
        let (converted, _) = convert(
            NexoRecipeType::Furnace,
            json!({
                "result": { "nexo_item": "out" },
                "input": { "minecraft_type": "iron_ore" },
                "experience": 2.5,
                "cookingTime": 200
            }),
        );
        let converted = converted.unwrap();
        assert_eq!(converted["experience"], 2.5);
        assert_eq!(converted["time"], 200);
    }

    #[test]
    fn cooking_without_input_returns_none_silently() {
        let (converted, diagnostics) = convert(
            NexoRecipeType::Furnace,
            json!({ "result": { "nexo_item": "out" } }),
        );
        assert!(converted.is_none());
        assert!(diagnostics.items.is_empty());
    }

    #[test]
    fn shapeless_clamps_amounts_between_1_and_9() {
        let (converted, _) = convert(
            NexoRecipeType::Shapeless,
            json!({
                "result": { "nexo_item": "out" },
                "ingredients": [
                    { "nexo_item": "big", "amount": 15 },
                    { "nexo_item": "tiny", "amount": 0 },
                    { "nexo_item": "frac", "amount": 2.9 },
                    { "minecraft_type": "stick" }
                ]
            }),
        );
        let converted = converted.unwrap();
        let ingredients = converted["ingredients"].as_array().unwrap();
        assert_eq!(ingredients.len(), 9 + 1 + 2 + 1);
        assert_eq!(ingredients[0], "ns:big");
        assert_eq!(ingredients[9], "ns:tiny");
        assert_eq!(ingredients[10], "ns:frac");
        assert_eq!(ingredients[12], "minecraft:stick");
    }

    #[test]
    fn shapeless_accepts_object_ingredients() {
        let (converted, _) = convert(
            NexoRecipeType::Shapeless,
            json!({
                "result": { "nexo_item": "out" },
                "ingredients": {
                    "first": { "nexo_item": "a" },
                    "second": { "nexo_item": "b", "amount": 2 }
                }
            }),
        );
        let converted = converted.unwrap();
        assert_eq!(converted["ingredients"], json!(["ns:a", "ns:b", "ns:b"]));
    }

    #[test]
    fn stonecutting_drops_category_but_keeps_group() {
        let (converted, _) = convert(
            NexoRecipeType::Stonecutting,
            json!({
                "result": { "nexo_item": "out" },
                "group": "stones",
                "category": "Building",
                "input": { "minecraft_type": "stone" }
            }),
        );
        let converted = converted.unwrap();
        assert_eq!(converted["type"], "stonecutting");
        assert_eq!(converted["ingredient"], "minecraft:stone");
        assert_eq!(converted["group"], "stones");
        assert!(!converted.contains_key("category"));
    }

    #[test]
    fn brewing_drops_group_and_category_and_evaluates_both_choices() {
        let (converted, diagnostics) = convert(
            NexoRecipeType::Brewing,
            json!({
                "result": { "nexo_item": "out" },
                "group": "potions",
                "category": "Brewing",
                "input": { "nexo_tag": "some_tag" },
                "ingredient": { "minecraft_type": "blaze_powder" }
            }),
        );
        // The input choice is evaluated (and warned about) even though the
        // recipe is ultimately dropped.
        assert_eq!(codes(&diagnostics), vec!["RECIPE_NEXO_TAG_UNEXPANDED"]);
        assert!(converted.is_none());

        let (converted, _) = convert(
            NexoRecipeType::Brewing,
            json!({
                "result": { "nexo_item": "out" },
                "group": "potions",
                "category": "Brewing",
                "input": { "minecraft_type": "potion" },
                "ingredient": { "minecraft_type": "blaze_powder" }
            }),
        );
        let converted = converted.unwrap();
        assert_eq!(converted["type"], "brewing");
        assert_eq!(converted["container"], "minecraft:potion");
        assert_eq!(converted["ingredient"], "minecraft:blaze_powder");
        assert!(!converted.contains_key("group"));
        assert!(!converted.contains_key("category"));
    }

    #[test]
    fn missing_result_is_reported() {
        let (converted, diagnostics) = convert(NexoRecipeType::Shaped, json!({ "shape": ["A"] }));
        assert!(converted.is_none());
        assert_eq!(codes(&diagnostics), vec!["RECIPE_RESULT_MISSING"]);
    }

    #[test]
    fn tag_result_drops_recipe_without_missing_choice_error() {
        let (converted, diagnostics) = convert(
            NexoRecipeType::Shapeless,
            json!({ "result": { "tag": "stone" } }),
        );
        assert!(converted.is_none());
        assert!(diagnostics.items.is_empty());
    }

    #[test]
    fn choice_priority_matches_nexo() {
        fn choose(value: Value) -> (Option<String>, DiagnosticBag) {
            let mut diagnostics = DiagnosticBag::new();
            let input = make_section(value);
            let mut context = RecipeContext { namespace: "ns", source: "s", id: "i", diagnostics: &mut diagnostics };
            let chosen = choice(&input, &mut context, "f");
            (chosen, diagnostics)
        }

        // nexo_item wins over everything else.
        let (chosen, diagnostics) = choose(json!({ "nexo_item": "Winner", "minecraft_type": "stone", "tag": "#foo" }));
        assert_eq!(chosen.as_deref(), Some("ns:winner"));
        assert!(diagnostics.items.is_empty());

        // Crucible/MMOItems choices have priority and cannot be reconstructed.
        let (chosen, diagnostics) = choose(json!({ "crucible_item": "CRUX", "minecraft_type": "stone" }));
        assert_eq!(chosen, None);
        assert_eq!(codes(&diagnostics), vec!["RECIPE_EXACT_EXTERNAL_CHOICE"]);

        let (chosen, diagnostics) = choose(json!({ "mmoitems_id": "MI" }));
        assert_eq!(chosen, None);
        assert_eq!(codes(&diagnostics), vec!["RECIPE_EXACT_EXTERNAL_CHOICE"]);

        // minecraft_type is next.
        let (chosen, _) = choose(json!({ "minecraft_type": "Stone" }));
        assert_eq!(chosen.as_deref(), Some("minecraft:stone"));

        // tag keeps its # prefix and always resolves against minecraft.
        let (chosen, _) = choose(json!({ "tag": "#stone" }));
        assert_eq!(chosen.as_deref(), Some("#minecraft:stone"));
        let (chosen, _) = choose(json!({ "tag": "custom:foo" }));
        assert_eq!(chosen.as_deref(), Some("#custom:foo"));

        // nexo_tag (even null) and serialized minecraft_item are lossy.
        let (chosen, diagnostics) = choose(json!({ "nexo_tag": null }));
        assert_eq!(chosen, None);
        assert_eq!(codes(&diagnostics), vec!["RECIPE_NEXO_TAG_UNEXPANDED"]);
        let (chosen, diagnostics) = choose(json!({ "minecraft_item": { "type": "stone" } }));
        assert_eq!(chosen, None);
        assert_eq!(codes(&diagnostics), vec!["RECIPE_EXACT_SERIALIZED_ITEM"]);

        // Nothing supported left: hard error.
        let (chosen, diagnostics) = choose(json!({}));
        assert_eq!(chosen, None);
        let tail = &diagnostics.items[diagnostics.items.len() - 1];
        assert_eq!(tail.code, "RECIPE_CHOICE_MISSING");
        assert_eq!(tail.severity, crate::diagnostics::Severity::Error);
    }

    #[test]
    fn empty_group_is_kept_and_count_defaults_to_one() {
        let (converted, _) = convert(
            NexoRecipeType::Shapeless,
            json!({ "result": { "nexo_item": "out" }, "group": "", "ingredients": [] }),
        );
        let converted = converted.unwrap();
        assert_eq!(converted["group"], "");
        assert_eq!(converted["result"]["count"], 1);
    }
}
