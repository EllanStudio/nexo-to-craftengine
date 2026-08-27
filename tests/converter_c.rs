//! Port of legacy/test/converter.test.ts lines 948..1334 (end of file).
//!
//! Covers: custom block model/loot rules, recipe field mapping, sound mapping,
//! author namespace inference, model typo alias recovery, ambiguous alias
//! rejection, end-to-end globals-driven rotation, end-to-end resource copy +
//! graph audit, bbmodel blueprint relocation, modern tints/player heads,
//! attribute/PDC processors, and glyph code point allocation/tag rewriting.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

use nexo2ce::converter::{convert, ConvertOptions};
use nexo2ce::diagnostics::DiagnosticBag;
use nexo2ce::glyphs::{convert_glyphs, rewrite_glyph_tags};
use nexo2ce::io::load_yaml;
use nexo2ce::items::{convert_item, ItemOptions, ResolvedItem};
use nexo2ce::json::JsonObject;
use nexo2ce::mechanics::convert_mechanics;
use nexo2ce::model_aliases::discover_model_aliases;
use nexo2ce::models::{convert_models, read_pack_model, ModelContext};
use nexo2ce::recipes::{convert_recipe, NexoRecipeType};
use nexo2ce::sounds::convert_sounds;
use nexo2ce::source_namespace::infer_author_namespace_from_bundle_paths;
use nexo2ce::{ClientMode, CmdPolicy};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "nexo2ce-{}-{}-{}",
            name,
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn obj(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        other => panic!("expected a JSON object, got {other}"),
    }
}

fn lines(entries: &[&str]) -> String {
    entries.join("\n")
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent directories");
    }
    std::fs::write(&path, contents).expect("write fixture file");
}

fn read_output_yaml(path: &Path) -> Value {
    let mut diagnostics = DiagnosticBag::new();
    load_yaml(path, &mut diagnostics).unwrap_or_else(|| panic!("failed to load {}", path.display()))
}

fn has_code(diagnostics: &DiagnosticBag, code: &str) -> bool {
    diagnostics.items.iter().any(|entry| entry.code == code)
}

#[test]
fn custom_blocks_require_model_and_never_add_wrong_self_loot() {
    let mut missing_diagnostics = DiagnosticBag::new();
    let empty_block = obj(json!({ "Mechanics": { "noteblock": {} } }));
    let missing = convert_mechanics(
        &empty_block,
        "demo:block",
        None,
        &mut missing_diagnostics,
        "fixture.yml",
        "block",
        None,
        None,
    );
    assert!(missing.block.is_none());
    assert!(missing.behavior.is_empty());
    assert!(has_code(&missing_diagnostics, "BLOCK_MODEL_MISSING"));

    let mut custom_diagnostics = DiagnosticBag::new();
    let custom_drop = obj(json!({ "Mechanics": { "noteblock": {
        "drop": { "loots": [{ "minecraft_type": "DIAMOND", "probability": 1, "amount": 1 }] },
    } } }));
    let custom = convert_mechanics(
        &custom_drop,
        "demo:block",
        Some("demo:block/model"),
        &mut custom_diagnostics,
        "fixture.yml",
        "block",
        None,
        None,
    );
    assert!(custom.block.as_ref().and_then(|block| block.get("loot")).is_none());
    assert!(has_code(&custom_diagnostics, "CUSTOM_BLOCK_DROP_MANUAL"));

    let mut self_diagnostics = DiagnosticBag::new();
    let self_loot = convert_mechanics(
        &empty_block,
        "demo:block",
        Some("demo:block/model"),
        &mut self_diagnostics,
        "fixture.yml",
        "block",
        None,
        None,
    );
    assert_eq!(
        self_loot.block.as_ref().and_then(|block| block.get("loot")),
        Some(&json!({
            "pools": [{
                "rolls": 1,
                "conditions": [{ "type": "survives_explosion" }],
                "entries": [{ "type": "item", "item": "demo:block" }],
            }],
        }))
    );
}

#[test]
fn nexo_recipe_fields_map_to_craft_engine_recipe_semantics() {
    let mut diagnostics = DiagnosticBag::new();
    let shaped = convert_recipe(
        NexoRecipeType::Shaped,
        "chair",
        &obj(json!({
            "result": { "nexo_item": "chair", "amount": 2 },
            "shape": ["XX", " X"],
            "ingredients": { "X": { "minecraft_type": "STICK" } },
        })),
        "demo",
        &mut diagnostics,
        "recipe.yml",
    )
    .expect("shaped recipe converts");
    assert_eq!(shaped.get("type"), Some(&json!("shaped")));
    assert_eq!(shaped.get("result"), Some(&json!({ "id": "demo:chair", "count": 2 })));
    assert_eq!(
        shaped.get("ingredients").and_then(|ingredients| ingredients.get("X")),
        Some(&json!("minecraft:stick"))
    );

    let cooking = convert_recipe(
        NexoRecipeType::Furnace,
        "glass",
        &obj(json!({
            "result": { "minecraft_type": "GLASS" },
            "input": { "minecraft_type": "SAND" },
            "cookingTime": 200,
            "experience": 0.1,
        })),
        "demo",
        &mut diagnostics,
        "recipe.yml",
    )
    .expect("furnace recipe converts");
    assert_eq!(cooking.get("type"), Some(&json!("smelting")));
    assert_eq!(cooking.get("time").and_then(Value::as_f64), Some(200.0));
    assert_eq!(cooking.get("experience").and_then(Value::as_f64), Some(0.1));

    let missing = convert_recipe(
        NexoRecipeType::Shaped,
        "broken",
        &obj(json!({
            "result": { "nexo_item": "chair" },
            "shape": ["XY"],
            "ingredients": { "X": { "minecraft_type": "STICK" } },
        })),
        "demo",
        &mut diagnostics,
        "recipe.yml",
    );
    assert!(missing.is_none());
    assert!(has_code(&diagnostics, "SHAPED_INGREDIENT_MISSING"));
}

#[test]
fn nexo_sound_entries_become_ce_sound_event_maps_without_ogg_suffix() {
    let mut diagnostics = DiagnosticBag::new();
    let sounds = convert_sounds(
        &obj(json!({ "sounds": [{ "id": "demo:music.test", "sound": "demo:music/test.ogg", "stream": true }] })),
        &mut diagnostics,
        "sounds.yml",
    );
    let event = sounds
        .get("demo:music.test")
        .expect("sound event present")
        .as_object()
        .expect("sound event object");
    let file = event
        .get("sounds")
        .and_then(Value::as_array)
        .expect("sounds list")[0]
        .as_object()
        .expect("sound file entry");
    assert_eq!(file.get("name"), Some(&json!("demo:music/test")));
    assert_eq!(file.get("stream"), Some(&json!(true)));
    assert_eq!(file.get("attenuation_distance").and_then(Value::as_f64), Some(16.0));
}

#[test]
fn author_namespaces_are_inferred_from_bundle_declarations_and_nexo_filenames() {
    let chinese = infer_author_namespace_from_bundle_paths(
        &[
            "Nexo/items/lanshan/lanshan_chinese_2.yml".to_string(),
            "ItemsAdder/contents/lanshan_chinese_2/configs/categories.yml".to_string(),
            "ItemsAdder/contents/lanshan_chinese_2/resourcepack/assets/lanshan_chinese_2/models/item/demo.json".to_string(),
        ],
        "Nexo",
    )
    .expect("chinese bundle namespace inferred");
    assert_eq!(chinese.namespace, "lanshan_chinese_2");

    let autumn = infer_author_namespace_from_bundle_paths(
        &[
            "wrapper/Nexo/items/lanshan/lanshan_autumn_field.yml".to_string(),
            "wrapper/ItemsAdder/contents/lanshan_autumn_field/configs/1.yml".to_string(),
        ],
        "wrapper/Nexo",
    )
    .expect("autumn bundle namespace inferred");
    assert_eq!(autumn.namespace, "lanshan_autumn_field");

    let balloon = infer_author_namespace_from_bundle_paths(
        &[
            "Nexo/item/lanshan/lanshan_happy_ghast_hot_air_balloon_sprite.yml".to_string(),
            "Nexo/item/lanshan/lanshan_hot_air_balloon.yml".to_string(),
            "Nexo/item/lanshan/lanshan_hot_air_balloon_sprite.yml".to_string(),
            "ItemsAdder/contents/lanshan_hot_air_balloon/configs/1.yml".to_string(),
            "MythicMobs/packs/lanshan_hot_air_balloon/packinfo.yml".to_string(),
        ],
        "Nexo",
    )
    .expect("balloon bundle namespace inferred");
    assert_eq!(balloon.namespace, "lanshan_hot_air_balloon");

    let nexo_only = infer_author_namespace_from_bundle_paths(
        &[
            "Nexo/item/lanshan/lanshan_happy_ghast_hot_air_balloon_sprite.yml".to_string(),
            "Nexo/item/lanshan/lanshan_hot_air_balloon.yml".to_string(),
            "Nexo/item/lanshan/lanshan_hot_air_balloon_sprite.yml".to_string(),
        ],
        "Nexo",
    )
    .expect("nexo-only bundle namespace inferred");
    assert_eq!(nexo_only.namespace, "lanshan_hot_air_balloon");
}

#[test]
fn missing_static_model_typo_redirects_to_existing_near_match_without_creating_assets() {
    let temp = TempDir::new("model-alias");
    let input = temp.path.join("Nexo");
    let output = temp.path.join("CraftEnginePack");
    write(
        &input,
        "items/demo.yml",
        &lines(&[
            "red_balloon:",
            "  material: PAPER",
            "  Pack:",
            "    generate_model: false",
            "    model: demo/red_balloon_sprite",
            "    custom_model_data: 1234",
            "",
        ]),
    );
    write(
        &input,
        "pack/assets/minecraft/models/demo/red_balloon_spirit.json",
        &serde_json::to_string(&json!({
            "parent": "minecraft:item/generated",
            "textures": { "layer0": "demo/red_balloon" },
        }))
        .unwrap(),
    );
    write(&input, "pack/assets/minecraft/textures/demo/red_balloon.png", "fixture");

    let result = convert(&ConvertOptions {
        input: input.display().to_string(),
        output: output.display().to_string(),
        namespace: None,
        source_namespace: None,
        client_mode: ClientMode::Hybrid,
        cmd_policy: CmdPolicy::Preserve,
        strict: true,
        force: false,
        audit: true,
    })
    .expect("conversion succeeds");

    assert!(result.success, "{}", result.diagnostics.format_lines().join("\n"));
    assert_eq!(result.namespace, "demo");
    assert_eq!(result.namespace_mode, "author");
    assert_eq!(result.audit.as_ref().expect("audit summary").missing_models, 0);
    assert!(result
        .diagnostics
        .items
        .iter()
        .any(|entry| entry.code == "MODEL_REFERENCE_TYPO_RECOVERED" && !entry.lossy));

    let yaml = read_output_yaml(&output.join("configuration/items.yml"));
    assert_eq!(
        yaml["items"]["demo:red_balloon"]["model"]["path"],
        json!("minecraft:demo/red_balloon_spirit")
    );
    assert!(std::fs::read_to_string(
        output.join("resourcepack/assets/minecraft/models/demo/red_balloon_spirit.json")
    )
    .is_ok());
    assert!(std::fs::read_to_string(
        output.join("resourcepack/assets/minecraft/models/demo/red_balloon_sprite.json")
    )
    .is_err());
}

#[test]
fn ambiguous_near_match_models_are_never_guessed() {
    let temp = TempDir::new("model-ambiguous");
    let root = temp.path.join("pack");
    write(&root, "assets/minecraft/models/demo/red_balloon_spritz.json", "{}");
    write(&root, "assets/minecraft/models/demo/red_balloon_sprita.json", "{}");
    let mut diagnostics = DiagnosticBag::new();
    let aliases = discover_model_aliases(
        Some(&root),
        &[ResolvedItem {
            id: "red_balloon".to_string(),
            source: "items.yml".to_string(),
            template: false,
            template_ids: Vec::new(),
            config: obj(json!({ "Pack": { "model": "demo/red_balloon_sprite" } })),
        }],
        &mut diagnostics,
    );
    assert!(aliases.is_empty());
    assert!(diagnostics.items.is_empty());
}

#[test]
fn end_to_end_globals_drive_rotation_with_concise_furniture_variants() {
    let temp = TempDir::new("globals");
    let input = temp.path.join("Nexo");
    let output = temp.path.join("CraftEnginePack");
    // TS fixture creates an empty pack/assets directory (converter.test.ts 1088).
    std::fs::create_dir_all(input.join("pack/assets")).unwrap();
    write(&input, "mechanics.yml", "furniture:\n  default_rotatable_on_sneak: true\n");
    write(
        &input,
        "settings.yml",
        "Furniture:\n  allowed_gamemodes_for_rotation:\n    - ADVENTURE\n",
    );
    write(
        &input,
        "inventory.yml",
        &lines(&[
            "NexoInventory:",
            "  type: FILE",
            "  menu_title: '<glyph:not_used_by_categories>'",
            "  layout:",
            "    demo:",
            "      itemname: '<aqua>Demo Items</aqua>'",
            "      icon: turning",
            "      slot: 4",
            "",
        ]),
    );
    write(
        &input,
        "items/demo.yml",
        &lines(&[
            "support:",
            "  material: PAPER",
            "  Pack:",
            "    model: demo:block/support",
            "  Mechanics:",
            "    noteblock: {}",
            "turning:",
            "  material: PAPER",
            "  Pack:",
            "    model: demo:item/turning",
            "  Mechanics:",
            "    furniture:",
            "      rotatable: true",
            "      limited_placing:",
            "        floor: false",
            "        roof: false",
            "        wall: true",
            "      properties:",
            "        display_transform: FIXED",
            "        offset_against_blocks: true",
            "      hitbox:",
            "        interactions:",
            "          - '0,0,0 1,1'",
            "      lights:",
            "        lights:",
            "          - '0,1,0 14'",
            "",
        ]),
    );

    let result = convert(&ConvertOptions {
        input: input.display().to_string(),
        output: output.display().to_string(),
        namespace: Some("demo".to_string()),
        source_namespace: None,
        client_mode: ClientMode::Modern,
        cmd_policy: CmdPolicy::Preserve,
        strict: true,
        force: false,
        audit: false,
    })
    .expect("conversion succeeds");

    assert!(result.success, "{}", result.diagnostics.format_lines().join("\n"));
    assert_eq!(result.category_count, 1);

    let category_yaml = read_output_yaml(&output.join("configuration/categories.yml"));
    let category = &category_yaml["categories"]["demo:demo"];
    let category_keys: std::collections::BTreeSet<&str> = category
        .as_object()
        .expect("category object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        category_keys,
        ["icon", "list", "name", "priority"].into_iter().collect::<std::collections::BTreeSet<_>>()
    );
    assert_eq!(category["name"], json!("<!i><green><aqua>Demo Items</aqua></green>"));
    assert_eq!(category["icon"], json!("demo:turning"));
    assert_eq!(category["priority"].as_f64(), Some(3.0));
    assert_eq!(category["list"], json!(["demo:support", "demo:turning"]));

    let furniture_text = std::fs::read_to_string(output.join("configuration/furniture.yml"))
        .expect("furniture.yml written");
    let yaml = read_output_yaml(&output.join("configuration/furniture.yml"));
    assert!(std::fs::read_to_string(output.join("configuration/furniture-templates.yml")).is_err());
    assert!(!furniture_text.contains("_nexo2ce/furniture/variant-shift"));
    assert!(!furniture_text.contains("__nexo2ce_"));
    assert!(!furniture_text.contains("${"));

    let furniture = &yaml["furniture"]["demo:turning"];
    assert!(furniture.get("template").is_none());
    assert_eq!(furniture["settings"]["item"], json!("demo:turning"));
    assert!(furniture.get("behavior").is_none());
    let behavior = &furniture["behaviors"];
    assert_eq!(behavior["type"], json!("glowing_furniture"));
    let lights = behavior["lights"].as_array().expect("lights array");
    assert_eq!(lights.len(), 1);
    assert_eq!(lights[0]["position"], json!("0,1,0.01"));
    assert_eq!(lights[0]["level"].as_f64(), Some(14.0));
    assert!(behavior.get("variants").is_none());
    let variants = furniture["variants"].as_object().expect("variants object");
    assert_eq!(variants.keys().map(String::as_str).collect::<Vec<_>>(), vec!["wall"]);
    assert!(!furniture_text.contains("_nexo_ground_barrier_grid"));
    assert!(!furniture_text.contains("_nexo_ceiling_barrier_grid"));
    assert!(!furniture_text.contains("_nexo_wall_supported"));
    assert!(!furniture_text.contains("<arg:position.y>"));

    let events = furniture["events"].as_array().expect("events array");
    assert!(!events.iter().any(|entry| entry["on"] == json!("place")));
    let click = events
        .iter()
        .find(|entry| entry["on"] == json!("right_click"))
        .expect("right_click event");
    let functions = click["functions"].as_array().expect("functions array");
    let rotate = functions
        .iter()
        .find(|entry| entry["type"] == json!("rotate_furniture"))
        .expect("rotate_furniture function");
    let conditions = rotate["conditions"].as_array().expect("conditions array");
    assert_eq!(conditions[0]["type"], json!("expression"));
    let terms = conditions[1]["terms"].as_array().expect("terms array");
    assert_eq!(
        terms.iter().map(|term| term["value2"].clone()).collect::<Vec<Value>>(),
        vec![json!("ADVENTURE")]
    );
}

#[test]
fn end_to_end_conversion_copies_resources_and_passes_model_texture_graph_audit() {
    let temp = TempDir::new("e2e");
    let input = temp.path.join("Nexo");
    let output = temp.path.join("CraftEnginePack");
    write(
        &input,
        "items/demo.yml",
        &lines(&[
            "demo:",
            "  itemname: Demo",
            "  material: PAPER",
            "  Pack:",
            "    model: custom/demo",
            "    custom_model_data: 1234",
            "  Mechanics:",
            "    furniture:",
            "      limited_placing:",
            "        floor: false",
            "        roof: false",
            "        wall: false",
            "      properties:",
            "        offset_against_blocks: false",
            "      hitbox:",
            "        interactions:",
            "          - '0,0,0 1,1'",
            "",
        ]),
    );
    write(
        &input,
        "pack/assets/minecraft/models/custom/demo.json",
        &serde_json::to_string(&json!({
            "parent": "minecraft:item/generated",
            "textures": { "layer0": "custom/demo" },
        }))
        .unwrap(),
    );
    write(&input, "pack/assets/minecraft/textures/custom/demo.png", "fixture");

    let result = convert(&ConvertOptions {
        input: input.display().to_string(),
        output: output.display().to_string(),
        namespace: Some("demo".to_string()),
        source_namespace: None,
        client_mode: ClientMode::Hybrid,
        cmd_policy: CmdPolicy::Preserve,
        strict: true,
        force: false,
        audit: true,
    })
    .expect("conversion succeeds");

    assert!(result.success, "{}", result.diagnostics.format_lines().join("\n"));
    assert_eq!(result.item_count, 1);
    assert_eq!(result.furniture_count, 1);
    let audit = result.audit.as_ref().expect("audit summary");
    assert_eq!(audit.missing_models, 0);
    assert_eq!(audit.missing_textures, 0);

    let yaml = read_output_yaml(&output.join("configuration/items.yml"));
    let item = &yaml["items"]["demo:demo"];
    assert_eq!(item["item_model"], json!("demo:demo"));
    assert_eq!(item["custom_model_data"].as_f64(), Some(1234.0));
    assert!(std::fs::read(output.join("resourcepack/assets/minecraft/models/custom/demo.json")).is_ok());
    for absent in ["blocks.yml", "recipes.yml", "sounds.yml", "images.yml"] {
        assert!(
            std::fs::read_to_string(output.join("configuration").join(absent)).is_err(),
            "{absent} should not exist"
        );
    }
}

#[test]
fn bbmodel_assets_relocate_to_ce_blueprint_paths_and_pass_graph_audit() {
    let temp = TempDir::new("bbmodel");
    let input = temp.path.join("Nexo");
    let output = temp.path.join("CraftEnginePack");
    write(
        &input,
        "items/demo.yml",
        &lines(&["chair:", "  material: PAPER", "  Pack:", "    bbmodel: demo:item/chair", ""]),
    );
    write(
        &input,
        "pack/assets/demo/models/item/chair.bbmodel",
        &serde_json::to_string(&json!({
            "meta": { "format_version": "4.10", "model_format": "free" },
            "name": "chair",
            "resolution": { "width": 16, "height": 16 },
            "elements": [],
            "outliner": [],
            "textures": [],
        }))
        .unwrap(),
    );

    let result = convert(&ConvertOptions {
        input: input.display().to_string(),
        output: output.display().to_string(),
        namespace: Some("demo".to_string()),
        source_namespace: None,
        client_mode: ClientMode::Hybrid,
        cmd_policy: CmdPolicy::Preserve,
        strict: false,
        force: false,
        audit: true,
    })
    .expect("conversion succeeds");

    assert!(result.success, "{}", result.diagnostics.format_lines().join("\n"));
    let audit = result.audit.as_ref().expect("audit summary");
    assert_eq!(audit.referenced_blueprints, 1);
    assert_eq!(audit.missing_blueprints, 0);
    assert!(has_code(&result.diagnostics, "BBMODEL_CONVERTER_REVIEW"));
    assert!(std::fs::read_to_string(output.join("blueprint/demo/item/chair.bbmodel")).is_ok());
    assert!(std::fs::read_to_string(
        output.join("resourcepack/assets/demo/models/item/chair.bbmodel")
    )
    .is_err());
    assert!(std::fs::read_to_string(output.join("configuration/furniture.yml")).is_err());
    assert!(std::fs::read_to_string(output.join("configuration/furniture-templates.yml")).is_err());

    let yaml = read_output_yaml(&output.join("configuration/items.yml"));
    let item = &yaml["items"]["demo:chair"];
    let model_text = serde_json::to_string(&item["model"]).expect("model serializes");
    assert!(model_text.contains("\"path\":\"demo:item/chair\""), "{model_text}");
    assert!(model_text.contains("\"blueprint\":\"demo/item/chair\""), "{model_text}");
}

#[test]
fn modern_model_tints_and_player_head_special_rendering_match_nexo_and_minecraft() {
    let mut diagnostics = DiagnosticBag::new();
    let pack = obj(json!({ "model": "demo:item/horse" }));
    let mut context = ModelContext {
        source: "fixture.yml".to_string(),
        item: "demo".to_string(),
        diagnostics: &mut diagnostics,
        model_aliases: None,
    };
    let horse = read_pack_model(Some(&pack), "horse", &mut context);

    let inherited = convert_models(&horse, None, "leather_horse_armor", None, ClientMode::Modern, &mut context)
        .model
        .expect("model emitted");
    assert_eq!(inherited["tints"], json!([{ "type": "dye", "default": -6265536 }]));

    let colored = convert_models(
        &horse,
        None,
        "leather_horse_armor",
        Some(&json!("255,0,0")),
        ClientMode::Modern,
        &mut context,
    )
    .model
    .expect("model emitted");
    assert_eq!(colored["tints"], json!([{ "type": "dye", "default": 16711680 }]));

    let head = convert_models(&horse, None, "player_head", None, ClientMode::Modern, &mut context)
        .model
        .expect("model emitted");
    assert_eq!(
        head,
        json!({ "type": "special", "base": "demo:item/horse", "model": { "type": "player_head" } })
    );
}

#[test]
fn nexo_attribute_and_pdc_schemas_become_loadable_craft_engine_processors() {
    let mut diagnostics = DiagnosticBag::new();
    let source = ResolvedItem {
        id: "blade".to_string(),
        source: "items.yml".to_string(),
        template: false,
        template_ids: Vec::new(),
        config: obj(json!({
            "material": "IRON_SWORD",
            "AttributeModifiers": [{
                "attribute": "GENERIC_ATTACK_DAMAGE",
                "amount": 3,
                "operation": "ADD_SCALAR",
                "slot": "MAINHAND",
            }],
            "PersistentData": [{ "key": "demo:value", "type": "INTEGER", "value": 7 }],
        })),
    };
    let options = ItemOptions {
        namespace: "demo".to_string(),
        client_mode: ClientMode::Modern,
        model_aliases: None,
    };
    let result = convert_item(&source, &options, None, &mut diagnostics).expect("item converts");
    let data = &result.config["data"];
    // JS deepEqual compares numbers by value (3 == 3.0), so check amount numerically.
    let modifiers = data["attribute_modifiers"].as_array().expect("attribute_modifiers array");
    assert_eq!(modifiers.len(), 1);
    let modifier = &modifiers[0];
    assert_eq!(modifier["type"], json!("minecraft:attack_damage"));
    assert_eq!(modifier["slot"], json!("mainhand"));
    assert_eq!(modifier["id"], json!("nexo:blade_attack_damage"));
    assert_eq!(modifier["amount"].as_f64(), Some(3.0));
    assert_eq!(modifier["operation"], json!("add_multiplied_base"));
    let pdc = data["pdc"].as_object().expect("pdc object");
    assert_eq!(pdc.len(), 1);
    assert_eq!(pdc.get("demo:value").and_then(Value::as_f64), Some(7.0));
}

#[test]
fn glyph_grids_count_supplementary_code_points_and_allocate_per_font() {
    let temp = TempDir::new("glyph-unicode");
    write(
        &temp.path,
        "glyphs/unicode.yml",
        &lines(&[
            "astral:",
            "  texture: demo:font/astral",
            "  char: \"😀😁\"",
            "  font: demo:astral",
            "astral_second:",
            "  reference: astral",
            "  index: 2",
            "fixed_a:",
            "  texture: demo:font/a",
            "  char: \"\\uA410\"",
            "  font: demo:a",
            "fixed_b:",
            "  texture: demo:font/b",
            "  char: \"\\uA410\"",
            "  font: demo:b",
            "auto_a:",
            "  texture: demo:font/auto_a",
            "  font: demo:a",
            "auto_c:",
            "  texture: demo:font/auto_c",
            "  font: demo:c",
            "",
        ]),
    );
    let mut diagnostics = DiagnosticBag::new();
    let glyphs = convert_glyphs(&temp.path, "demo", &mut diagnostics, Some("nexo:default"), Some(""))
        .expect("glyphs convert");
    assert_eq!(glyphs.images["demo:astral"]["chars"], json!(["😀😁"]));
    assert_eq!(glyphs.entries["astral"].columns, 2);
    assert_eq!(
        rewrite_glyph_tags(&json!("<glyph:astral_second>"), &glyphs.entries, &mut diagnostics, "item.yml", "demo"),
        json!("<white><image:demo:astral:0:1></white>")
    );
    assert_eq!(glyphs.entries["auto_a"].chars[0], "\u{A411}");
    assert_eq!(glyphs.entries["auto_c"].chars[0], "\u{A410}");
    assert!(!has_code(&diagnostics, "GLYPH_CHAR_CONFLICT"));
    assert!(!has_code(&diagnostics, "GLYPH_SUPPLEMENTARY_CHAR_REVIEW"));
}

#[test]
fn glyph_allocator_preserves_explicit_codepoints_and_rewrites_nexo_tags_to_ce_images() {
    let temp = TempDir::new("glyph");
    write(
        &temp.path,
        "glyphs/a.yml",
        "reserved:\n  texture: demo:font/reserved\n  char: \"\\uA410\"\nslice:\n  reference: auto\n  index: 1..2\n",
    );
    write(&temp.path, "glyphs/b.yml", "auto:\n  texture: demo:font/auto\n  rows: 1\n  columns: 2\n");
    let mut diagnostics = DiagnosticBag::new();
    let glyphs = convert_glyphs(&temp.path, "demo", &mut diagnostics, Some("nexo:default"), Some(""))
        .expect("glyphs convert");
    let auto = &glyphs.entries["auto"];
    assert_eq!(auto.chars[0], "\u{A411}\u{A412}");
    assert_eq!(glyphs.images["demo:auto"]["font"], json!("nexo:default"));
    assert_eq!(
        rewrite_glyph_tags(&json!("x<glyph:auto>y"), &glyphs.entries, &mut diagnostics, "item.yml", "demo"),
        json!("x<white><image:demo:auto:0:0></white><shift:-1><white><image:demo:auto:0:1></white>y")
    );
    assert_eq!(
        rewrite_glyph_tags(&json!("<g:auto:2:colorable>"), &glyphs.entries, &mut diagnostics, "item.yml", "demo"),
        json!("<image:demo:auto:0:1>")
    );
    assert_eq!(
        rewrite_glyph_tags(&json!("<glyph:slice>"), &glyphs.entries, &mut diagnostics, "item.yml", "demo"),
        json!("<white><image:demo:auto:0:0></white><shift:-1><white><image:demo:auto:0:1></white>")
    );
    assert_eq!(
        rewrite_glyph_tags(&json!("<glyph:auto:1..2>"), &glyphs.entries, &mut diagnostics, "item.yml", "demo"),
        json!("<white><image:demo:auto:0:0></white><shift:-1><white><image:demo:auto:0:1></white><shift:-1>")
    );
    assert_eq!(
        rewrite_glyph_tags(&json!("\\<glyph:auto>"), &glyphs.entries, &mut diagnostics, "item.yml", "demo"),
        json!("\\<glyph:auto>")
    );
}
