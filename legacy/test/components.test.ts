import assert from "node:assert/strict";
import test from "node:test";
import { DiagnosticBag } from "../src/diagnostics.js";
import { convertItem, type ResolvedItem } from "../src/items.js";
import type { JsonObject } from "../src/types.js";

function convertedComponents(source: JsonObject, material = "PAPER", id = "builder_components") {
  const diagnostics = new DiagnosticBag();
  const item: ResolvedItem = {
    id,
    source: "components.yml",
    template: false,
    templateIds: [],
    config: { material, Components: source },
  };
  const converted = convertItem(item, { namespace: "demo", clientMode: "modern" }, undefined, diagnostics)!;
  const data = converted.config.data as JsonObject | undefined;
  return { components: data?.components as JsonObject | undefined, diagnostics };
}

test("all 16 Nexo builder Components serialize to Minecraft 1.21.11 codec shapes", () => {
  const { components, diagnostics } = convertedComponents({
    can_place_on: [{ block: ["STONE", "minecraft:planks"] }],
    can_break: { block: "OAK_LOG", state: { axis: "y", unknown: "ignored" } },
    tool: {
      default_mining_speed: 2.5,
      damage_per_block: 2,
      rules: [{ materials: ["STONE", "OAK_LOG"], tag: "mineable/pickaxe", speed: 4, correct_for_drops: true }],
    },
    jukebox_playable: "minecraft:precipice",
    use_remainder: { minecraft_type: "BUCKET", amount: 3 },
    death_protection: { death_effects: {
      APPLY_EFFECTS: { speed: { amplifier: 1, duration: "2s", ambient: false, show_particles: false, show_icon: true, probability: 0.5 } },
      REMOVE_EFFECTS: ["poison"],
      CLEAR_ALL_EFFECTS: true,
      TELEPORT_RANDOMLY: { diameter: 8 },
      PLAY_SOUND: { sound: "entity.player.levelup", range: 2 },
    } },
    consumable: {
      consume_duration: "10t",
      animation: "drink",
      consume_particles: false,
      sound: "entity.generic.drink",
      effects: { CLEAR_ALL_EFFECTS: true },
    },
    glider: false,
    equippable: {
      slot: "HAND",
      allowed_entity_types: ["PIG"],
      asset_id: "demo:bronze",
      camera_overlay: "demo:misc/bronze",
      equip_sound: "item.armor.equip_chain",
      dispensable: false,
      swappable: false,
      damage_on_hurt: false,
      equip_on_interact: true,
      can_be_sheared: true,
      shear_sound: "entity.sheep.shear",
    },
    repairable: ["IRON_INGOT", "DIAMOND"],
    weapon: { damage_per_attack: 3, disable_blocking: "10t" },
    blocks_attacks: {
      block_delay: "5t",
      disable_cooldown_scale: 0.5,
      block_sound: "item.shield.block",
      disable_sound: "item.shield.break",
      bypassed_by: "bypasses_shield",
      item_damage: { threshold: 2, base: 3, factor: 0.75 },
      damage_reductions: [
        { base: 2, factor: 0.5, horizontal_blocking: 45, type: "player_attack" },
        { base: 1, factor: 1, type: "is_projectile" },
      ],
    },
    attack_range: { reach: "1..6", creative_reach: "2..8", hitbox_margin: 2, mob_factor: 3 },
    kinetic_weapon: {
      contact_cooldown: "5t",
      delay: "2t",
      forward_movement: 0.3,
      damage_multiplier: 2,
      sound: "item.spear.use",
      hit_sound: "item.spear.hit",
      dismount_conditions: { max_duration: "20t", min_speed: 0.4, min_relative_speed: 0.2 },
      knockback_conditions: {},
    },
    piercing_weapon: { deals_knockback: false, dismounts: true, sound: "item.spear.use", hit_sound: "item.spear.hit" },
    swing_animation: { type: "STAB", duration: "10t" },
    use_effects: { can_sprint: true, interact_vibrations: false, speed_multiplier: 2 },
  });

  assert.ok(components);
  assert.deepEqual(components.can_place_on, [
    { blocks: "minecraft:stone" },
    { blocks: "#minecraft:planks" },
  ]);
  assert.deepEqual(components.can_break, { blocks: "minecraft:oak_log", state: { axis: "y" } });
  assert.deepEqual(components.tool, {
    rules: [
      { blocks: ["minecraft:stone", "minecraft:oak_log"], speed: 4, correct_for_drops: true },
      { blocks: "#minecraft:mineable/pickaxe", speed: 4, correct_for_drops: true },
    ],
    can_destroy_blocks_in_creative: false,
    default_mining_speed: 2.5,
    damage_per_block: 2,
  });
  assert.equal(components.jukebox_playable, "minecraft:precipice");
  assert.deepEqual(components.use_remainder, { id: "minecraft:bucket", count: 3 });
  assert.deepEqual(components.death_protection, { death_effects: [
    { type: "minecraft:apply_effects", effects: [{
      id: "minecraft:speed", amplifier: 1, duration: 40, ambient: false, show_particles: false, show_icon: true,
    }], probability: 0.5 },
    { type: "minecraft:remove_effects", effects: "minecraft:poison" },
    { type: "minecraft:clear_all_effects" },
    { type: "minecraft:teleport_randomly", diameter: 8 },
    { type: "minecraft:play_sound", sound: "minecraft:entity.player.levelup" },
  ] });
  assert.deepEqual(components.consumable, {
    consume_seconds: 0.5,
    animation: "drink",
    has_consume_particles: false,
    sound: "minecraft:entity.generic.drink",
    on_consume_effects: [{ type: "minecraft:clear_all_effects" }],
  });
  assert.deepEqual(components.equippable, {
    slot: "mainhand",
    allowed_entities: "minecraft:pig",
    asset_id: "demo:bronze",
    camera_overlay: "demo:misc/bronze",
    equip_sound: "minecraft:item.armor.equip_chain",
    shearing_sound: "minecraft:entity.sheep.shear",
    dispensable: false,
    swappable: false,
    damage_on_hurt: false,
    equip_on_interact: true,
    can_be_sheared: true,
  });
  assert.deepEqual(components.repairable, { items: ["minecraft:iron_ingot", "minecraft:diamond"] });
  assert.deepEqual(components.weapon, { item_damage_per_attack: 3, disable_blocking_for_seconds: 0.5 });
  assert.deepEqual(components.blocks_attacks, {
    block_delay_seconds: 0.25,
    disable_cooldown_scale: 0.5,
    block_sound: "minecraft:item.shield.block",
    disabled_sound: "minecraft:item.shield.break",
    bypassed_by: "#minecraft:bypasses_shield",
    item_damage: { threshold: 2, base: 3, factor: 0.75 },
    damage_reductions: [
      { base: 2, factor: 0.5, horizontal_blocking_angle: 45, type: "minecraft:player_attack" },
      { base: 1, factor: 1, type: "#minecraft:is_projectile" },
    ],
  });
  assert.deepEqual(components.attack_range, {
    min_reach: 1, max_reach: 6, min_creative_reach: 2, max_creative_reach: 8, hitbox_margin: 1, mob_factor: 2,
  });
  assert.deepEqual(components.kinetic_weapon, {
    contact_cooldown_ticks: 5,
    delay_ticks: 2,
    forward_movement: 0.3,
    damage_multiplier: 2,
    sound: "minecraft:item.spear.use",
    hit_sound: "minecraft:item.spear.hit",
    dismount_conditions: { max_duration_ticks: 20, min_speed: 0.4, min_relative_speed: 0.2 },
    knockback_conditions: { max_duration_ticks: 0 },
  });
  assert.deepEqual(components.piercing_weapon, {
    deals_knockback: false, dismounts: true, sound: "minecraft:item.spear.use", hit_sound: "minecraft:item.spear.hit",
  });
  assert.deepEqual(components.swing_animation, { type: "stab", duration: 10 });
  assert.deepEqual(components.use_effects, { can_sprint: true, interact_vibrations: false, speed_multiplier: 1 });
  assert.equal(diagnostics.items.some((entry) => entry.code === "COMPONENT_CODEC_MANUAL"), false);
  assert.ok(diagnostics.items.some((entry) => entry.code === "COMPONENT_BLOCK_STATE_PROPERTY_IGNORED"));
});

test("COMPONENT_CODEC_MANUAL remains only for runtime-registry or external ItemStack state", () => {
  const { components, diagnostics } = convertedComponents({
    can_place_on: { nexo_block: "runtime_custom_block" },
    use_remainder: { nexo_item: "custom_bowl", amount: 1 },
    consumable: { consume_duration: "1s" },
    repairable: ["IRON_INGOT", "minecraft:planks"],
    jukebox_playable: "minecraft:not_a_song",
  });

  assert.deepEqual(components, { consumable: { consume_seconds: 1 } });
  const manual = diagnostics.items.filter((entry) => entry.code === "COMPONENT_CODEC_MANUAL");
  assert.equal(manual.length, 4);
  assert.ok(manual.every((entry) => entry.lossy));
  assert.deepEqual(manual.map((entry) => entry.field).sort(), [
    "Components.can_place_on",
    "Components.jukebox_playable",
    "Components.repairable",
    "Components.use_remainder",
  ]);
});

test("consumable inherits omitted fields from the locked vanilla ItemStack template", () => {
  const { components, diagnostics } = convertedComponents({ consumable: { consume_duration: "2s" } }, "CHORUS_FRUIT");
  assert.deepEqual(components?.consumable, {
    consume_seconds: 2,
    on_consume_effects: [{ type: "minecraft:teleport_randomly" }],
  });
  assert.equal(diagnostics.items.length, 0);
});

test("non-registered sound IDs use the inline SoundEvent codec shape", () => {
  const { components, diagnostics } = convertedComponents({
    consumable: { sound: "demo:snack" },
    kinetic_weapon: { sound: "demo:thrust", hit_sound: "demo:impact" },
  });
  assert.deepEqual(components?.consumable, { sound: { sound_id: "demo:snack" } });
  assert.deepEqual(components?.kinetic_weapon, {
    sound: { sound_id: "demo:thrust" },
    hit_sound: { sound_id: "demo:impact" },
  });
  assert.equal(diagnostics.items.length, 0);
});

test("positive-only Minecraft codecs fall back to manual instead of emitting invalid NBT", () => {
  const { components, diagnostics } = convertedComponents({
    tool: { rules: [{ materials: ["STONE"], speed: 0 }] },
    blocks_attacks: { damage_reductions: [{ horizontal_blocking: 0 }] },
    swing_animation: { duration: "0t" },
    death_protection: { death_effects: { TELEPORT_RANDOMLY: { diameter: 0 } } },
  });
  assert.equal(components, undefined);
  const fields = diagnostics.items.filter((entry) => entry.code === "COMPONENT_CODEC_MANUAL").map((entry) => entry.field).sort();
  assert.deepEqual(fields, [
    "Components.blocks_attacks",
    "Components.death_protection",
    "Components.swing_animation",
    "Components.tool",
  ]);
});

test("equippable inferred Nexo defaults are made explicit only when codec defaults differ", () => {
  const { components, diagnostics } = convertedComponents({ equippable: { slot: "CHEST" } }, "ELYTRA", "horse_harness");
  assert.deepEqual(components?.equippable, {
    slot: "chest",
    damage_on_hurt: false,
    can_be_sheared: true,
  });
  assert.equal(diagnostics.items.length, 0);
});
