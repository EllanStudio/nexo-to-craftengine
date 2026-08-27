//! Port of legacy/test/converter.test.ts lines 26-489.
//!
//! Covers: resource locations, FILE/DIRECTORY category inventories, strict YAML
//! duplicate keys, output path overlap safety, dual items/item directory loading,
//! bow/crossbow/damaged model trees, template inheritance, modern compound CMD,
//! root coercions, vanilla-dyeable dye recipes, Components whitelist/clamps,
//! explicit item_model, ItemModel root metadata, PotionEffects, unset_components
//! and invalid PotionEffects diagnostics.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use nexo2ce::categories::{convert_categories, CategoryConversionOptions, CategoryItem};
use nexo2ce::converter::{convert, ConvertOptions};
use nexo2ce::diagnostics::{Details, DiagnosticBag, Severity};
use nexo2ce::io::load_yaml;
use nexo2ce::items::{
    convert_item, match_bukkit_material, resolve_item_templates, ItemOptions, ResolvedItem,
    SourceItem,
};
use nexo2ce::json::JsonObject;
use nexo2ce::models::{build_legacy_model, convert_models, read_pack_model, ModelContext};
use nexo2ce::resource_location::{normalize_model_location, normalize_texture_location};
use nexo2ce::{ClientMode, CmdPolicy};
use serde_json::{json, Value};

static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "nexo2ce-{}-{}-{}",
            name,
            std::process::id(),
            DIR_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Mirrors the node:fs symlink(target, link, "junction") calls of the TS test:
/// on Windows a junction is created (no elevation required), on Unix a plain
/// directory symlink.
fn symlink_dir(target: &Path, link: &Path) {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &link.to_string_lossy(),
                &target.to_string_lossy(),
            ])
            .output()
            .expect("run mklink /J");
        assert!(
            output.status.success() && link.is_dir(),
            "mklink /J failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).expect("create directory symlink");
    }
}

fn obj(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        other => panic!("expected JSON object, got {other}"),
    }
}

/// JS deepStrictEqual semantics: numbers compare by numeric value (3 == 3.0),
/// objects require identical key sets, arrays compare order-sensitive.
fn js_deep_equal(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        },
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| js_deep_equal(value, other)))
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|(left, right)| js_deep_equal(left, right))
        }
        (a, b) => a == b,
    }
}

fn has_code(diagnostics: &DiagnosticBag, code: &str) -> bool {
    diagnostics.items.iter().any(|entry| entry.code == code)
}

/// TS test helper: context(diagnostics, item = "demo").
fn model_context<'a>(diagnostics: &'a mut DiagnosticBag, item: &str) -> ModelContext<'a> {
    ModelContext {
        source: "fixture.yml".to_string(),
        item: item.to_string(),
        diagnostics,
        model_aliases: None,
    }
}

fn item_options(client_mode: ClientMode) -> ItemOptions<'static> {
    ItemOptions {
        namespace: "demo".to_string(),
        client_mode,
        model_aliases: None,
    }
}

fn resolved_item(id: &str, config: JsonObject) -> ResolvedItem {
    ResolvedItem {
        id: id.to_string(),
        source: "fixture.yml".to_string(),
        config,
        template: false,
        template_ids: Vec::new(),
    }
}

fn convert_options(input: &Path, output: &Path, force: bool) -> ConvertOptions {
    ConvertOptions {
        input: input.display().to_string(),
        output: output.display().to_string(),
        namespace: Some("demo".to_string()),
        source_namespace: None,
        client_mode: ClientMode::Modern,
        cmd_policy: CmdPolicy::Preserve,
        strict: false,
        force,
        audit: false,
    }
}

#[test]
fn resource_locations_use_minecrafts_real_default_namespace() {
    let mut diagnostics = DiagnosticBag::new();
    assert_eq!(
        normalize_model_location("custom/chair.json", &mut diagnostics, &Details::new()).as_deref(),
        Some("minecraft:custom/chair")
    );
    assert_eq!(
        normalize_texture_location("demo:block/chair.png", &mut diagnostics, &Details::new())
            .as_deref(),
        Some("demo:block/chair")
    );
    assert_eq!(diagnostics.items.len(), 0);
}

#[test]
fn nexo_file_inventory_becomes_ordered_craftengine_categories() {
    // The TS test joins process.cwd()/fixtures/Nexo; the paths never need to exist.
    let mut diagnostics = DiagnosticBag::new();
    let categories = convert_categories(CategoryConversionOptions {
        root: "fixtures/Nexo".to_string(),
        namespace: "demo".to_string(),
        items: vec![
            CategoryItem {
                source: "fixtures/Nexo/items/tools.yml".to_string(),
                source_id: "hammer".to_string(),
                target_id: "demo:hammer".to_string(),
                config: JsonObject::new(),
            },
            CategoryItem {
                source: "fixtures/Nexo/items/tools.yml".to_string(),
                source_id: "chisel".to_string(),
                target_id: "demo:chisel".to_string(),
                config: obj(json!({ "excludeFromInventory": true })),
            },
            CategoryItem {
                source: "fixtures/Nexo/items/seasonal/decor.yml".to_string(),
                source_id: "lantern".to_string(),
                target_id: "demo:lantern".to_string(),
                config: JsonObject::new(),
            },
        ],
        inventory: Some(obj(json!({ "NexoInventory": {
            "type": "FILE",
            "layout": {
                "tools": { "itemname": "<gold>Tools</gold>", "icon": "nexo:hammer", "slot": 3 },
                "decor": { "title": "Seasonal Decor", "slot": 1 },
            },
        } }))),
        inventory_source: Some("fixtures/Nexo/inventory.yml".to_string()),
        rewrite_text: None,
        diagnostics: &mut diagnostics,
    });

    let keys: Vec<&str> = categories.keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["demo:seasonal/decor", "demo:tools"]);
    assert!(js_deep_equal(
        categories.get("demo:seasonal/decor").expect("seasonal/decor category"),
        &json!({
            "name": "<!i><green>Seasonal Decor</green>",
            "icon": "demo:lantern",
            "priority": 0,
            "list": ["demo:lantern"],
        })
    ));
    assert!(js_deep_equal(
        categories.get("demo:tools").expect("tools category"),
        &json!({
            "name": "<!i><green><gold>Tools</gold></green>",
            "icon": "demo:hammer",
            "priority": 2,
            "list": ["demo:hammer"],
        })
    ));
    assert_eq!(diagnostics.items.len(), 0);
}

#[test]
fn invalid_local_category_icons_fall_back_to_a_converted_member() {
    let mut diagnostics = DiagnosticBag::new();
    let categories = convert_categories(CategoryConversionOptions {
        root: "fixtures/Nexo".to_string(),
        namespace: "demo".to_string(),
        items: vec![CategoryItem {
            source: "fixtures/Nexo/items/demo.yml".to_string(),
            source_id: "lamp".to_string(),
            target_id: "demo:lamp".to_string(),
            config: JsonObject::new(),
        }],
        inventory: Some(obj(json!({ "NexoInventory": { "layout": { "demo": { "icon": "nexo:missing" } } } }))),
        inventory_source: Some("fixtures/Nexo/inventory.yml".to_string()),
        rewrite_text: None,
        diagnostics: &mut diagnostics,
    });

    assert_eq!(
        categories
            .get("demo:demo")
            .expect("demo category")
            .get("icon")
            .and_then(Value::as_str),
        Some("demo:lamp")
    );
    assert_eq!(
        diagnostics.items.first().map(|entry| entry.code.as_str()),
        Some("CATEGORY_ICON_FALLBACK")
    );
    assert_eq!(diagnostics.items.first().map(|entry| entry.lossy), Some(true));
}

#[test]
fn nexo_directory_inventory_becomes_visible_parents_and_hidden_ce_subcategories() {
    let mut diagnostics = DiagnosticBag::new();
    let categories = convert_categories(CategoryConversionOptions {
        root: "fixtures/Nexo".to_string(),
        namespace: "demo".to_string(),
        items: vec![
            CategoryItem {
                source: "fixtures/Nexo/items/root.yml".to_string(),
                source_id: "token".to_string(),
                target_id: "demo:token".to_string(),
                config: JsonObject::new(),
            },
            CategoryItem {
                source: "fixtures/Nexo/items/furniture/chairs.yml".to_string(),
                source_id: "chair".to_string(),
                target_id: "demo:chair".to_string(),
                config: JsonObject::new(),
            },
            CategoryItem {
                source: "fixtures/Nexo/items/furniture/tables.yml".to_string(),
                source_id: "table".to_string(),
                target_id: "demo:table".to_string(),
                config: JsonObject::new(),
            },
        ],
        inventory: Some(obj(json!({ "NexoInventory": {
            "type": "DIRECTORY",
            "directory_icon": "nexo:chair",
            "layout": {
                "furniture": {
                    "itemname": "Furniture",
                    "slot": 1,
                    "chairs": { "itemname": "Chairs", "slot": 2 },
                    "tables": { "itemname": "Tables", "slot": 1 },
                },
                "root": { "itemname": "Root Items", "slot": 2 },
            },
        } }))),
        inventory_source: None,
        rewrite_text: None,
        diagnostics: &mut diagnostics,
    });

    let furniture = categories.get("demo:furniture").expect("furniture category");
    assert!(js_deep_equal(
        furniture.get("list").expect("furniture list"),
        &json!(["#demo:furniture/tables", "#demo:furniture/chairs"])
    ));
    assert_eq!(furniture.get("priority").and_then(Value::as_f64), Some(0.0));
    assert_eq!(furniture.get("icon").and_then(Value::as_str), Some("demo:chair"));
    assert_eq!(
        categories
            .get("demo:root")
            .expect("root category")
            .get("priority")
            .and_then(Value::as_f64),
        Some(1.0)
    );
    assert!(js_deep_equal(
        categories.get("demo:furniture/tables").expect("tables category"),
        &json!({
            "name": "<!i><green>Tables</green>",
            "icon": "demo:table",
            "list": ["demo:table"],
            "hidden": true,
        })
    ));
    assert_eq!(
        categories
            .get("demo:furniture/chairs")
            .expect("chairs category")
            .get("hidden"),
        Some(&json!(true))
    );
    assert_eq!(diagnostics.items.len(), 0);
}

#[test]
fn strict_yaml_loader_rejects_duplicate_mapping_keys() {
    let temp = TempDir::new("yaml");
    let file = temp.path().join("duplicate.yml");
    std::fs::write(&file, "demo:\n  material: PAPER\n  material: STONE\n").unwrap();
    let mut diagnostics = DiagnosticBag::new();
    assert!(load_yaml(&file, &mut diagnostics).is_none());
    assert!(has_code(&diagnostics, "YAML_INVALID"));
}

#[test]
fn converter_rejects_destructive_or_recursive_output_path_overlap_before_force_deletion() {
    let temp = TempDir::new("path-safety");
    let bundle = temp.path().join("bundle");
    let input = bundle.join("Nexo");
    let item_file = input.join("items").join("demo.yml");
    std::fs::create_dir_all(input.join("items")).unwrap();
    std::fs::create_dir_all(input.join("pack").join("assets")).unwrap();
    std::fs::write(&item_file, "demo:\n  material: PAPER\n").unwrap();

    let error = convert(&convert_options(&input, &bundle, true))
        .expect_err("ancestor output must be rejected");
    assert!(error.to_string().contains("must not overlap"), "{error}");
    assert!(
        std::fs::read_to_string(&item_file).unwrap().contains("demo:"),
        "ancestor output must not delete the source bundle"
    );

    let error = convert(&convert_options(
        &input,
        &input.join("pack").join("assets").join("generated-output"),
        true,
    ))
    .expect_err("nested output must be rejected");
    assert!(error.to_string().contains("must not overlap"), "{error}");

    let linked_input = temp.path().join("linked").join("Nexo");
    let external_items = temp.path().join("external-items");
    std::fs::create_dir_all(&linked_input).unwrap();
    std::fs::create_dir_all(&external_items).unwrap();
    let external_item = external_items.join("linked.yml");
    std::fs::write(&external_item, "linked:\n  material: PAPER\n").unwrap();
    symlink_dir(&external_items, &linked_input.join("items"));

    let error = convert(&convert_options(&linked_input, &external_items, true))
        .expect_err("symlinked overlap must be rejected");
    assert!(error.to_string().contains("must not overlap"), "{error}");
    assert!(
        std::fs::read_to_string(&external_item).unwrap().contains("linked:"),
        "linked item sources must not be deleted"
    );
}

#[test]
fn converter_loads_valid_yaml_from_both_items_and_item_directories() {
    let temp = TempDir::new("dual-items");
    let input = temp.path().join("Nexo");
    let output = temp.path().join("output");
    std::fs::create_dir_all(input.join("items")).unwrap();
    std::fs::create_dir_all(input.join("item")).unwrap();
    std::fs::write(input.join("items").join("plural.yml"), "plural:\n  material: PAPER\n").unwrap();
    std::fs::write(input.join("item").join("singular.yml"), "singular:\n  material: STICK\n").unwrap();

    let result = convert(&convert_options(&input, &output, false)).expect("convert must not fail");
    assert!(result.success, "{}", result.diagnostics.format_lines().join("\n"));
    assert_eq!(result.item_count, 2);
    assert_eq!(result.category_count, 2);
    let mut yaml_diagnostics = DiagnosticBag::new();
    let yaml = load_yaml(&output.join("configuration").join("items.yml"), &mut yaml_diagnostics)
        .expect("items.yml must be written");
    let mut item_keys: Vec<String> = yaml
        .get("items")
        .and_then(Value::as_object)
        .expect("items mapping")
        .keys()
        .cloned()
        .collect();
    item_keys.sort();
    assert_eq!(item_keys, vec!["demo:plural".to_string(), "demo:singular".to_string()]);

    // item/items junctions to one physical directory must load once.
    let alias_input = temp.path().join("AliasNexo");
    let alias_source = temp.path().join("alias-source");
    std::fs::create_dir_all(&alias_input).unwrap();
    std::fs::create_dir_all(&alias_source).unwrap();
    std::fs::write(alias_source.join("only.yml"), "only:\n  material: PAPER\n").unwrap();
    symlink_dir(&alias_source, &alias_input.join("items"));
    symlink_dir(&alias_source, &alias_input.join("item"));
    let alias_result = convert(&convert_options(&alias_input, &temp.path().join("alias-output"), false))
        .expect("alias convert must not fail");
    assert!(alias_result.success, "{}", alias_result.diagnostics.format_lines().join("\n"));
    assert_eq!(alias_result.item_count, 1, "item/items aliases to one physical directory must load once");
    assert_eq!(alias_result.category_count, 1);
    assert!(!has_code(&alias_result.diagnostics, "DUPLICATE_ITEM_ID"));

    // Hard-linked YAML files must load once.
    let hardlink_input = temp.path().join("HardlinkNexo");
    std::fs::create_dir_all(hardlink_input.join("items")).unwrap();
    std::fs::create_dir_all(hardlink_input.join("item")).unwrap();
    let original = hardlink_input.join("items").join("original.yml");
    std::fs::write(&original, "hardlinked:\n  material: PAPER\n").unwrap();
    std::fs::hard_link(&original, hardlink_input.join("item").join("alias.yml")).unwrap();
    let hardlink_result =
        convert(&convert_options(&hardlink_input, &temp.path().join("hardlink-output"), false))
            .expect("hardlink convert must not fail");
    assert!(hardlink_result.success, "{}", hardlink_result.diagnostics.format_lines().join("\n"));
    assert_eq!(hardlink_result.item_count, 1, "hard-linked item YAML aliases must load once");
    assert_eq!(hardlink_result.category_count, 1);
    assert!(!has_code(&hardlink_result.diagnostics, "DUPLICATE_ITEM_ID"));
}

#[test]
fn nexo_bow_shortcut_thresholds_and_condition_tree_match_actual_generator() {
    let mut diagnostics = DiagnosticBag::new();
    let pack = obj(json!({
        "model": "demo:item/bow",
        "pulling_models": ["demo:item/bow_0", "demo:item/bow_1", "demo:item/bow_2"],
    }));
    let model = {
        let mut context = model_context(&mut diagnostics, "demo");
        let info = read_pack_model(Some(&pack), "bow", &mut context);
        convert_models(&info, None, "bow", None, ClientMode::Modern, &mut context)
            .model
            .expect("converted model")
    };
    assert_eq!(model.get("type").and_then(Value::as_str), Some("condition"));
    assert_eq!(model.get("property").and_then(Value::as_str), Some("using_item"));
    let dispatch = model.get("on_true").expect("on_true dispatch");
    assert_eq!(dispatch.get("property").and_then(Value::as_str), Some("use_duration"));
    assert_eq!(dispatch.get("scale").and_then(Value::as_f64), Some(0.05));
    let thresholds: Vec<f64> = dispatch
        .get("entries")
        .and_then(Value::as_array)
        .expect("entries")
        .iter()
        .map(|entry| entry.get("threshold").and_then(Value::as_f64).expect("threshold"))
        .collect();
    assert_eq!(thresholds, vec![0.0, 0.65, 0.9]);
}

#[test]
fn nexo_crossbow_modern_tree_wraps_pulling_in_charge_type_select() {
    let mut diagnostics = DiagnosticBag::new();
    let pack = obj(json!({
        "model": "demo:item/crossbow",
        "pulling_models": ["demo:item/pull_0", "demo:item/pull_1"],
        "charged_model": "demo:item/arrow",
        "firework_model": "demo:item/rocket",
    }));
    let model = {
        let mut context = model_context(&mut diagnostics, "demo");
        let info = read_pack_model(Some(&pack), "crossbow", &mut context);
        convert_models(&info, None, "crossbow", None, ClientMode::Modern, &mut context)
            .model
            .expect("converted model")
    };
    assert_eq!(model.get("type").and_then(Value::as_str), Some("select"));
    assert_eq!(model.get("property").and_then(Value::as_str), Some("charge_type"));
    let whens: Vec<Value> = model
        .get("cases")
        .and_then(Value::as_array)
        .expect("cases")
        .iter()
        .map(|entry| entry.get("when").cloned().unwrap_or(Value::Null))
        .collect();
    assert_eq!(whens, vec![json!("arrow"), json!("rocket")]);
    assert_eq!(
        model.get("fallback").expect("fallback").get("type").and_then(Value::as_str),
        Some("condition")
    );
}

#[test]
fn legacy_damaged_models_preserves_nexo_pulling_predicate_quirk() {
    let mut diagnostics = DiagnosticBag::new();
    let pack = obj(json!({
        "model": "demo:item/tool",
        "damaged_models": ["demo:item/d0", "demo:item/d1", "demo:item/d2"],
    }));
    let legacy = {
        let mut context = model_context(&mut diagnostics, "demo");
        let info = read_pack_model(Some(&pack), "tool", &mut context);
        build_legacy_model(&info)
    };
    let overrides = legacy.get("overrides").and_then(Value::as_array).expect("overrides");
    assert_eq!(overrides.len(), 2);
    let predicates: Vec<&Value> = overrides
        .iter()
        .map(|entry| entry.get("predicate").expect("predicate"))
        .collect();
    assert!(js_deep_equal(predicates[0], &json!({ "pulling": 1, "damage": 0.35 })));
    assert!(js_deep_equal(predicates[1], &json!({ "pulling": 1, "damage": 0.65 })));
}

#[test]
fn template_inheritance_is_recursive_and_item_values_override_templates() {
    let mut diagnostics = DiagnosticBag::new();
    let items = vec![
        SourceItem {
            id: "base".to_string(),
            source: "a.yml".to_string(),
            template: false,
            config: obj(json!({ "material": "PAPER", "itemname": "<item_id_capitalized>", "lore": ["base"] })),
        },
        SourceItem {
            id: "mid".to_string(),
            source: "a.yml".to_string(),
            template: false,
            config: obj(json!({ "template": "base", "Pack": { "model": "demo:<item_id>" } })),
        },
        SourceItem {
            id: "tea_set".to_string(),
            source: "b.yml".to_string(),
            template: false,
            config: obj(json!({ "template": "mid", "material": "STICK" })),
        },
        SourceItem {
            id: "invalid_child".to_string(),
            source: "b.yml".to_string(),
            template: false,
            config: obj(json!({ "template": "mid", "material": "not a material" })),
        },
    ];
    let resolved = resolve_item_templates(&items, &mut diagnostics);
    let find = |id: &str| {
        resolved
            .iter()
            .find(|entry| entry.id == id)
            .unwrap_or_else(|| panic!("missing resolved item {id}"))
    };
    let tea_set = find("tea_set");
    assert_eq!(tea_set.config.get("material").and_then(Value::as_str), Some("STICK"));
    assert_eq!(tea_set.config.get("itemname").and_then(Value::as_str), Some("Tea Set"));
    assert_eq!(
        tea_set
            .config
            .get("Pack")
            .and_then(Value::as_object)
            .and_then(|pack| pack.get("model"))
            .and_then(Value::as_str),
        Some("demo:tea_set")
    );
    assert_eq!(
        find("invalid_child").config.get("material").and_then(Value::as_str),
        Some("PAPER")
    );
    assert!(has_code(&diagnostics, "INVALID_MATERIAL_INHERITED"));
    assert!(find("base").template);
    assert!(find("mid").template);
}

#[test]
fn modern_compound_custom_model_data_remains_a_component_while_pack_cmd_is_root_metadata() {
    let mut diagnostics = DiagnosticBag::new();
    let item = resolved_item(
        "demo",
        obj(json!({
            "material": "PAPER",
            "Pack": { "model": "demo:item/demo", "custom_model_data": 1234 },
            "Components": {
                "custom_model_data": { "floats": [1.5], "flags": [true] },
                "item_model": "demo:special",
            },
        })),
    );
    let converted = convert_item(&item, &item_options(ClientMode::Hybrid), Some(1234), &mut diagnostics)
        .expect("converted item");
    assert_eq!(converted.config.get("custom_model_data").and_then(Value::as_f64), Some(1234.0));
    assert_eq!(converted.config.get("item_model").and_then(Value::as_str), Some("demo:special"));
    let data = converted.config.get("data").and_then(Value::as_object).expect("data");
    assert!(js_deep_equal(
        data.get("components")
            .and_then(Value::as_object)
            .and_then(|components| components.get("custom_model_data"))
            .expect("custom_model_data component"),
        &json!({ "floats": [1.5], "flags": [true] })
    ));
    assert!(data.get("custom_model_data").is_none());
}

#[test]
fn root_item_coercions_follow_nexo_and_invalid_materials_fall_back_to_paper() {
    let mut diagnostics = DiagnosticBag::new();
    assert_eq!(match_bukkit_material(Some(&json!("IRON SWORD"))).as_deref(), Some("iron_sword"));
    assert_eq!(match_bukkit_material(Some(&json!("minecraft:paper"))).as_deref(), Some("paper"));
    assert_eq!(match_bukkit_material(Some(&json!("LEGACY_STONE"))), None);
    let item = resolved_item(
        "root_fields",
        obj(json!({
            "material": "not a material",
            "itemname": 42,
            "customname": false,
            "lore": "scalar lore",
            "color": 16711680,
            "unbreakable": "true",
            "max_durability": 99,
            "trim_pattern": "sentry",
            "Enchantments": { "sharpness": 3 },
        })),
    );
    let converted = convert_item(&item, &item_options(ClientMode::Modern), None, &mut diagnostics)
        .expect("converted item");
    assert_eq!(converted.config.get("material").and_then(Value::as_str), Some("paper"));
    let data = converted.config.get("data").and_then(Value::as_object).expect("data");
    assert!(data.get("item_name").is_none());
    assert!(data.get("custom_name").is_none());
    assert!(data.get("lore").is_none());
    assert!(data.get("dyed_color").is_none());
    assert_eq!(data.get("unbreakable"), Some(&json!(false)));
    assert!(data.get("max_damage").is_none());
    assert!(js_deep_equal(
        data.get("trim").expect("trim"),
        &json!({ "pattern": "minecraft:sentry", "material": "minecraft:redstone" })
    ));
    assert!(js_deep_equal(
        data.get("enchantments").expect("enchantments"),
        &json!({ "minecraft:sharpness": 3 })
    ));
    assert!(has_code(&diagnostics, "INVALID_MATERIAL_DEFAULTED"));
    assert!(has_code(&diagnostics, "ROOT_MAX_DURABILITY_IGNORED"));
}

#[test]
fn vanilla_dyeable_nexo_items_opt_into_craftengines_custom_dye_recipe() {
    let mut diagnostics = DiagnosticBag::new();
    let leather = resolved_item(
        "dyeable_chair",
        obj(json!({
            "material": "LEATHER_HORSE_ARMOR",
            "color": "255, 255, 255",
            "Pack": { "model": "demo:item/dyeable_chair" },
        })),
    );
    let converted = convert_item(&leather, &item_options(ClientMode::Modern), None, &mut diagnostics)
        .expect("converted item");
    assert!(js_deep_equal(
        converted.config.get("settings").expect("settings"),
        &json!({ "dyeable": true })
    ));
    assert_eq!(
        converted
            .config
            .get("data")
            .and_then(Value::as_object)
            .and_then(|data| data.get("dyed_color"))
            .and_then(Value::as_f64),
        Some(0xffffff_u32 as f64)
    );
    assert_eq!(converted.semantics.get("dyeable"), Some(&json!(true)));

    let mut paper_config = leather.config.clone();
    paper_config.insert("material".to_string(), json!("PAPER"));
    let paper = resolved_item("colored_paper", paper_config);
    let non_dyeable = convert_item(&paper, &item_options(ClientMode::Modern), None, &mut diagnostics)
        .expect("converted item");
    assert!(
        non_dyeable.config.get("settings").is_none(),
        "a color component alone must not invent a dye recipe"
    );
    assert_eq!(non_dyeable.semantics.get("dyeable"), Some(&json!(false)));
}

#[test]
fn components_use_nexos_exact_whitelist_clamps_and_vanilla_codec_shapes() {
    let mut diagnostics = DiagnosticBag::new();
    let item = resolved_item(
        "components",
        obj(json!({
            "material": "PAPER",
            "Components": {
                "max_stack_size": 0,
                "max_damage": -4,
                "food": { "nutrition": 3.9, "saturation": 1.25, "can_always_eat": true },
                "painting_variant": "kebab",
                "use_cooldown": { "duration": "20t" },
                "enchantable": 0,
                "glider": true,
                "minimum_attack_charge": 2,
                "tooltip_display": ["minecraft:lore", "custom_model_data"],
                "tool": { "damage_per_block": 1 },
                "Potion_Contents": { "potion": "minecraft:water" },
            },
        })),
    );
    let converted = convert_item(&item, &item_options(ClientMode::Modern), None, &mut diagnostics)
        .expect("converted item");
    let components = converted
        .config
        .get("data")
        .and_then(Value::as_object)
        .and_then(|data| data.get("components"))
        .and_then(Value::as_object)
        .expect("components");
    assert_eq!(components.get("max_stack_size").and_then(Value::as_f64), Some(1.0));
    assert_eq!(components.get("max_damage").and_then(Value::as_f64), Some(1.0));
    assert!(js_deep_equal(
        components.get("food").expect("food"),
        &json!({ "nutrition": 3, "saturation": 1.25, "can_always_eat": true })
    ));
    assert_eq!(
        components.get("painting/variant").and_then(Value::as_str),
        Some("minecraft:kebab")
    );
    assert!(js_deep_equal(
        components.get("use_cooldown").expect("use_cooldown"),
        &json!({ "seconds": 1, "cooldown_group": "nexo:components" })
    ));
    assert_eq!(components.get("enchantable").and_then(Value::as_f64), Some(1.0));
    assert!(js_deep_equal(components.get("glider").expect("glider"), &json!({})));
    assert_eq!(components.get("minimum_attack_charge").and_then(Value::as_f64), Some(1.0));
    assert!(js_deep_equal(
        components.get("tooltip_display").expect("tooltip_display"),
        &json!({ "hide_tooltip": false, "hidden_components": ["minecraft:lore", "minecraft:custom_model_data"] })
    ));
    assert!(js_deep_equal(
        components.get("tool").expect("tool"),
        &json!({ "rules": [], "can_destroy_blocks_in_creative": false })
    ));
    assert!(components.get("Potion_Contents").is_none());
    assert!(!has_code(&diagnostics, "COMPONENT_CODEC_MANUAL"));
    assert!(has_code(&diagnostics, "NEXO_COMPONENT_UNKNOWN_IGNORED"));
}

#[test]
fn explicit_components_item_model_survives_without_a_generated_local_model() {
    let mut diagnostics = DiagnosticBag::new();
    let item = resolved_item(
        "pointer",
        obj(json!({ "material": "PAPER", "Components": { "item_model": "demo:external" } })),
    );
    let converted = convert_item(&item, &item_options(ClientMode::Modern), None, &mut diagnostics)
        .expect("converted item");
    assert_eq!(converted.config.get("item_model").and_then(Value::as_str), Some("demo:external"));
    assert!(converted.config.get("model").is_none());
}

#[test]
fn explicit_itemmodel_root_metadata_overrides_ces_incompatible_defaults() {
    let mut diagnostics = DiagnosticBag::new();
    let configured = resolved_item(
        "metadata",
        obj(json!({
            "material": "PAPER",
            "ItemModel": {
                "type": "minecraft:model",
                "model": "demo:item/metadata",
                "hand_animation_on_swap": false,
                "oversized_in_gui": true,
                "swap_animation_scale": 0.25,
            },
        })),
    );
    let converted = convert_item(&configured, &item_options(ClientMode::Modern), None, &mut diagnostics)
        .expect("converted item");
    assert_eq!(converted.config.get("hand_animation_on_swap"), Some(&json!(false)));
    assert_eq!(converted.config.get("oversized_in_gui"), Some(&json!(true)));
    assert_eq!(converted.config.get("swap_animation_scale").and_then(Value::as_f64), Some(0.25));
    assert!(js_deep_equal(
        converted.config.get("model").expect("model"),
        &json!({ "type": "model", "path": "demo:item/metadata" })
    ));

    let defaults = resolved_item(
        "defaults",
        obj(json!({ "material": "PAPER", "ItemModel": { "type": "model", "model": "demo:item/defaults" } })),
    );
    let defaulted = convert_item(&defaults, &item_options(ClientMode::Modern), None, &mut diagnostics)
        .expect("converted item");
    assert_eq!(defaulted.config.get("hand_animation_on_swap"), Some(&json!(true)));
    assert_eq!(defaulted.config.get("oversized_in_gui"), Some(&json!(false)));
    assert_eq!(defaulted.config.get("swap_animation_scale").and_then(Value::as_f64), Some(1.0));
}

#[test]
fn nexo_potion_effects_become_exact_1_21_11_potion_contents_entries() {
    let mut diagnostics = DiagnosticBag::new();
    let item = resolved_item(
        "tonic",
        obj(json!({
            "material": "POTION",
            "color": "255,0,16",
            "PotionEffects": [
                { "type": "SPEED", "duration": 100, "amplifier": 2 },
                { "type": "minecraft:slowness", "duration": 20, "amplifier": 0, "ambient": true, "has-particles": false },
                { "effect": "demo:stun", "duration": 40, "amplifier": 1, "has-icon": false },
            ],
            "Components": { "potion_contents": { "potion": "minecraft:strong_healing" } },
        })),
    );
    let converted = convert_item(&item, &item_options(ClientMode::Modern), None, &mut diagnostics)
        .expect("converted item");
    let potion = converted
        .config
        .get("data")
        .and_then(Value::as_object)
        .and_then(|data| data.get("components"))
        .and_then(Value::as_object)
        .and_then(|components| components.get("potion_contents"))
        .expect("potion_contents");
    assert_eq!(potion.get("custom_color").and_then(Value::as_f64), Some(0xff0010_u32 as f64));
    assert!(js_deep_equal(
        potion.get("custom_effects").expect("custom_effects"),
        &json!([
            { "id": "minecraft:speed", "duration": 100, "amplifier": 2, "ambient": false, "show_particles": true, "show_icon": true },
            { "id": "minecraft:slowness", "duration": 20, "amplifier": 0, "ambient": true, "show_particles": false, "show_icon": false },
            { "id": "demo:stun", "duration": 40, "amplifier": 1, "ambient": false, "show_particles": true, "show_icon": false },
        ])
    ));
    assert!(has_code(&diagnostics, "NEXO_COMPONENT_POTION_CONTENTS_IGNORED"));
    assert!(!has_code(&diagnostics, "POTION_EFFECTS_MANUAL"));
}

#[test]
fn components_unset_components_is_applied_after_generated_potion_effects() {
    let mut diagnostics = DiagnosticBag::new();
    let item = resolved_item(
        "cleared",
        obj(json!({
            "material": "PAPER",
            "PotionEffects": [{ "type": "speed", "duration": 20, "amplifier": 0 }],
            "Components": { "unset_components": ["minecraft:potion_contents"] },
        })),
    );
    let converted = convert_item(&item, &item_options(ClientMode::Modern), None, &mut diagnostics)
        .expect("converted item");
    let data = converted.config.get("data").and_then(Value::as_object);
    assert!(data
        .and_then(|data| data.get("components"))
        .and_then(Value::as_object)
        .and_then(|components| components.get("potion_contents"))
        .is_none());
    assert!(js_deep_equal(
        data.and_then(|data| data.get("remove_components")).expect("remove_components"),
        &json!(["potion_contents"])
    ));
}

#[test]
fn invalid_potion_effects_mirror_bukkit_construction_failure_diagnostics() {
    let mut diagnostics = DiagnosticBag::new();
    let item = resolved_item(
        "bad_tonic",
        obj(json!({ "material": "PAPER", "PotionEffects": [{ "type": "speed", "duration": 20.5, "amplifier": "1" }] })),
    );
    let converted = convert_item(&item, &item_options(ClientMode::Modern), None, &mut diagnostics)
        .expect("converted item");
    assert!(converted
        .config
        .get("data")
        .and_then(Value::as_object)
        .and_then(|data| data.get("components"))
        .is_none());
    assert!(diagnostics
        .items
        .iter()
        .any(|entry| entry.code == "POTION_EFFECT_INTEGER_REQUIRED" && entry.severity == Severity::Error));
}
