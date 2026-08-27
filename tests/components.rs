//! Port of legacy/test/components.test.ts: Nexo builder Components must
//! serialize to Minecraft 1.21.11 codec shapes through convert_item.

use serde_json::{json, Value};

use nexo2ce::diagnostics::DiagnosticBag;
use nexo2ce::items::{convert_item, ItemOptions, ResolvedItem};
use nexo2ce::json::JsonObject;
use nexo2ce::ClientMode;

fn converted_components(source: JsonObject, material: &str, id: &str) -> (Option<JsonObject>, DiagnosticBag) {
    let mut diagnostics = DiagnosticBag::new();
    let mut config = JsonObject::new();
    config.insert("material".to_string(), json!(material));
    config.insert("Components".to_string(), Value::Object(source));
    let item = ResolvedItem {
        id: id.to_string(),
        source: "components.yml".to_string(),
        template: false,
        template_ids: Vec::new(),
        config,
    };
    let options = ItemOptions {
        namespace: "demo".to_string(),
        client_mode: ClientMode::Modern,
        model_aliases: None,
    };
    let converted = convert_item(&item, &options, None, &mut diagnostics).expect("item converts");
    let components = converted
        .config
        .get("data")
        .and_then(|data| data.get("components"))
        .and_then(|components| components.as_object())
        .cloned();
    (components, diagnostics)
}

fn components_of(value: &Option<JsonObject>) -> Value {
    Value::Object(value.clone().expect("components present"))
}

#[test]
fn all_16_nexo_builder_components_serialize_to_codec_shapes() {
    let source = json!({
        "can_place_on": [{ "block": ["STONE", "minecraft:planks"] }],
        "can_break": { "block": "OAK_LOG", "state": { "axis": "y", "unknown": "ignored" } },
        "tool": {
            "default_mining_speed": 2.5,
            "damage_per_block": 2,
            "rules": [{ "materials": ["STONE", "OAK_LOG"], "tag": "mineable/pickaxe", "speed": 4, "correct_for_drops": true }]
        },
        "jukebox_playable": "minecraft:precipice",
        "use_remainder": { "minecraft_type": "BUCKET", "amount": 3 },
        "death_protection": { "death_effects": {
            "APPLY_EFFECTS": { "speed": { "amplifier": 1, "duration": "2s", "ambient": false, "show_particles": false, "show_icon": true, "probability": 0.5 } },
            "REMOVE_EFFECTS": ["poison"],
            "CLEAR_ALL_EFFECTS": true,
            "TELEPORT_RANDOMLY": { "diameter": 8 },
            "PLAY_SOUND": { "sound": "entity.player.levelup", "range": 2 }
        } },
        "consumable": {
            "consume_duration": "10t",
            "animation": "drink",
            "consume_particles": false,
            "sound": "entity.generic.drink",
            "effects": { "CLEAR_ALL_EFFECTS": true }
        },
        "glider": false,
        "equippable": {
            "slot": "HAND",
            "allowed_entity_types": ["PIG"],
            "asset_id": "demo:bronze",
            "camera_overlay": "demo:misc/bronze",
            "equip_sound": "item.armor.equip_chain",
            "dispensable": false,
            "swappable": false,
            "damage_on_hurt": false,
            "equip_on_interact": true,
            "can_be_sheared": true,
            "shear_sound": "entity.sheep.shear"
        },
        "repairable": ["IRON_INGOT", "DIAMOND"],
        "weapon": { "damage_per_attack": 3, "disable_blocking": "10t" },
        "blocks_attacks": {
            "block_delay": "5t",
            "disable_cooldown_scale": 0.5,
            "block_sound": "item.shield.block",
            "disable_sound": "item.shield.break",
            "bypassed_by": "bypasses_shield",
            "item_damage": { "threshold": 2, "base": 3, "factor": 0.75 },
            "damage_reductions": [
                { "base": 2, "factor": 0.5, "horizontal_blocking": 45, "type": "player_attack" },
                { "base": 1, "factor": 1, "type": "is_projectile" }
            ]
        },
        "attack_range": { "reach": "1..6", "creative_reach": "2..8", "hitbox_margin": 2, "mob_factor": 3 },
        "kinetic_weapon": {
            "contact_cooldown": "5t",
            "delay": "2t",
            "forward_movement": 0.3,
            "damage_multiplier": 2,
            "sound": "item.spear.use",
            "hit_sound": "item.spear.hit",
            "dismount_conditions": { "max_duration": "20t", "min_speed": 0.4, "min_relative_speed": 0.2 },
            "knockback_conditions": {}
        },
        "piercing_weapon": { "deals_knockback": false, "dismounts": true, "sound": "item.spear.use", "hit_sound": "item.spear.hit" },
        "swing_animation": { "type": "STAB", "duration": "10t" },
        "use_effects": { "can_sprint": true, "interact_vibrations": false, "speed_multiplier": 2 }
    })
    .as_object()
    .unwrap()
    .clone();
    let (components, diagnostics) = converted_components(source, "PAPER", "builder_components");
    let components = components_of(&components);

    assert_eq!(
        components["can_place_on"],
        json!([{ "blocks": "minecraft:stone" }, { "blocks": "#minecraft:planks" }])
    );
    assert_eq!(components["can_break"], json!({ "blocks": "minecraft:oak_log", "state": { "axis": "y" } }));
    assert_eq!(
        components["tool"],
        json!({
            "rules": [
                { "blocks": ["minecraft:stone", "minecraft:oak_log"], "speed": 4, "correct_for_drops": true },
                { "blocks": "#minecraft:mineable/pickaxe", "speed": 4, "correct_for_drops": true }
            ],
            "can_destroy_blocks_in_creative": false,
            "default_mining_speed": 2.5,
            "damage_per_block": 2
        })
    );
    assert_eq!(components["jukebox_playable"], json!("minecraft:precipice"));
    assert_eq!(components["use_remainder"], json!({ "id": "minecraft:bucket", "count": 3 }));
    assert_eq!(
        components["death_protection"],
        json!({ "death_effects": [
            { "type": "minecraft:apply_effects", "effects": [{
                "id": "minecraft:speed", "amplifier": 1, "duration": 40, "ambient": false, "show_particles": false, "show_icon": true
            }], "probability": 0.5 },
            { "type": "minecraft:remove_effects", "effects": "minecraft:poison" },
            { "type": "minecraft:clear_all_effects" },
            { "type": "minecraft:teleport_randomly", "diameter": 8 },
            { "type": "minecraft:play_sound", "sound": "minecraft:entity.player.levelup" }
        ] })
    );
    assert_eq!(
        components["consumable"],
        json!({
            "consume_seconds": 0.5,
            "animation": "drink",
            "has_consume_particles": false,
            "sound": "minecraft:entity.generic.drink",
            "on_consume_effects": [{ "type": "minecraft:clear_all_effects" }]
        })
    );
    assert_eq!(
        components["equippable"],
        json!({
            "slot": "mainhand",
            "allowed_entities": "minecraft:pig",
            "asset_id": "demo:bronze",
            "camera_overlay": "demo:misc/bronze",
            "equip_sound": "minecraft:item.armor.equip_chain",
            "shearing_sound": "minecraft:entity.sheep.shear",
            "dispensable": false,
            "swappable": false,
            "damage_on_hurt": false,
            "equip_on_interact": true,
            "can_be_sheared": true
        })
    );
    assert_eq!(components["repairable"], json!({ "items": ["minecraft:iron_ingot", "minecraft:diamond"] }));
    assert_eq!(components["weapon"], json!({ "item_damage_per_attack": 3, "disable_blocking_for_seconds": 0.5 }));
    assert_eq!(
        components["blocks_attacks"],
        json!({
            "block_delay_seconds": 0.25,
            "disable_cooldown_scale": 0.5,
            "block_sound": "minecraft:item.shield.block",
            "disabled_sound": "minecraft:item.shield.break",
            "bypassed_by": "#minecraft:bypasses_shield",
            "item_damage": { "threshold": 2, "base": 3, "factor": 0.75 },
            "damage_reductions": [
                { "base": 2, "factor": 0.5, "horizontal_blocking_angle": 45, "type": "minecraft:player_attack" },
                { "base": 1, "factor": 1, "type": "#minecraft:is_projectile" }
            ]
        })
    );
    assert_eq!(
        components["attack_range"],
        json!({ "min_reach": 1, "max_reach": 6, "min_creative_reach": 2, "max_creative_reach": 8, "hitbox_margin": 1, "mob_factor": 2 })
    );
    assert_eq!(
        components["kinetic_weapon"],
        json!({
            "contact_cooldown_ticks": 5,
            "delay_ticks": 2,
            "forward_movement": 0.3,
            "damage_multiplier": 2,
            "sound": "minecraft:item.spear.use",
            "hit_sound": "minecraft:item.spear.hit",
            "dismount_conditions": { "max_duration_ticks": 20, "min_speed": 0.4, "min_relative_speed": 0.2 },
            "knockback_conditions": { "max_duration_ticks": 0 }
        })
    );
    assert_eq!(
        components["piercing_weapon"],
        json!({ "deals_knockback": false, "dismounts": true, "sound": "minecraft:item.spear.use", "hit_sound": "minecraft:item.spear.hit" })
    );
    assert_eq!(components["swing_animation"], json!({ "type": "stab", "duration": 10 }));
    assert_eq!(components["use_effects"], json!({ "can_sprint": true, "interact_vibrations": false, "speed_multiplier": 1 }));
    assert!(!diagnostics.items.iter().any(|entry| entry.code == "COMPONENT_CODEC_MANUAL"));
    assert!(diagnostics.items.iter().any(|entry| entry.code == "COMPONENT_BLOCK_STATE_PROPERTY_IGNORED"));
}

#[test]
fn component_codec_manual_remains_only_for_runtime_or_external_state() {
    let source = json!({
        "can_place_on": { "nexo_block": "runtime_custom_block" },
        "use_remainder": { "nexo_item": "custom_bowl", "amount": 1 },
        "consumable": { "consume_duration": "1s" },
        "repairable": ["IRON_INGOT", "minecraft:planks"],
        "jukebox_playable": "minecraft:not_a_song"
    })
    .as_object()
    .unwrap()
    .clone();
    let (components, diagnostics) = converted_components(source, "PAPER", "builder_components");
    assert_eq!(components_of(&components), json!({ "consumable": { "consume_seconds": 1 } }));
    let manual: Vec<&nexo2ce::diagnostics::Diagnostic> = diagnostics
        .items
        .iter()
        .filter(|entry| entry.code == "COMPONENT_CODEC_MANUAL")
        .collect();
    assert_eq!(manual.len(), 4);
    assert!(manual.iter().all(|entry| entry.lossy));
    let mut fields: Vec<String> = manual.iter().map(|entry| entry.field.clone().unwrap_or_default()).collect();
    fields.sort();
    assert_eq!(
        fields,
        vec![
            "Components.can_place_on",
            "Components.jukebox_playable",
            "Components.repairable",
            "Components.use_remainder"
        ]
    );
}

#[test]
fn consumable_inherits_omitted_fields_from_vanilla_template() {
    let source = json!({ "consumable": { "consume_duration": "2s" } }).as_object().unwrap().clone();
    let (components, diagnostics) = converted_components(source, "CHORUS_FRUIT", "builder_components");
    assert_eq!(
        components_of(&components)["consumable"],
        json!({ "consume_seconds": 2, "on_consume_effects": [{ "type": "minecraft:teleport_randomly" }] })
    );
    assert_eq!(diagnostics.items.len(), 0);
}

#[test]
fn non_registered_sound_ids_use_inline_sound_event_codec() {
    let source = json!({
        "consumable": { "sound": "demo:snack" },
        "kinetic_weapon": { "sound": "demo:thrust", "hit_sound": "demo:impact" }
    })
    .as_object()
    .unwrap()
    .clone();
    let (components, diagnostics) = converted_components(source, "PAPER", "builder_components");
    let components = components_of(&components);
    assert_eq!(components["consumable"], json!({ "sound": { "sound_id": "demo:snack" } }));
    assert_eq!(
        components["kinetic_weapon"],
        json!({ "sound": { "sound_id": "demo:thrust" }, "hit_sound": { "sound_id": "demo:impact" } })
    );
    assert_eq!(diagnostics.items.len(), 0);
}

#[test]
fn positive_only_codecs_fall_back_to_manual() {
    let source = json!({
        "tool": { "rules": [{ "materials": ["STONE"], "speed": 0 }] },
        "blocks_attacks": { "damage_reductions": [{ "horizontal_blocking": 0 }] },
        "swing_animation": { "duration": "0t" },
        "death_protection": { "death_effects": { "TELEPORT_RANDOMLY": { "diameter": 0 } } }
    })
    .as_object()
    .unwrap()
    .clone();
    let (components, diagnostics) = converted_components(source, "PAPER", "builder_components");
    assert!(components.is_none());
    let mut fields: Vec<String> = diagnostics
        .items
        .iter()
        .filter(|entry| entry.code == "COMPONENT_CODEC_MANUAL")
        .map(|entry| entry.field.clone().unwrap_or_default())
        .collect();
    fields.sort();
    assert_eq!(
        fields,
        vec![
            "Components.blocks_attacks",
            "Components.death_protection",
            "Components.swing_animation",
            "Components.tool"
        ]
    );
}

#[test]
fn equippable_inferred_defaults_only_when_codec_defaults_differ() {
    let source = json!({ "equippable": { "slot": "CHEST" } }).as_object().unwrap().clone();
    let (components, diagnostics) = converted_components(source, "ELYTRA", "horse_harness");
    assert_eq!(
        components_of(&components)["equippable"],
        json!({ "slot": "chest", "damage_on_hurt": false, "can_be_sheared": true })
    );
    assert_eq!(diagnostics.items.len(), 0);
}
