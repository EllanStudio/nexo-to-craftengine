//! Port of legacy/test/converter.test.ts lines 490-947: furniture and
//! mechanics semantics (convert_mechanics) plus one Pack.generate_model
//! silence check (read_pack_model).

use serde_json::{json, Value};

use nexo2ce::diagnostics::{DiagnosticBag, Severity};
use nexo2ce::json::JsonObject;
use nexo2ce::mechanics::{convert_mechanics, FurnitureRuntimeSettings, MechanicsConversion};
use nexo2ce::models::{read_pack_model, ModelContext};

fn obj(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        other => panic!("expected JSON object, got {other}"),
    }
}

fn keys(value: &Value) -> Vec<String> {
    value.as_object().expect("object").keys().cloned().collect()
}

fn run_mechanics(
    config: Value,
    target_id: &str,
    base_model: &str,
    diagnostics: &mut DiagnosticBag,
    source: &str,
    item: &str,
    defaults: Option<&JsonObject>,
    runtime: Option<&FurnitureRuntimeSettings>,
) -> MechanicsConversion {
    let config = obj(config);
    convert_mechanics(
        &config,
        target_id,
        Some(base_model),
        diagnostics,
        source,
        item,
        defaults,
        runtime,
    )
}

#[test]
fn furniture_defaults_preserve_nexo_strict_and_fixed_scale_semantics() {
    let mut diagnostics = DiagnosticBag::new();
    let config = json!({
        "Mechanics": { "furniture": {
            "limited_placing": { "floor": true, "roof": false, "wall": false },
            "properties": { "display_transform": "FIXED" },
            "hitbox": { "interactions": ["0,0,0 1,2"] },
        } },
    });
    let converted = run_mechanics(config, "demo:chair", "demo:item/chair", &mut diagnostics, "fixture.yml", "chair", None, None);
    let behavior = Value::Object(converted.behavior[0].clone());
    assert_eq!(behavior["rules"]["ground"]["rotation"], "eight");
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    assert_eq!(furniture["loot"], json!({
        "pools": [{ "rolls": 1, "entries": [{ "type": "furniture_item", "item": "demo:chair" }] }],
    }));
    let variants = &furniture["variants"];
    assert_eq!(keys(variants), vec!["ground"]);
    let element = &variants["ground"]["elements"][0];
    assert_eq!(element["scale"], "0.5,0.5,0.5");
    assert_eq!(element["pitch"], -90);
    assert_eq!(element.get("position"), None);
}

#[test]
fn furniture_displays_inherit_dyed_color_from_the_placed_source_item() {
    let mut diagnostics = DiagnosticBag::new();
    let converted = run_mechanics(json!({ "Mechanics": { "furniture": {
        "limited_placing": { "floor": true, "roof": false, "wall": false },
        "properties": { "display_transform": "FIXED" },
        "hitbox": {},
    } } }), "demo:dyed_chair", "demo:item/dyed_chair", &mut diagnostics, "fixture.yml", "dyed_chair", None, None);
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    let element = &furniture["variants"]["ground"]["elements"][0];

    // CraftEngine 26.8 copies the actual placed ItemStack component through this
    // tint source instead of rebuilding the default, uncolored display item.
    assert_eq!(element["tint_source"], json!(["minecraft:dyed_color"]));
    assert_eq!(diagnostics.items.len(), 0);
}

#[test]
fn limited_placing_preserves_nexo_nested_false_default_plane_semantics() {
    let mut diagnostics = DiagnosticBag::new();
    let config = json!({ "Mechanics": { "furniture": {
        "limited_placing": { "floor": false, "roof": true },
        "properties": { "offset_against_blocks": false },
        "hitbox": {},
    } } });
    let converted = run_mechanics(config, "demo:x", "demo:item/x", &mut diagnostics, "fixture.yml", "x", None, None);
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    assert_eq!(keys(&furniture["variants"]), vec!["ceiling", "wall"]);
}

#[test]
fn fixed_floor_roof_quarter_turns_use_ce_stable_equivalent_transforms() {
    let mut diagnostics = DiagnosticBag::new();
    let config = json!({ "Mechanics": { "furniture": {
        "limited_placing": { "floor": true, "roof": true, "wall": false },
        "properties": { "display_transform": "FIXED", "offset_against_blocks": false },
        "hitbox": {
            "interaction": ["1,2,3 1,2"],
            "shulker": ["1,2,3 bad bad down"],
            "ghast": ["1,2,3 bad true"],
        },
        "seats": ["1,0.6,2"],
    } } });
    let converted = run_mechanics(config, "demo:x", "demo:item/x", &mut diagnostics, "fixture.yml", "x", None, None);
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    let ground = &furniture["variants"]["ground"];
    let ceiling = &furniture["variants"]["ceiling"];
    let ground_element = &ground["elements"][0];
    let ceiling_element = &ceiling["elements"][0];
    assert_eq!(ground_element["pitch"], 90);
    assert_eq!(ground_element.get("yaw"), None);
    assert_eq!(ground_element["rotation"], "0,1,0,0");
    assert_eq!(ground_element.get("position"), None);
    assert_eq!(ceiling_element["pitch"], -90);
    assert_eq!(ceiling_element.get("yaw"), None);
    assert_eq!(ceiling_element["rotation"], "0,1,0,0");
    assert_eq!(ceiling_element["position"], "0,-0.01,0");

    let ground_hitboxes = ground["hitboxes"].as_array().expect("ground hitboxes");
    assert_eq!(ground_hitboxes[0]["position"], "-1,1.5,-3"); // Nexo packet origin is display Y - 0.5.
    assert_eq!(ground_hitboxes[1]["position"], "-1,2,-3"); // Shulker uses exact display base.
    assert_eq!(ground_hitboxes[1].get("scale"), None); // CE parser supplies Nexo's fallback scale 1.
    assert_eq!(ground_hitboxes[1].get("peek"), None); // CE parser supplies fallback peek 0.
    assert_eq!(ground_hitboxes[2]["scale"], 0.25);
    let ceiling_hitboxes = ceiling["hitboxes"].as_array().expect("ceiling hitboxes");
    assert_eq!(ceiling_hitboxes[0]["position"], "-1,1.49,-3");
    assert!(diagnostics.items.iter().any(|entry| entry.code == "GHAST_VISIBLE_UNSUPPORTED"));

    assert_eq!(ground_hitboxes.len(), 3, "existing hitboxes must replace the occluded tiny seat proxy");
    for hitbox in ground_hitboxes {
        assert_eq!(hitbox["seats"], json!(["-1,0,-2"]));
    }
    assert_eq!(ceiling_hitboxes.len(), 3);
    for hitbox in ceiling_hitboxes {
        assert_eq!(hitbox["seats"], json!(["-1,-0.01,-2"]));
    }
}

#[test]
fn fixed_quarter_turn_recomposition_falls_back_for_non_commuting_display_transforms() {
    let mut diagnostics = DiagnosticBag::new();
    let converted = run_mechanics(json!({ "Mechanics": { "furniture": {
        "limited_placing": { "floor": false, "roof": true, "wall": false },
        "properties": { "display_transform": "FIXED", "translation": "1,0,0" },
        "hitbox": {},
    } } }), "demo:x", "demo:item/x", &mut diagnostics, "fixture.yml", "x", None, None);
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    let element = &furniture["variants"]["ceiling"]["elements"][0];
    assert_eq!(element["pitch"], 90);
    assert_eq!(element["yaw"], -180);
    assert_eq!(element.get("rotation"), None);
}

#[test]
fn furniture_global_default_properties_merge_before_item_overrides() {
    let mut diagnostics = DiagnosticBag::new();
    let mut config = json!({ "Mechanics": { "furniture": {
        "limited_placing": { "floor": false, "roof": false, "wall": false },
        "properties": { "scale": "0.8,0.8,0.8" },
        "hitbox": {},
    } } });
    let defaults = obj(json!({
        "display_transform": "FIXED", "translation": "1,2,3", "scale": "0.2,0.3,0.4", "offset_against_blocks": false,
    }));
    let converted = run_mechanics(config.clone(), "demo:x", "demo:item/x", &mut diagnostics, "fixture.yml", "x", Some(&defaults), None);
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    assert!(keys(&furniture["variants"]).is_empty());
    // Re-enable one canonical plane to inspect the merged element.
    config["Mechanics"]["furniture"]["limited_placing"] = json!({ "floor": true, "roof": false, "wall": false });
    let placed = run_mechanics(config, "demo:x", "demo:item/x", &mut diagnostics, "fixture.yml", "x", Some(&defaults), None);
    let placed_furniture = Value::Object(placed.furniture.expect("furniture"));
    let element = &placed_furniture["variants"]["ground"]["elements"][0];
    assert_eq!(element["display_transform"], "fixed");
    assert_eq!(element["translation"], "1,2,3");
    assert_eq!(element["scale"], "0.8,0.8,0.8");
    assert!(!diagnostics.items.iter().any(|entry| entry.code == "FURNITURE_PARTIAL_BLOCK_OFFSET_DYNAMIC"));
}

#[test]
fn nexo_furniture_lights_become_ce_glowing_variants_with_persistent_right_click_toggling() {
    let mut diagnostics = DiagnosticBag::new();
    let config = json!({ "Mechanics": { "furniture": {
        "limited_placing": { "floor": true, "roof": false, "wall": false },
        "properties": { "display_transform": "FIXED", "offset_against_blocks": false },
        "hitbox": { "barriers": ["0,0,0"] },
        "lights": {
            "toggleable": true,
            "lights": ["0,1,0 14", "1..2,0,-1 12", "0,0,0 15"],
        },
    } } });
    let converted = run_mechanics(config, "demo:lamp", "demo:item/lamp", &mut diagnostics, "fixture.yml", "lamp", None, None);
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    let variants = &furniture["variants"];
    assert_eq!(keys(variants), vec!["ground", "ground_unlit"]);
    assert_eq!(variants["ground_unlit"], variants["ground"]);
    assert!(!keys(variants).iter().any(|name| name.starts_with("_nexo_")));
    let ground_hitboxes = variants["ground"]["hitboxes"].as_array().expect("ground hitboxes");
    assert!(!ground_hitboxes.iter().any(|entry| entry.get("_nexo_barrier").is_some()));

    assert_eq!(furniture.get("behavior"), None, "official default furniture uses the plural behaviors key");
    let behavior = &furniture["behaviors"];
    assert_eq!(behavior["type"], "glowing_furniture");
    let lights = &behavior["variants"]["ground"];
    assert_eq!(lights, &json!([
        { "position": "0,1,0", "level": 14 },
        { "position": "-1,0,1", "level": 12 },
        { "position": "-2,0,1", "level": 12 },
    ]));
    assert_eq!(behavior["variants"].get("ground_unlit"), None);
    let events = furniture["events"].as_array().expect("events");
    let right_click = events.iter().find(|event| event["on"] == "right_click").expect("right_click event");
    let functions = right_click["functions"].as_array().expect("functions");
    let cases = functions[0]["cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 2);
    assert!(cases.iter().any(|entry| entry["when"] == "ground"));
    assert!(cases.iter().any(|entry| entry["when"] == "ground_unlit"));
    assert_eq!(functions[1]["type"], "update_interaction_tick");
    let semantics = Value::Object(converted.semantics);
    assert_eq!(semantics["furniture"]["lights"], 3);
    assert_eq!(semantics["furniture"]["toggleable_light"], true);
    assert!(diagnostics.items.iter().any(|entry| entry.code == "NEXO_LIGHT_BARRIER_OVERLAP_IGNORED"));
    let requirement = diagnostics
        .items
        .iter()
        .find(|entry| entry.code == "CRAFTENGINE_FURNITURE_LIGHT_SYSTEM_REQUIRED")
        .expect("light system requirement");
    let context = requirement.context.as_ref().expect("requirement context");
    assert_eq!(context.get("setting"), Some(&json!("furniture.light-system.enable")));
    assert_eq!(context.get("required_value"), Some(&json!(true)));
    assert_eq!(requirement.lossy, false);
}

#[test]
fn furniture_light_positions_follow_only_explicit_placement_anchors() {
    let mut diagnostics = DiagnosticBag::new();
    let fixed_conversion = run_mechanics(json!({ "Mechanics": { "furniture": {
        "limited_placing": { "floor": true, "roof": true, "wall": true },
        "properties": { "display_transform": "FIXED" },
        "hitbox": { "barriers": ["0,0,0"] },
        "lights": { "lights": ["0,1,0 14"] },
    } } }), "demo:fixed_lamp", "demo:item/fixed_lamp", &mut diagnostics, "fixture.yml", "fixed_lamp", None, None);
    let fixed = Value::Object(fixed_conversion.furniture.expect("furniture"));
    let fixed_behavior = &fixed["behaviors"];
    assert_eq!(fixed_behavior["type"], "glowing_furniture");
    let fixed_lights = &fixed_behavior["variants"];
    assert_eq!(fixed_lights["ground"], json!([{ "position": "0,1,0", "level": 14 }]));
    assert_eq!(fixed_lights["ceiling"], json!([{ "position": "0,0.99,0", "level": 14 }]));
    assert_eq!(fixed_lights["wall"], json!([{ "position": "0,1,0.01", "level": 14 }]));
    assert_eq!(keys(fixed_lights), vec!["ground", "ceiling", "wall"]);

    let non_fixed_conversion = run_mechanics(json!({ "Mechanics": { "furniture": {
        "limited_placing": { "floor": true, "roof": false, "wall": false },
        "properties": { "display_transform": "HEAD" },
        "hitbox": { "interactions": ["0,0,0 1,1"] },
        "lights": { "lights": ["0,1,0 10"] },
    } } }), "demo:head_lamp", "demo:item/head_lamp", &mut diagnostics, "fixture.yml", "head_lamp", None, None);
    let non_fixed = Value::Object(non_fixed_conversion.furniture.expect("furniture"));
    let non_fixed_behavior = &non_fixed["behaviors"];
    assert_eq!(non_fixed_behavior["lights"], json!([{ "position": "0,1.5,0", "level": 10 }]));
    assert_eq!(non_fixed_behavior.get("variants"), None);
}

#[test]
fn rotatable_false_is_exact_and_scalar_true_uses_native_ce_rotation() {
    let mut disabled_diagnostics = DiagnosticBag::new();
    let disabled = run_mechanics(
        json!({ "Mechanics": { "furniture": { "rotatable": false } } }),
        "demo:still", "demo:item/still", &mut disabled_diagnostics, "fixture.yml", "still", None, None,
    );
    let disabled_furniture = Value::Object(disabled.furniture.expect("furniture"));
    assert_eq!(disabled_furniture.get("events"), None);
    let disabled_semantics = Value::Object(disabled.semantics);
    assert_eq!(disabled_semantics["furniture"]["rotatable"], false);
    assert!(!disabled_diagnostics.items.iter().any(|entry| entry.field.as_deref().is_some_and(|field| field.ends_with(".rotatable"))));

    let mut diagnostics = DiagnosticBag::new();
    let runtime = FurnitureRuntimeSettings {
        default_rotatable_on_sneak: Some(true),
        rotation_gamemodes: Some(vec!["SURVIVAL".to_string(), "ADVENTURE".to_string()]),
    };
    let converted = run_mechanics(
        json!({ "Mechanics": { "furniture": { "rotatable": true, "restricted_rotation": "NONE" } } }),
        "demo:turning", "demo:item/turning", &mut diagnostics, "fixture.yml", "turning", None, Some(&runtime),
    );
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    let functions = furniture["events"][0]["functions"].as_array().expect("functions");
    let types: Vec<&Value> = functions.iter().map(|entry| &entry["type"]).collect();
    assert_eq!(types, vec![&json!("update_interaction_tick"), &json!("rotate_furniture")]);
    assert_eq!(functions[1]["degree"], 22.5);
    let conditions = functions[1]["conditions"].as_array().expect("conditions");
    assert_eq!(conditions[0]["type"], "expression");
    let gamemodes: Vec<Value> = conditions[1]["terms"]
        .as_array()
        .expect("terms")
        .iter()
        .map(|term| term["value2"].clone())
        .collect();
    assert_eq!(gamemodes, vec![json!("SURVIVAL"), json!("ADVENTURE")]);
    assert_eq!(functions[0]["conditions"], functions[1]["conditions"]);
    let semantics = Value::Object(converted.semantics);
    assert_eq!(semantics["furniture"]["rotation_on_sneak"], true);
}

#[test]
fn nested_rotatable_toggleable_light_and_seats_preserve_nexo_interaction_order() {
    let mut diagnostics = DiagnosticBag::new();
    let runtime = FurnitureRuntimeSettings {
        default_rotatable_on_sneak: Some(true),
        rotation_gamemodes: Some(Vec::new()),
    };
    let converted = run_mechanics(json!({ "Mechanics": { "furniture": {
        "rotatable": { "rotatable": true, "on_sneak": false },
        "restricted_rotation": "VERY_STRICT",
        "seats": ["0,0.6,0"],
        "hitbox": { "interactions": ["0,0,0 1,1"] },
        "lights": { "toggleable": true, "lights": ["0,1,0 12"] },
    } } }), "demo:chair_lamp", "demo:item/chair_lamp", &mut diagnostics, "fixture.yml", "chair_lamp", None, Some(&runtime));
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    let functions = furniture["events"][0]["functions"].as_array().expect("functions");
    let types: Vec<&Value> = functions.iter().map(|entry| &entry["type"]).collect();
    assert_eq!(types, vec![
        &json!("when"),
        &json!("update_interaction_tick"),
        &json!("update_interaction_tick"),
        &json!("rotate_furniture"),
    ]);
    let light_conditions = functions[1]
        .get("conditions")
        .and_then(|conditions| conditions.as_array())
        .expect("light conditions");
    assert_eq!(light_conditions[0]["type"], "expression");
    assert_eq!(functions[2]["conditions"][0]["type"], "!expression");
    assert_eq!(functions[2]["conditions"][1]["terms"][0]["value2"], "__NEXO_NO_GAMEMODE__");
    assert_eq!(functions[3]["degree"], 45);
    assert_eq!(functions[3].get("on_failure"), None);
}

#[test]
fn seat_only_furniture_keeps_a_tiny_clickable_fallback_proxy() {
    let mut diagnostics = DiagnosticBag::new();
    let converted = run_mechanics(json!({ "Mechanics": { "furniture": {
        "limited_placing": { "floor": true, "roof": false, "wall": false },
        "properties": { "display_transform": "FIXED" },
        "hitbox": {},
        "seats": ["0,0.6,0"],
    } } }), "demo:seat_only", "demo:item/seat_only", &mut diagnostics, "fixture.yml", "seat_only", None, None);
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    let hitboxes = furniture["variants"]["ground"]["hitboxes"].as_array().expect("hitboxes");
    assert_eq!(hitboxes.len(), 1);
    let hitbox = &hitboxes[0];
    assert_eq!(
        json!({
            "type": hitbox["type"],
            "position": hitbox["position"],
            "width": hitbox["width"],
            "height": hitbox["height"],
            "seats": hitbox["seats"],
        }),
        json!({
            "type": "interaction",
            "position": "0,0.6,0",
            "width": 0.1,
            "height": 0.1,
            "seats": ["0,0,0"],
        })
    );
}

#[test]
fn wall_barriers_stay_block_centered_while_the_fixed_model_uses_nexo_wall_offset() {
    let mut diagnostics = DiagnosticBag::new();
    let converted = run_mechanics(json!({ "Mechanics": { "furniture": {
        "limited_placing": { "floor": false, "roof": false, "wall": true },
        "properties": { "display_transform": "FIXED", "scale": "0.5,0.5,0.5" },
        "hitbox": { "interactions": ["0,0,0 1,1"], "barriers": ["0,0,0"] },
        "seats": ["0,0.6,0"],
    } } }), "demo:wall", "demo:item/wall", &mut diagnostics, "fixture.yml", "wall", None, None);
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    let wall = &furniture["variants"]["wall"];
    let element = &wall["elements"][0];
    let hitboxes = wall["hitboxes"].as_array().expect("hitboxes");
    let interaction = hitboxes.iter().find(|entry| entry["type"] == "interaction").expect("interaction hitbox");
    let barrier = hitboxes.iter().find(|entry| entry["type"] == "shulker").expect("barrier hitbox");
    assert_eq!(element["position"], "0,0,0.01");
    assert_eq!(interaction["position"], "0,-0.5,0.01");
    assert_eq!(barrier["position"], "0,-0.5,0.5");
    for hitbox in hitboxes {
        assert_eq!(hitbox["seats"], json!(["0,0,0.01"]));
    }
    assert_eq!(keys(&furniture["variants"]), vec!["wall"]);
    let wall_events = furniture.get("events").and_then(|events| events.as_array()).cloned().unwrap_or_default();
    assert!(!wall_events.iter().any(|event| event["on"] == "place"));
    assert!(!diagnostics.items.iter().any(|entry| entry.code == "FURNITURE_WALL_SUPPORT_OFFSET_DYNAMIC"));

    let no_offset = run_mechanics(json!({ "Mechanics": { "furniture": {
        "limited_placing": { "floor": false, "roof": false, "wall": true },
        "properties": { "display_transform": "FIXED", "scale": "0.5,0.5,0.5", "offset_against_blocks": false },
        "hitbox": { "interactions": ["0,0,0 1,1"], "barriers": ["0,0,0"] },
    } } }), "demo:centered-wall", "demo:item/centered-wall", &mut diagnostics, "fixture.yml", "centered-wall", None, None);
    let no_offset_furniture = Value::Object(no_offset.furniture.expect("furniture"));
    let no_offset_wall = &no_offset_furniture["variants"]["wall"];
    assert_eq!(no_offset_wall["elements"][0]["position"], "0,0,0.01");
    let no_offset_hitboxes = no_offset_wall["hitboxes"].as_array().expect("hitboxes");
    let no_offset_interaction = no_offset_hitboxes
        .iter()
        .find(|entry| entry["type"] == "interaction")
        .expect("interaction hitbox");
    assert_eq!(no_offset_interaction["position"], "0,-0.5,0.01");
    assert_eq!(keys(&no_offset_furniture["variants"]), vec!["wall"]);
}

#[test]
fn ceiling_barriers_use_the_target_block_bottom_while_displays_keep_nexo_clearance() {
    let mut diagnostics = DiagnosticBag::new();
    let converted = run_mechanics(json!({ "Mechanics": { "furniture": {
        "limited_placing": { "floor": false, "roof": true, "wall": false },
        "properties": { "display_transform": "FIXED" },
        "hitbox": { "barriers": ["0,0,0"] },
    } } }), "demo:ceiling", "demo:item/ceiling", &mut diagnostics, "fixture.yml", "ceiling", None, None);
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    let variants = &furniture["variants"];
    let ceiling = &variants["ceiling"];
    assert_eq!(ceiling["elements"][0]["position"], "0,-0.01,0");
    assert_eq!(ceiling["hitboxes"][0]["position"], "0,-1,0");
    assert_eq!(keys(variants), vec!["ceiling"]);
    let events = furniture.get("events").and_then(|events| events.as_array()).cloned().unwrap_or_default();
    assert!(!events.iter().any(|event| event["on"] == "place"));
}

#[test]
fn autumn_signpost_haystack_and_both_streamers_retain_nexo_world_space_semantics() {
    let mut diagnostics = DiagnosticBag::new();
    let barrier_source = json!({ "Mechanics": { "furniture": {
        "rotatable": false,
        "limited_placing": { "roof": true, "floor": true, "wall": true },
        "properties": { "display_transform": "FIXED", "scale": "0.5,0.5,0.5" },
        "hitbox": { "barriers": ["0,0,0"] },
    } } });
    let signpost_conversion = run_mechanics(
        barrier_source.clone(),
        "lanshan_autumn_field:field_signpost", "lanshan_autumn_field:item/field_signpost",
        &mut diagnostics, "autumn.yml", "field_signpost", None, None,
    );
    let signpost = Value::Object(signpost_conversion.furniture.expect("furniture"));

    let mut haystack_source = barrier_source.clone();
    haystack_source["Mechanics"]["furniture"]["seats"] = json!(["0.0,1.0,0.0"]);
    let haystack_conversion = run_mechanics(
        haystack_source,
        "lanshan_autumn_field:field_haystack", "lanshan_autumn_field:item/field_haystack",
        &mut diagnostics, "autumn.yml", "field_haystack", None, None,
    );
    let haystack = Value::Object(haystack_conversion.furniture.expect("furniture"));

    let mut chair_source = barrier_source.clone();
    chair_source["Mechanics"]["furniture"]["limited_placing"] = json!({ "roof": false, "floor": true, "wall": false });
    chair_source["Mechanics"]["furniture"]["seats"] = json!(["0.0,0.6,0.0"]);
    let chair_conversion = run_mechanics(
        chair_source,
        "lanshan_autumn_field:field_chair", "lanshan_autumn_field:item/field_chair",
        &mut diagnostics, "autumn.yml", "field_chair", None, None,
    );
    let chair = Value::Object(chair_conversion.furniture.expect("furniture"));
    let chair_ground_hitboxes = chair["variants"]["ground"]["hitboxes"].as_array().expect("chair ground hitboxes");
    assert_eq!(chair_ground_hitboxes.len(), 1, "field_chair must not retain an occluded 0.1x0.1 seat proxy");
    assert_eq!(chair_ground_hitboxes[0]["seats"], json!(["0,0,0"]), "CE +0.6 must land the player at Nexo Y=0.6");

    let haystack_variants = &haystack["variants"];
    assert_eq!(haystack_variants["ground"]["hitboxes"][0]["seats"], json!(["0,0.4,0"]));
    assert_eq!(haystack_variants["ceiling"]["hitboxes"][0]["seats"], json!(["0,0.39,0"]));
    assert_eq!(haystack_variants["wall"]["hitboxes"][0]["seats"], json!(["0,0.4,0.01"]));
    assert_eq!(keys(haystack_variants), vec!["ground", "ceiling", "wall"]);

    for converted in [&signpost, &haystack] {
        let variants = &converted["variants"];
        let ground_element = &variants["ground"]["elements"][0];
        let ceiling_element = &variants["ceiling"]["elements"][0];
        assert_eq!(ground_element.get("position"), None);
        assert_eq!(ground_element["pitch"], 90);
        assert_eq!(ground_element.get("yaw"), None);
        assert_eq!(ground_element["rotation"], "0,1,0,0");
        assert_eq!(variants["ground"]["hitboxes"][0]["position"], "0,0,0");
        assert_eq!(ceiling_element["position"], "0,-0.01,0");
        assert_eq!(ceiling_element["pitch"], -90);
        assert_eq!(ceiling_element.get("yaw"), None);
        assert_eq!(ceiling_element["rotation"], "0,1,0,0");
        assert_eq!(variants["ceiling"]["hitboxes"][0]["position"], "0,-1,0");
        assert_eq!(variants["wall"]["elements"][0]["position"], "0,0,0.01");
        assert_eq!(variants["wall"]["hitboxes"][0]["position"], "0,-0.5,0.5");
    }

    let streamer_source = json!({ "Mechanics": { "furniture": {
        "rotatable": false,
        "limited_placing": { "roof": false, "floor": false, "wall": true },
        "properties": { "display_transform": "FIXED", "scale": "0.5,0.5,0.5" },
        "hitbox": { "interactions": ["0,0,0 1.0,1.0"] },
    } } });
    let large_conversion = run_mechanics(
        streamer_source.clone(),
        "lanshan_autumn_field:large_crop_streamer", "lanshan_autumn_field:item/large_crop_streamer",
        &mut diagnostics, "autumn.yml", "large_crop_streamer", None, None,
    );
    let small_conversion = run_mechanics(
        streamer_source,
        "lanshan_autumn_field:small_crop_streamer", "lanshan_autumn_field:item/small_crop_streamer",
        &mut diagnostics, "autumn.yml", "small_crop_streamer", None, None,
    );
    let large = Value::Object(large_conversion.furniture.expect("furniture"));
    let small = Value::Object(small_conversion.furniture.expect("furniture"));
    let wall = &large["variants"]["wall"];
    assert_eq!(wall["elements"][0]["position"], "0,0,0.01");
    assert_eq!(wall["hitboxes"][0]["position"], "0,-0.5,0.01");
    assert_eq!(large["settings"]["item"], "lanshan_autumn_field:large_crop_streamer");
    assert_eq!(small["settings"]["item"], "lanshan_autumn_field:small_crop_streamer");
}

#[test]
fn autumn_ceiling_lantern_keeps_nexo_separate_visual_barrier_and_light_anchors() {
    let mut diagnostics = DiagnosticBag::new();
    let converted = run_mechanics(json!({ "Mechanics": { "furniture": {
        "rotatable": false,
        "limited_placing": { "roof": true, "floor": false, "wall": false },
        "properties": { "display_transform": "FIXED", "scale": "0.5,0.5,0.5" },
        "hitbox": { "barriers": ["0,0,0"] },
        "lights": { "toggleable": true, "lights": ["0,-1,0 14"] },
    } } }), "lanshan_autumn_field:field_lantern_ceiling", "lanshan_autumn_field:item/field_lantern_ceiling", &mut diagnostics, "autumn.yml", "field_lantern_ceiling", None, None);
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    let ceiling = &furniture["variants"]["ceiling"];
    let element = &ceiling["elements"][0];
    let barrier = &ceiling["hitboxes"][0];
    assert_eq!(element["position"], "0,-0.01,0");
    assert_eq!(element["pitch"], -90);
    assert_eq!(element.get("yaw"), None);
    assert_eq!(element["rotation"], "0,1,0,0");
    assert_eq!(element["display_transform"], "fixed");
    assert_eq!(barrier, &json!({ "type": "shulker", "position": "0,-1,0", "interaction_entity": false }));
    let glowing = &furniture["behaviors"];
    assert_eq!(glowing["type"], "glowing_furniture");
    assert_eq!(glowing["variants"]["ceiling"], json!([{ "position": "0,-1.01,0", "level": 14 }]));
    let codes: Vec<&str> = diagnostics.items.iter().map(|entry| entry.code.as_str()).collect();
    assert_eq!(codes, vec!["CRAFTENGINE_FURNITURE_LIGHT_SYSTEM_REQUIRED"]);
}

#[test]
fn barrier_furniture_keeps_one_concise_base_placement_without_generated_voxel_profiles() {
    let mut diagnostics = DiagnosticBag::new();
    let converted = run_mechanics(json!({ "Mechanics": { "furniture": {
        "rotatable": false,
        "limited_placing": { "floor": true, "roof": false, "wall": false },
        "properties": { "display_transform": "FIXED", "offset_against_blocks": false },
        "hitbox": { "barriers": ["0,0,0"] },
        "seats": ["0,0.6,0"],
    } } }), "demo:concise", "demo:item/concise", &mut diagnostics, "fixture.yml", "concise", None, None);
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    let variants = &furniture["variants"];
    assert_eq!(keys(variants), vec!["ground"]);
    let ground = &variants["ground"];
    assert_eq!(ground["hitboxes"][0]["position"], "0,0,0");
    assert_eq!(ground["hitboxes"][0]["seats"], json!(["0,0,0"]));
    assert_eq!(furniture.get("events"), None);
    let serialized = serde_json::to_string(&furniture).expect("serialize furniture");
    assert!(!serialized.contains("_nexo_"));
    assert!(!serialized.contains("<arg:position.y>"));
}

#[test]
fn pack_generate_model_is_matched_silently_because_nexo_does_not_parse_it_as_the_decision_key() {
    let mut diagnostics = DiagnosticBag::new();
    let pack = obj(json!({ "generate_model": false, "model": "demo:item/existing" }));
    let model = {
        let mut context = ModelContext {
            source: "fixture.yml".to_string(),
            item: "demo".to_string(),
            diagnostics: &mut diagnostics,
            model_aliases: None,
        };
        read_pack_model(Some(&pack), "demo", &mut context)
    };
    assert_eq!(model.base.path, "demo:item/existing");
    assert!(!diagnostics.items.iter().any(|entry| entry.code == "NEXO_GENERATE_MODEL_IGNORED"));
}

#[test]
fn shulker_normalized_length_uses_ce_sine_squared_inverse_not_linear_interpolation() {
    let mut diagnostics = DiagnosticBag::new();
    let config = json!({ "Mechanics": { "furniture": { "hitbox": { "shulkers": ["0,0,0 2 1.25 DOWN false"] } } } });
    let converted = run_mechanics(config, "demo:x", "demo:item/x", &mut diagnostics, "fixture.yml", "x", None, None);
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    let hitbox = &furniture["variants"]["ground"]["hitboxes"][0];
    assert_eq!(hitbox["scale"], 2);
    assert_eq!(hitbox["peek"], 33);
    assert_eq!(hitbox["direction"], "down");
}

#[test]
fn barrier_mapping_uses_ce_defaults_without_verbose_hitbox_boilerplate() {
    let mut diagnostics = DiagnosticBag::new();
    let config = json!({ "Mechanics": { "furniture": { "hitbox": { "barriers": ["0,0,0"] } } } });
    let converted = run_mechanics(config, "demo:x", "demo:item/x", &mut diagnostics, "fixture.yml", "x", None, None);
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    let hitbox = &furniture["variants"]["ground"]["hitboxes"][0];
    assert_eq!(hitbox, &json!({ "type": "shulker", "position": "0,0.5,0", "interaction_entity": false }));
    assert!(!diagnostics.items.iter().any(|entry| {
        entry.code.contains("BARRIER") && entry.field.as_deref().is_some_and(|field| field.contains("hitbox.barriers"))
    }));
}

#[test]
fn oversized_barrier_ranges_fail_safely_without_eager_cartesian_expansion() {
    let mut diagnostics = DiagnosticBag::new();
    let converted = run_mechanics(json!({ "Mechanics": { "furniture": {
        "hitbox": { "barriers": ["0..1000000,0..1000000,0..1000000"] },
    } } }), "demo:unsafe", "demo:item/unsafe", &mut diagnostics, "fixture.yml", "unsafe", None, None);
    let furniture = Value::Object(converted.furniture.expect("furniture"));
    let hitboxes = furniture["variants"]["ground"]["hitboxes"].as_array().expect("hitboxes");
    assert_eq!(hitboxes.len(), 0);
    assert!(diagnostics.items.iter().any(|entry| entry.code == "BARRIER_RANGE_TOO_LARGE" && entry.severity == Severity::Error));
}
