import assert from "node:assert/strict";
import { link, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { parse, stringify } from "yaml";
import { convert } from "../src/converter.js";
import { DiagnosticBag } from "../src/diagnostics.js";
import { compactFurnitureDefinition, mergeFurnitureTemplates } from "../src/furniture-templates.js";
import { convertGlyphs, rewriteGlyphTags } from "../src/glyphs.js";
import { convertItem, matchBukkitMaterial, resolveItemTemplates, type ResolvedItem, type SourceItem } from "../src/items.js";
import { loadYaml } from "../src/io.js";
import { convertMechanics } from "../src/mechanics.js";
import { MINECRAFT_1_21_11_SOLID_BLOCK_COUNT, MINECRAFT_1_21_11_SOLID_BLOCK_PATTERN } from "../src/minecraft-1.21.11.js";
import { discoverModelAliases } from "../src/model-aliases.js";
import { inferAuthorNamespaceFromBundlePaths } from "../src/source-namespace.js";
import { buildLegacyModel, convertModels, readPackModel } from "../src/models.js";
import { convertRecipe } from "../src/recipes.js";
import { normalizeModelLocation, normalizeTextureLocation } from "../src/resource-location.js";
import { convertSounds } from "../src/sounds.js";
import { isObject, type JsonObject } from "../src/types.js";
import { expandCraftEngineTemplateEntry } from "./craftengine-template-expander.js";

function context(diagnostics: DiagnosticBag, item = "demo") {
  return { source: "fixture.yml", item, diagnostics };
}

test("locked Minecraft 1.21.11 solidity table matches Bukkit semantics", () => {
  const pattern = new RegExp("^(?:" + MINECRAFT_1_21_11_SOLID_BLOCK_PATTERN + ")$");
  assert.equal(MINECRAFT_1_21_11_SOLID_BLOCK_COUNT, 913);
  for (const solid of ["minecraft:stone", "minecraft:oak_door", "minecraft:oak_slab", "minecraft:white_banner"]) {
    assert.equal(pattern.test(solid), true);
  }
  for (const nonSolid of ["minecraft:air", "minecraft:water", "minecraft:dandelion", "minecraft:oak_button"]) {
    assert.equal(pattern.test(nonSolid), false);
  }
});

test("resource locations use Minecraft's real default namespace", () => {
  const diagnostics = new DiagnosticBag();
  assert.equal(normalizeModelLocation("custom/chair.json", diagnostics, {}), "minecraft:custom/chair");
  assert.equal(normalizeTextureLocation("demo:block/chair.png", diagnostics, {}), "demo:block/chair");
  assert.equal(diagnostics.items.length, 0);
});

test("strict YAML loader rejects duplicate mapping keys", async () => {
  const root = await mkdtemp(join(tmpdir(), "nexo2ce-yaml-"));
  try {
    const file = join(root, "duplicate.yml");
    await writeFile(file, "demo:\n  material: PAPER\n  material: STONE\n", "utf8");
    const diagnostics = new DiagnosticBag();
    assert.equal(await loadYaml(file, diagnostics), undefined);
    assert.ok(diagnostics.items.some((entry) => entry.code === "YAML_INVALID"));
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("converter rejects destructive or recursive output path overlap before --force deletion", async () => {
  const temp = await mkdtemp(join(tmpdir(), "nexo2ce-path-safety-"));
  const bundle = join(temp, "bundle");
  const input = join(bundle, "Nexo");
  const itemFile = join(input, "items", "demo.yml");
  try {
    await mkdir(join(input, "items"), { recursive: true });
    await mkdir(join(input, "pack", "assets"), { recursive: true });
    await writeFile(itemFile, "demo:\n  material: PAPER\n", "utf8");
    const options = { clientMode: "modern" as const, cmdPolicy: "preserve" as const, strict: false, force: true, audit: false };
    await assert.rejects(convert({ input, output: bundle, namespace: "demo", ...options }), /must not overlap/u);
    assert.match(await readFile(itemFile, "utf8"), /demo:/u, "ancestor output must not delete the source bundle");
    await assert.rejects(convert({
      input,
      output: join(input, "pack", "assets", "generated-output"),
      namespace: "demo",
      ...options,
    }), /must not overlap/u);

    const linkedInput = join(temp, "linked", "Nexo");
    const externalItems = join(temp, "external-items");
    await mkdir(linkedInput, { recursive: true });
    await mkdir(externalItems, { recursive: true });
    const externalItem = join(externalItems, "linked.yml");
    await writeFile(externalItem, "linked:\n  material: PAPER\n", "utf8");
    await symlink(externalItems, join(linkedInput, "items"), "junction");
    await assert.rejects(convert({
      input: linkedInput, output: externalItems, namespace: "demo", ...options,
    }), /must not overlap/u);
    assert.match(await readFile(externalItem, "utf8"), /linked:/u, "linked item sources must not be deleted");
  } finally { await rm(temp, { recursive: true, force: true }); }
});

test("converter loads valid YAML from both items and item directories", async () => {
  const temp = await mkdtemp(join(tmpdir(), "nexo2ce-dual-items-"));
  const input = join(temp, "Nexo");
  const output = join(temp, "output");
  try {
    await mkdir(join(input, "items"), { recursive: true });
    await mkdir(join(input, "item"), { recursive: true });
    await writeFile(join(input, "items", "plural.yml"), "plural:\n  material: PAPER\n", "utf8");
    await writeFile(join(input, "item", "singular.yml"), "singular:\n  material: STICK\n", "utf8");
    const result = await convert({
      input, output, namespace: "demo", clientMode: "modern", cmdPolicy: "preserve",
      strict: false, force: false, audit: false,
    });
    assert.equal(result.success, true, result.diagnostics.formatLines().join("\n"));
    assert.equal(result.itemCount, 2);
    const yaml = parse(await readFile(join(output, "configuration", "items.yml"), "utf8")) as JsonObject;
    assert.deepEqual(Object.keys(yaml.items as JsonObject).sort(), ["demo:plural", "demo:singular"]);

    const aliasInput = join(temp, "AliasNexo");
    const aliasSource = join(temp, "alias-source");
    await mkdir(aliasInput, { recursive: true });
    await mkdir(aliasSource, { recursive: true });
    await writeFile(join(aliasSource, "only.yml"), "only:\n  material: PAPER\n", "utf8");
    await symlink(aliasSource, join(aliasInput, "items"), "junction");
    await symlink(aliasSource, join(aliasInput, "item"), "junction");
    const aliasResult = await convert({
      input: aliasInput, output: join(temp, "alias-output"), namespace: "demo",
      clientMode: "modern", cmdPolicy: "preserve", strict: false, force: false, audit: false,
    });
    assert.equal(aliasResult.success, true, aliasResult.diagnostics.formatLines().join("\n"));
    assert.equal(aliasResult.itemCount, 1, "item/items aliases to one physical directory must load once");
    assert.equal(aliasResult.diagnostics.items.some((entry) => entry.code === "DUPLICATE_ITEM_ID"), false);

    const hardlinkInput = join(temp, "HardlinkNexo");
    await mkdir(join(hardlinkInput, "items"), { recursive: true });
    await mkdir(join(hardlinkInput, "item"), { recursive: true });
    const original = join(hardlinkInput, "items", "original.yml");
    await writeFile(original, "hardlinked:\n  material: PAPER\n", "utf8");
    await link(original, join(hardlinkInput, "item", "alias.yml"));
    const hardlinkResult = await convert({
      input: hardlinkInput, output: join(temp, "hardlink-output"), namespace: "demo",
      clientMode: "modern", cmdPolicy: "preserve", strict: false, force: false, audit: false,
    });
    assert.equal(hardlinkResult.success, true, hardlinkResult.diagnostics.formatLines().join("\n"));
    assert.equal(hardlinkResult.itemCount, 1, "hard-linked item YAML aliases must load once");
    assert.equal(hardlinkResult.diagnostics.items.some((entry) => entry.code === "DUPLICATE_ITEM_ID"), false);
  } finally { await rm(temp, { recursive: true, force: true }); }
});

test("Nexo bow shortcut thresholds and condition tree match actual generator", () => {
  const diagnostics = new DiagnosticBag();
  const pack: JsonObject = {
    model: "demo:item/bow",
    pulling_models: ["demo:item/bow_0", "demo:item/bow_1", "demo:item/bow_2"],
  };
  const info = readPackModel(pack, "bow", context(diagnostics));
  const converted = convertModels(info, undefined, "bow", undefined, "modern", context(diagnostics));
  const model = converted.model as JsonObject;
  assert.equal(model.type, "condition");
  assert.equal(model.property, "using_item");
  const dispatch = model.on_true as JsonObject;
  assert.equal(dispatch.property, "use_duration");
  assert.equal(dispatch.scale, 0.05);
  const entries = dispatch.entries as JsonObject[];
  assert.deepEqual(entries.map((entry) => entry.threshold), [0, 0.65, 0.9]);
});

test("Nexo crossbow modern tree wraps pulling in charge type select", () => {
  const diagnostics = new DiagnosticBag();
  const info = readPackModel({
    model: "demo:item/crossbow",
    pulling_models: ["demo:item/pull_0", "demo:item/pull_1"],
    charged_model: "demo:item/arrow",
    firework_model: "demo:item/rocket",
  }, "crossbow", context(diagnostics));
  const model = convertModels(info, undefined, "crossbow", undefined, "modern", context(diagnostics)).model as JsonObject;
  assert.equal(model.type, "select");
  assert.equal(model.property, "charge_type");
  assert.deepEqual((model.cases as JsonObject[]).map((entry) => entry.when), ["arrow", "rocket"]);
  assert.equal((model.fallback as JsonObject).type, "condition");
});

test("legacy damaged_models preserves Nexo pulling predicate quirk", () => {
  const diagnostics = new DiagnosticBag();
  const info = readPackModel({ model: "demo:item/tool", damaged_models: ["demo:item/d0", "demo:item/d1", "demo:item/d2"] }, "tool", context(diagnostics));
  const legacy = buildLegacyModel(info);
  const overrides = legacy.overrides as JsonObject[];
  assert.equal(overrides.length, 2);
  assert.deepEqual(overrides.map((entry) => entry.predicate), [{ pulling: 1, damage: 0.35 }, { pulling: 1, damage: 0.65 }]);
});

test("template inheritance is recursive and item values override templates", () => {
  const diagnostics = new DiagnosticBag();
  const items: SourceItem[] = [
    { id: "base", source: "a.yml", template: false, config: { material: "PAPER", itemname: "<item_id_capitalized>", lore: ["base"] } },
    { id: "mid", source: "a.yml", template: false, config: { template: "base", Pack: { model: "demo:<item_id>" } } },
    { id: "tea_set", source: "b.yml", template: false, config: { template: "mid", material: "STICK" } },
    { id: "invalid_child", source: "b.yml", template: false, config: { template: "mid", material: "not a material" } },
  ];
  const resolved = resolveItemTemplates(items, diagnostics);
  const item = resolved.find((entry) => entry.id === "tea_set")!;
  assert.equal(item.config.material, "STICK");
  assert.equal(item.config.itemname, "Tea Set");
  assert.equal((item.config.Pack as JsonObject).model, "demo:tea_set");
  assert.equal(resolved.find((entry) => entry.id === "invalid_child")!.config.material, "PAPER");
  assert.ok(diagnostics.items.some((entry) => entry.code === "INVALID_MATERIAL_INHERITED"));
  assert.equal(resolved.find((entry) => entry.id === "base")!.template, true);
  assert.equal(resolved.find((entry) => entry.id === "mid")!.template, true);
});

test("modern compound custom_model_data remains a component while Pack CMD is root metadata", () => {
  const diagnostics = new DiagnosticBag();
  const item: ResolvedItem = {
    id: "demo", source: "fixture.yml", template: false, templateIds: [],
    config: {
      material: "PAPER",
      Pack: { model: "demo:item/demo", custom_model_data: 1234 },
      Components: { custom_model_data: { floats: [1.5], flags: [true] }, item_model: "demo:special" },
    },
  };
  const converted = convertItem(item, { namespace: "demo", clientMode: "hybrid" }, 1234, diagnostics)!;
  assert.equal(converted.config.custom_model_data, 1234);
  assert.equal(converted.config.item_model, "demo:special");
  const data = converted.config.data as JsonObject;
  assert.deepEqual((data.components as JsonObject).custom_model_data, { floats: [1.5], flags: [true] });
  assert.equal(data.custom_model_data, undefined);
});

test("root item coercions follow Nexo and invalid materials fall back to PAPER", () => {
  const diagnostics = new DiagnosticBag();
  assert.equal(matchBukkitMaterial("IRON SWORD"), "iron_sword");
  assert.equal(matchBukkitMaterial("minecraft:paper"), "paper");
  assert.equal(matchBukkitMaterial("LEGACY_STONE"), undefined);
  const item: ResolvedItem = {
    id: "root_fields", source: "fixture.yml", template: false, templateIds: [],
    config: {
      material: "not a material", itemname: 42, customname: false, lore: "scalar lore",
      color: 16711680, unbreakable: "true", max_durability: 99,
      trim_pattern: "sentry",
      Enchantments: { sharpness: 3 },
    },
  };
  const converted = convertItem(item, { namespace: "demo", clientMode: "modern" }, undefined, diagnostics)!;
  assert.equal(converted.config.material, "paper");
  const data = converted.config.data as JsonObject;
  assert.equal(data.item_name, undefined);
  assert.equal(data.custom_name, undefined);
  assert.equal(data.lore, undefined);
  assert.equal(data.dyed_color, undefined);
  assert.equal(data.unbreakable, false);
  assert.equal(data.max_damage, undefined);
  assert.deepEqual(data.trim, { pattern: "minecraft:sentry", material: "minecraft:redstone" });
  assert.deepEqual(data.enchantments, { "minecraft:sharpness": 3 });
  assert.ok(diagnostics.items.some((entry) => entry.code === "INVALID_MATERIAL_DEFAULTED"));
  assert.ok(diagnostics.items.some((entry) => entry.code === "ROOT_MAX_DURABILITY_IGNORED"));
});

test("Components use Nexo's exact whitelist, clamps, and vanilla codec shapes", () => {
  const diagnostics = new DiagnosticBag();
  const item: ResolvedItem = {
    id: "components", source: "fixture.yml", template: false, templateIds: [],
    config: { material: "PAPER", Components: {
      max_stack_size: 0,
      max_damage: -4,
      food: { nutrition: 3.9, saturation: 1.25, can_always_eat: true },
      painting_variant: "kebab",
      use_cooldown: { duration: "20t" },
      enchantable: 0,
      glider: true,
      minimum_attack_charge: 2,
      tooltip_display: ["minecraft:lore", "custom_model_data"],
      tool: { damage_per_block: 1 },
      Potion_Contents: { potion: "minecraft:water" },
    } },
  };
  const converted = convertItem(item, { namespace: "demo", clientMode: "modern" }, undefined, diagnostics)!;
  const components = ((converted.config.data as JsonObject).components as JsonObject);
  assert.equal(components.max_stack_size, 1);
  assert.equal(components.max_damage, 1);
  assert.deepEqual(components.food, { nutrition: 3, saturation: 1.25, can_always_eat: true });
  assert.equal(components["painting/variant"], "minecraft:kebab");
  assert.deepEqual(components.use_cooldown, { seconds: 1, cooldown_group: "nexo:components" });
  assert.equal(components.enchantable, 1);
  assert.deepEqual(components.glider, {});
  assert.equal(components.minimum_attack_charge, 1);
  assert.deepEqual(components.tooltip_display, { hide_tooltip: false, hidden_components: ["minecraft:lore", "minecraft:custom_model_data"] });
  assert.equal(components.tool, undefined);
  assert.equal(components.Potion_Contents, undefined);
  assert.ok(diagnostics.items.some((entry) => entry.code === "COMPONENT_CODEC_MANUAL" && entry.lossy));
  assert.ok(diagnostics.items.some((entry) => entry.code === "NEXO_COMPONENT_UNKNOWN_IGNORED"));
});

test("explicit Components.item_model survives without a generated local model", () => {
  const diagnostics = new DiagnosticBag();
  const item: ResolvedItem = {
    id: "pointer", source: "fixture.yml", template: false, templateIds: [],
    config: { material: "PAPER", Components: { item_model: "demo:external" } },
  };
  const converted = convertItem(item, { namespace: "demo", clientMode: "modern" }, undefined, diagnostics)!;
  assert.equal(converted.config.item_model, "demo:external");
  assert.equal(converted.config.model, undefined);
});

test("explicit ItemModel root metadata overrides CE's incompatible defaults", () => {
  const diagnostics = new DiagnosticBag();
  const configured: ResolvedItem = {
    id: "metadata", source: "fixture.yml", template: false, templateIds: [],
    config: {
      material: "PAPER",
      ItemModel: {
        type: "minecraft:model", model: "demo:item/metadata",
        hand_animation_on_swap: false, oversized_in_gui: true, swap_animation_scale: 0.25,
      },
    },
  };
  const converted = convertItem(configured, { namespace: "demo", clientMode: "modern" }, undefined, diagnostics)!;
  assert.equal(converted.config.hand_animation_on_swap, false);
  assert.equal(converted.config.oversized_in_gui, true);
  assert.equal(converted.config.swap_animation_scale, 0.25);
  assert.deepEqual(converted.config.model, { type: "model", path: "demo:item/metadata" });

  const defaults: ResolvedItem = {
    id: "defaults", source: "fixture.yml", template: false, templateIds: [],
    config: { material: "PAPER", ItemModel: { type: "model", model: "demo:item/defaults" } },
  };
  const defaulted = convertItem(defaults, { namespace: "demo", clientMode: "modern" }, undefined, diagnostics)!;
  assert.equal(defaulted.config.hand_animation_on_swap, true);
  assert.equal(defaulted.config.oversized_in_gui, false);
  assert.equal(defaulted.config.swap_animation_scale, 1);
});

test("Nexo PotionEffects become exact 1.21.11 potion_contents entries", () => {
  const diagnostics = new DiagnosticBag();
  const item: ResolvedItem = {
    id: "tonic", source: "fixture.yml", template: false, templateIds: [],
    config: {
      material: "POTION", color: "255,0,16",
      PotionEffects: [
        { type: "SPEED", duration: 100, amplifier: 2 },
        { type: "minecraft:slowness", duration: 20, amplifier: 0, ambient: true, "has-particles": false },
        { effect: "demo:stun", duration: 40, amplifier: 1, "has-icon": false },
      ],
      Components: { potion_contents: { potion: "minecraft:strong_healing" } },
    },
  };
  const converted = convertItem(item, { namespace: "demo", clientMode: "modern" }, undefined, diagnostics)!;
  const data = converted.config.data as JsonObject;
  const potion = ((data.components as JsonObject).potion_contents as JsonObject);
  assert.equal(potion.custom_color, 0xff0010);
  assert.deepEqual(potion.custom_effects, [
    { id: "minecraft:speed", duration: 100, amplifier: 2, ambient: false, show_particles: true, show_icon: true },
    { id: "minecraft:slowness", duration: 20, amplifier: 0, ambient: true, show_particles: false, show_icon: false },
    { id: "demo:stun", duration: 40, amplifier: 1, ambient: false, show_particles: true, show_icon: false },
  ]);
  assert.ok(diagnostics.items.some((entry) => entry.code === "NEXO_COMPONENT_POTION_CONTENTS_IGNORED"));
  assert.equal(diagnostics.items.some((entry) => entry.code === "POTION_EFFECTS_MANUAL"), false);
});

test("Components.unset_components is applied after generated PotionEffects", () => {
  const diagnostics = new DiagnosticBag();
  const item: ResolvedItem = {
    id: "cleared", source: "fixture.yml", template: false, templateIds: [],
    config: {
      material: "PAPER",
      PotionEffects: [{ type: "speed", duration: 20, amplifier: 0 }],
      Components: { unset_components: ["minecraft:potion_contents"] },
    },
  };
  const converted = convertItem(item, { namespace: "demo", clientMode: "modern" }, undefined, diagnostics)!;
  const data = converted.config.data as JsonObject;
  assert.equal((data.components as JsonObject | undefined)?.potion_contents, undefined);
  assert.deepEqual(data.remove_components, ["potion_contents"]);
});

test("invalid PotionEffects mirror Bukkit construction failure diagnostics", () => {
  const diagnostics = new DiagnosticBag();
  const item: ResolvedItem = {
    id: "bad_tonic", source: "fixture.yml", template: false, templateIds: [],
    config: { material: "PAPER", PotionEffects: [{ type: "speed", duration: 20.5, amplifier: "1" }] },
  };
  const converted = convertItem(item, { namespace: "demo", clientMode: "modern" }, undefined, diagnostics)!;
  assert.equal((converted.config.data as JsonObject | undefined)?.components, undefined);
  assert.ok(diagnostics.items.some((entry) => entry.code === "POTION_EFFECT_INTEGER_REQUIRED" && entry.severity === "error"));
});

test("furniture defaults preserve Nexo STRICT and FIXED scale semantics", () => {
  const diagnostics = new DiagnosticBag();
  const config: JsonObject = {
    Mechanics: { furniture: {
      limited_placing: { floor: true, roof: false, wall: false },
      properties: { display_transform: "FIXED" },
      hitbox: { interactions: ["0,0,0 1,2"] },
    } },
  };
  const converted = convertMechanics(config, "demo:chair", "demo:item/chair", diagnostics, "fixture.yml", "chair");
  const behavior = converted.behavior[0]!;
  assert.equal((((behavior.rules as JsonObject).ground as JsonObject).rotation), "eight");
  assert.deepEqual(converted.furniture!.loot, {
    pools: [{ rolls: 1, entries: [{ type: "furniture_item", item: "demo:chair" }] }],
  });
  const variants = converted.furniture!.variants as JsonObject;
  assert.deepEqual(Object.keys(variants), ["ground"]);
  const element = (((variants.ground as JsonObject).elements as JsonObject[])[0])!;
  assert.equal(element.scale, "0.5,0.5,0.5");
  assert.equal(element.pitch, -90);
  assert.equal(element.position, undefined);
});

test("limited_placing preserves Nexo nested false/default plane semantics", () => {
  const diagnostics = new DiagnosticBag();
  const config: JsonObject = { Mechanics: { furniture: {
    limited_placing: { floor: false, roof: true },
    properties: { offset_against_blocks: false },
    hitbox: {},
  } } };
  const converted = convertMechanics(config, "demo:x", "demo:item/x", diagnostics, "fixture.yml", "x");
  assert.deepEqual(Object.keys(converted.furniture!.variants as JsonObject), ["ceiling", "wall"]);
});

test("FIXED floor/roof quarter turns use CE-stable equivalent transforms", () => {
  const diagnostics = new DiagnosticBag();
  const config: JsonObject = { Mechanics: { furniture: {
    limited_placing: { floor: true, roof: true, wall: false },
    properties: { display_transform: "FIXED", offset_against_blocks: false },
    hitbox: {
      interaction: ["1,2,3 1,2"],
      shulker: ["1,2,3 bad bad down"],
      ghast: ["1,2,3 bad true"],
    },
    seats: ["1,0.6,2"],
  } } };
  const converted = convertMechanics(config, "demo:x", "demo:item/x", diagnostics, "fixture.yml", "x");
  const variants = converted.furniture!.variants as JsonObject;
  const ground = variants.ground as JsonObject;
  const ceiling = variants.ceiling as JsonObject;
  const groundElement = (ground.elements as JsonObject[])[0]!;
  const ceilingElement = (ceiling.elements as JsonObject[])[0]!;
  assert.equal(groundElement.pitch, 90);
  assert.equal(groundElement.yaw, undefined);
  assert.equal(groundElement.rotation, "0,1,0,0");
  assert.equal(groundElement.position, undefined);
  assert.equal(ceilingElement.pitch, -90);
  assert.equal(ceilingElement.yaw, undefined);
  assert.equal(ceilingElement.rotation, "0,1,0,0");
  assert.equal(ceilingElement.position, "0,-0.01,0");

  const groundHitboxes = ground.hitboxes as JsonObject[];
  assert.equal(groundHitboxes[0]!.position, "-1,1.5,-3"); // Nexo packet origin is display Y - 0.5.
  assert.equal(groundHitboxes[1]!.position, "-1,2,-3"); // Shulker uses exact display base.
  assert.equal(groundHitboxes[1]!.scale, 1); // invalid compact numbers use Nexo defaults
  assert.equal(groundHitboxes[1]!.peek, 0);
  assert.equal(groundHitboxes[2]!.scale, 0.25);
  const ceilingHitboxes = ceiling.hitboxes as JsonObject[];
  assert.equal(ceilingHitboxes[0]!.position, "-1,1.49,-3");
  assert.ok(diagnostics.items.some((entry) => entry.code === "GHAST_VISIBLE_UNSUPPORTED"));

  const seatProxy = groundHitboxes[3]!;
  assert.equal(seatProxy.position, "-1,0.6,-2");
  assert.deepEqual(seatProxy.seats, ["-1,0.1,-2"]);
  const ceilingSeat = (ceiling.hitboxes as JsonObject[])[3]!;
  assert.equal(ceilingSeat.position, "-1,0.59,-2");
  assert.deepEqual(ceilingSeat.seats, ["-1,0.09,-2"]);
});

test("FIXED quarter-turn recomposition falls back for non-commuting display transforms", () => {
  const diagnostics = new DiagnosticBag();
  const converted = convertMechanics({ Mechanics: { furniture: {
    limited_placing: { floor: false, roof: true, wall: false },
    properties: { display_transform: "FIXED", translation: "1,0,0" },
    hitbox: {},
  } } }, "demo:x", "demo:item/x", diagnostics, "fixture.yml", "x");
  const ceiling = ((converted.furniture!.variants as JsonObject).ceiling as JsonObject);
  const element = (ceiling.elements as JsonObject[])[0]!;
  assert.equal(element.pitch, 90);
  assert.equal(element.yaw, -180);
  assert.equal(element.rotation, undefined);
});

test("furniture global default properties merge before item overrides", () => {
  const diagnostics = new DiagnosticBag();
  const config: JsonObject = { Mechanics: { furniture: {
    limited_placing: { floor: false, roof: false, wall: false },
    properties: { scale: "0.8,0.8,0.8" },
    hitbox: {},
  } } };
  const defaults: JsonObject = {
    display_transform: "FIXED", translation: "1,2,3", scale: "0.2,0.3,0.4", offset_against_blocks: false,
  };
  const converted = convertMechanics(config, "demo:x", "demo:item/x", diagnostics, "fixture.yml", "x", defaults);
  assert.deepEqual(Object.keys(converted.furniture!.variants as JsonObject), []);
  // Re-enable one canonical plane to inspect the merged element.
  ((config.Mechanics as JsonObject).furniture as JsonObject).limited_placing = { floor: true, roof: false, wall: false };
  const placed = convertMechanics(config, "demo:x", "demo:item/x", diagnostics, "fixture.yml", "x", defaults);
  const element = ((((placed.furniture!.variants as JsonObject).ground as JsonObject).elements as JsonObject[])[0])!;
  assert.equal(element.display_transform, "fixed");
  assert.equal(element.translation, "1,2,3");
  assert.equal(element.scale, "0.8,0.8,0.8");
  assert.equal(diagnostics.items.some((entry) => entry.code === "FURNITURE_PARTIAL_BLOCK_OFFSET_DYNAMIC"), false);
});

test("Nexo furniture lights become CE glowing variants with persistent right-click toggling", () => {
  const diagnostics = new DiagnosticBag();
  const config: JsonObject = { Mechanics: { furniture: {
    limited_placing: { floor: true, roof: false, wall: false },
    properties: { display_transform: "FIXED", offset_against_blocks: false },
    hitbox: { barriers: ["0,0,0"] },
    lights: {
      toggleable: true,
      lights: ["0,1,0 14", "1..2,0,-1 12", "0,0,0 15"],
    },
  } } };
  const converted = convertMechanics(config, "demo:lamp", "demo:item/lamp", diagnostics, "fixture.yml", "lamp");
  const furniture = converted.furniture!;
  const variants = furniture.variants as JsonObject;
  assert.equal(Object.keys(variants).length, 32);
  assert.ok(variants._nexo_ground_barrier_grid_8);
  assert.deepEqual(variants.ground_unlit, variants.ground);
  assert.deepEqual(variants._nexo_ground_barrier_grid_8_unlit, variants._nexo_ground_barrier_grid_8);
  const groundHitboxes = (variants.ground as JsonObject).hitboxes as JsonObject[];
  assert.equal(groundHitboxes.some((entry) => entry._nexo_barrier !== undefined), false);

  const behaviors = furniture.behaviors as JsonObject[];
  assert.equal(behaviors[0]!.type, "glowing_furniture");
  const lights = ((behaviors[0]!.variants as JsonObject).ground as JsonObject[]);
  assert.deepEqual(lights, [
    { position: "0,1,0", level: 14 },
    { position: "-1,0,1", level: 12 },
    { position: "-2,0,1", level: 12 },
  ]);
  assert.equal((behaviors[0]!.variants as JsonObject).ground_unlit, undefined);
  const events = furniture.events as JsonObject[];
  const functions = events.find((event) => event.on === "right_click")!.functions as JsonObject[];
  const cases = functions[0]!.cases as JsonObject[];
  assert.equal(cases.length, 32);
  assert.ok(cases.some((entry) => entry.when === "ground"));
  assert.ok(cases.some((entry) => entry.when === "ground_unlit"));
  assert.equal(functions[1]!.type, "update_interaction_tick");
  assert.equal((converted.semantics.furniture as JsonObject).lights, 3);
  assert.equal((converted.semantics.furniture as JsonObject).toggleable_light, true);
  assert.ok(diagnostics.items.some((entry) => entry.code === "NEXO_LIGHT_BARRIER_OVERLAP_IGNORED"));
});

test("furniture light positions include anchor offsets before grid and wall profile deltas", () => {
  const diagnostics = new DiagnosticBag();
  const fixed = convertMechanics({ Mechanics: { furniture: {
    limited_placing: { floor: true, roof: true, wall: true },
    properties: { display_transform: "FIXED" },
    hitbox: { barriers: ["0,0,0"] },
    lights: { lights: ["0,1,0 14"] },
  } } }, "demo:fixed_lamp", "demo:item/fixed_lamp", diagnostics, "fixture.yml", "fixed_lamp").furniture!;
  const fixedLights = (((fixed.behaviors as JsonObject[])[0]!.variants) as JsonObject);
  assert.deepEqual(fixedLights.ground, [{ position: "0,1,0", level: 14 }]);
  assert.deepEqual(fixedLights.ceiling, [{ position: "0,0.99,0", level: 14 }]);
  assert.deepEqual(fixedLights.wall, [{ position: "0,1,0.01", level: 14 }]);
  assert.deepEqual(fixedLights._nexo_ground_barrier_grid_8, [{ position: "0,1.5,0", level: 14 }]);
  assert.deepEqual(fixedLights._nexo_ceiling_barrier_grid_8, [{ position: "0,0.49,0", level: 14 }]);
  assert.deepEqual(fixedLights._nexo_wall_supported, [{ position: "0,1,0.5", level: 14 }]);
  const fixedCompacted = compactFurnitureDefinition(fixed, "demo:fixed_lamp");
  const fixedExpanded = expandCraftEngineTemplateEntry(
    fixedCompacted.definition, fixedCompacted.templates, "demo:fixed_lamp",
  );
  assert.deepEqual(fixedExpanded.behaviors, fixed.behaviors);

  const nonFixed = convertMechanics({ Mechanics: { furniture: {
    limited_placing: { floor: true, roof: false, wall: false },
    properties: { display_transform: "HEAD" },
    hitbox: { interactions: ["0,0,0 1,1"] },
    lights: { lights: ["0,1,0 10"] },
  } } }, "demo:head_lamp", "demo:item/head_lamp", diagnostics, "fixture.yml", "head_lamp").furniture!;
  const nonFixedLights = (((nonFixed.behaviors as JsonObject[])[0]!.variants) as JsonObject);
  assert.deepEqual(nonFixedLights.ground, [{ position: "0,1.5,0", level: 10 }]);
});

test("rotatable false is exact and scalar true uses native CE rotation", () => {
  const disabledDiagnostics = new DiagnosticBag();
  const disabled = convertMechanics(
    { Mechanics: { furniture: { rotatable: false } } },
    "demo:still", "demo:item/still", disabledDiagnostics, "fixture.yml", "still",
  );
  assert.equal(disabled.furniture!.events, undefined);
  assert.equal((disabled.semantics.furniture as JsonObject).rotatable, false);
  assert.equal(disabledDiagnostics.items.some((entry) => entry.field?.endsWith(".rotatable")), false);

  const diagnostics = new DiagnosticBag();
  const converted = convertMechanics(
    { Mechanics: { furniture: { rotatable: true, restricted_rotation: "NONE" } } },
    "demo:turning", "demo:item/turning", diagnostics, "fixture.yml", "turning", undefined,
    { defaultRotatableOnSneak: true, rotationGamemodes: ["SURVIVAL", "ADVENTURE"] },
  );
  const functions = ((converted.furniture!.events as JsonObject[])[0]!.functions as JsonObject[]);
  assert.deepEqual(functions.map((entry) => entry.type), ["update_interaction_tick", "rotate_furniture"]);
  assert.equal(functions[1]!.degree, 22.5);
  const conditions = functions[1]!.conditions as JsonObject[];
  assert.equal(conditions[0]!.type, "expression");
  assert.deepEqual(((conditions[1]!.terms as JsonObject[]).map((term) => term.value2)), ["SURVIVAL", "ADVENTURE"]);
  assert.deepEqual(functions[0]!.conditions, functions[1]!.conditions);
  assert.equal((converted.semantics.furniture as JsonObject).rotation_on_sneak, true);
});

test("nested rotatable, toggleable light, and seats preserve Nexo interaction order", () => {
  const diagnostics = new DiagnosticBag();
  const converted = convertMechanics({ Mechanics: { furniture: {
    rotatable: { rotatable: true, on_sneak: false },
    restricted_rotation: "VERY_STRICT",
    seats: ["0,0.6,0"],
    hitbox: { interactions: ["0,0,0 1,1"] },
    lights: { toggleable: true, lights: ["0,1,0 12"] },
  } } }, "demo:chair_lamp", "demo:item/chair_lamp", diagnostics, "fixture.yml", "chair_lamp", undefined,
  { defaultRotatableOnSneak: true, rotationGamemodes: [] });
  const functions = (((converted.furniture!.events as JsonObject[])[0]!.functions as JsonObject[]));
  assert.deepEqual(functions.map((entry) => entry.type), [
    "when", "update_interaction_tick", "update_interaction_tick", "rotate_furniture",
  ]);
  assert.equal(functions[1]!.conditions && (functions[1]!.conditions as JsonObject[])[0]!.type, "expression");
  assert.equal(((functions[2]!.conditions as JsonObject[])[0]!.type), "!expression");
  assert.equal((((functions[2]!.conditions as JsonObject[])[1]!.terms as JsonObject[])[0]!.value2), "__NEXO_NO_GAMEMODE__");
  assert.equal(functions[3]!.degree, 45);
  assert.equal(functions[3]!.on_failure, undefined);
});

test("wall Barriers stay block-centered while the FIXED model uses Nexo's wall offset", () => {
  const diagnostics = new DiagnosticBag();
  const converted = convertMechanics({ Mechanics: { furniture: {
    limited_placing: { floor: false, roof: false, wall: true },
    properties: { display_transform: "FIXED", scale: "0.5,0.5,0.5" },
    hitbox: { interactions: ["0,0,0 1,1"], barriers: ["0,0,0"] },
  } } }, "demo:wall", "demo:item/wall", diagnostics, "fixture.yml", "wall");
  const wall = (converted.furniture!.variants as JsonObject).wall as JsonObject;
  const element = (wall.elements as JsonObject[])[0]!;
  const hitboxes = wall.hitboxes as JsonObject[];
  const interaction = hitboxes.find((entry) => entry.type === "interaction")!;
  const barrier = hitboxes.find((entry) => entry.type === "shulker")!;
  assert.equal(element.position, "0,0,0.01");
  assert.equal(interaction.position, "0,-0.5,0.01");
  assert.equal(barrier.position, "0,-0.5,0.5");
  const supported = (converted.furniture!.variants as JsonObject)._nexo_wall_supported as JsonObject;
  assert.equal(((supported.elements as JsonObject[])[0]!.position), "0,0,0.5");
  const supportedHitboxes = supported.hitboxes as JsonObject[];
  assert.equal(supportedHitboxes.find((entry) => entry.type === "interaction")!.position, "0,-0.5,0.5");
  assert.equal(supportedHitboxes.find((entry) => entry.type === "shulker")!.position, "0,-0.5,0.5");
  const placeFunctions = (((converted.furniture!.events as JsonObject[])[0]!.functions) as JsonObject[]);
  assert.equal(placeFunctions.length, 4);
  assert.deepEqual(placeFunctions.map((entry) => ((entry.conditions as JsonObject[])[1]!.expression)), [
    "ABS(<arg:furniture.yaw>-(-90))<0.01",
    "ABS(<arg:furniture.yaw>-(90))<0.01",
    "ABS(<arg:furniture.yaw>-(0))<0.01",
    "ABS(<arg:furniture.yaw>-(180))<0.01",
  ]);
  const matchBlock = (placeFunctions[0]!.conditions as JsonObject[])[2]!;
  assert.equal(matchBlock.type, "match_block");
  assert.equal(matchBlock.regex, true);
  assert.ok(String((matchBlock.blocks as string[])[0]).includes("minecraft:"));
  assert.equal(diagnostics.items.some((entry) => entry.code === "FURNITURE_WALL_SUPPORT_OFFSET_DYNAMIC"), false);

  const noOffset = convertMechanics({ Mechanics: { furniture: {
    limited_placing: { floor: false, roof: false, wall: true },
    properties: { display_transform: "FIXED", scale: "0.5,0.5,0.5", offset_against_blocks: false },
    hitbox: { interactions: ["0,0,0 1,1"], barriers: ["0,0,0"] },
  } } }, "demo:centered-wall", "demo:item/centered-wall", diagnostics, "fixture.yml", "centered-wall");
  const noOffsetVariants = noOffset.furniture!.variants as JsonObject;
  const noOffsetWall = noOffsetVariants.wall as JsonObject;
  assert.equal((noOffsetWall.elements as JsonObject[])[0]!.position, "0,0,0.01");
  assert.equal((noOffsetWall.hitboxes as JsonObject[]).find((entry) => entry.type === "interaction")!.position, "0,-0.5,0.01");
  assert.ok(noOffsetVariants._nexo_wall_supported, "offset_against_blocks gates display translation correction, not Nexo's entity anchor");
});

test("ceiling Barriers use the target block bottom while displays keep Nexo's clearance", () => {
  const diagnostics = new DiagnosticBag();
  const converted = convertMechanics({ Mechanics: { furniture: {
    limited_placing: { floor: false, roof: true, wall: false },
    properties: { display_transform: "FIXED" },
    hitbox: { barriers: ["0,0,0"] },
  } } }, "demo:ceiling", "demo:item/ceiling", diagnostics, "fixture.yml", "ceiling");
  const variants = converted.furniture!.variants as JsonObject;
  const ceiling = variants.ceiling as JsonObject;
  assert.equal((ceiling.elements as JsonObject[])[0]!.position, "0,-0.01,0");
  assert.equal((ceiling.hitboxes as JsonObject[])[0]!.position, "0,-1,0");
  const halfHeightRay = variants._nexo_ceiling_barrier_grid_8 as JsonObject;
  assert.equal((halfHeightRay.elements as JsonObject[])[0]!.position, "0,-0.51,0");
  assert.equal((halfHeightRay.hitboxes as JsonObject[])[0]!.position, "0,-1.5,0");
});

test("Autumn signpost, haystack, and both streamers retain Nexo world-space semantics", () => {
  const diagnostics = new DiagnosticBag();
  const barrierSource = { Mechanics: { furniture: {
    rotatable: false,
    limited_placing: { roof: true, floor: true, wall: true },
    properties: { display_transform: "FIXED", scale: "0.5,0.5,0.5" },
    hitbox: { barriers: ["0,0,0"] },
  } } };
  const signpost = convertMechanics(barrierSource, "lanshan_autumn_field:field_signpost", "lanshan_autumn_field:item/field_signpost", diagnostics, "autumn.yml", "field_signpost").furniture!;
  const haystackSource = structuredClone(barrierSource);
  ((haystackSource.Mechanics as JsonObject).furniture as JsonObject).seats = ["0.0,1.0,0.0"];
  const haystack = convertMechanics(haystackSource, "lanshan_autumn_field:field_haystack", "lanshan_autumn_field:item/field_haystack", diagnostics, "autumn.yml", "field_haystack").furniture!;
  for (const converted of [signpost, haystack]) {
    const variants = converted.variants as JsonObject;
    const groundElement = (((variants.ground as JsonObject).elements as JsonObject[])[0])!;
    const ceilingElement = (((variants.ceiling as JsonObject).elements as JsonObject[])[0])!;
    assert.deepEqual(
      { position: groundElement.position, pitch: groundElement.pitch, yaw: groundElement.yaw, rotation: groundElement.rotation },
      { position: undefined, pitch: 90, yaw: undefined, rotation: "0,1,0,0" },
    );
    assert.equal((((variants.ground as JsonObject).hitboxes as JsonObject[])[0]!.position), "0,0,0");
    assert.deepEqual(
      { position: ceilingElement.position, pitch: ceilingElement.pitch, yaw: ceilingElement.yaw, rotation: ceilingElement.rotation },
      { position: "0,-0.01,0", pitch: -90, yaw: undefined, rotation: "0,1,0,0" },
    );
    assert.equal((((variants.ceiling as JsonObject).hitboxes as JsonObject[])[0]!.position), "0,-1,0");
    assert.equal((((variants.wall as JsonObject).elements as JsonObject[])[0]!.position), "0,0,0.01");
    assert.equal((((variants.wall as JsonObject).hitboxes as JsonObject[])[0]!.position), "0,-0.5,0.5");
  }

  const streamerSource = { Mechanics: { furniture: {
    rotatable: false,
    limited_placing: { roof: false, floor: false, wall: true },
    properties: { display_transform: "FIXED", scale: "0.5,0.5,0.5" },
    hitbox: { interactions: ["0,0,0 1.0,1.0"] },
  } } };
  const large = convertMechanics(streamerSource, "lanshan_autumn_field:large_crop_streamer", "lanshan_autumn_field:item/large_crop_streamer", diagnostics, "autumn.yml", "large_crop_streamer").furniture!;
  const small = convertMechanics(streamerSource, "lanshan_autumn_field:small_crop_streamer", "lanshan_autumn_field:item/small_crop_streamer", diagnostics, "autumn.yml", "small_crop_streamer").furniture!;
  const wall = (large.variants as JsonObject).wall as JsonObject;
  assert.equal((wall.elements as JsonObject[])[0]!.position, "0,0,0.01");
  assert.equal((wall.hitboxes as JsonObject[])[0]!.position, "0,-0.5,0.01");
  const compactLarge = compactFurnitureDefinition(large, "lanshan_autumn_field:large_crop_streamer");
  const compactSmall = compactFurnitureDefinition(small, "lanshan_autumn_field:small_crop_streamer");
  assert.equal(compactLarge.definition.template, compactSmall.definition.template);
  const templates: JsonObject = {};
  mergeFurnitureTemplates(templates, compactLarge.templates);
  mergeFurnitureTemplates(templates, compactSmall.templates);
  const expandedLarge = expandCraftEngineTemplateEntry(compactLarge.definition, templates, "lanshan_autumn_field:large_crop_streamer");
  const expandedSmall = expandCraftEngineTemplateEntry(compactSmall.definition, templates, "lanshan_autumn_field:small_crop_streamer");
  assert.equal((expandedLarge.settings as JsonObject).item, "lanshan_autumn_field:large_crop_streamer");
  assert.equal((expandedSmall.settings as JsonObject).item, "lanshan_autumn_field:small_crop_streamer");
});

test("Autumn ceiling lantern keeps Nexo's separate visual, Barrier, and light anchors", () => {
  const diagnostics = new DiagnosticBag();
  const converted = convertMechanics({ Mechanics: { furniture: {
    rotatable: false,
    limited_placing: { roof: true, floor: false, wall: false },
    properties: { display_transform: "FIXED", scale: "0.5,0.5,0.5" },
    hitbox: { barriers: ["0,0,0"] },
    lights: { toggleable: true, lights: ["0,-1,0 14"] },
  } } }, "lanshan_autumn_field:field_lantern_ceiling", "lanshan_autumn_field:item/field_lantern_ceiling", diagnostics, "autumn.yml", "field_lantern_ceiling");
  const variants = converted.furniture!.variants as JsonObject;
  const ceiling = variants.ceiling as JsonObject;
  const element = (ceiling.elements as JsonObject[])[0]!;
  const barrier = (ceiling.hitboxes as JsonObject[])[0]!;
  assert.deepEqual(
    { position: element.position, pitch: element.pitch, yaw: element.yaw, rotation: element.rotation, displayTransform: element.display_transform },
    { position: "0,-0.01,0", pitch: -90, yaw: undefined, rotation: "0,1,0,0", displayTransform: "fixed" },
  );
  assert.deepEqual(
    { type: barrier.type, position: barrier.position, scale: barrier.scale, peek: barrier.peek },
    { type: "shulker", position: "0,-1,0", scale: 1, peek: 0 },
  );
  const glowing = (converted.furniture!.behaviors as JsonObject[]).find((behavior) => behavior.type === "glowing_furniture")!;
  assert.deepEqual((glowing.variants as JsonObject).ceiling, [{ position: "0,-1.01,0", level: 14 }]);
  const compacted = compactFurnitureDefinition(converted.furniture!, "lanshan_autumn_field:field_lantern_ceiling");
  const expanded = expandCraftEngineTemplateEntry(compacted.definition, compacted.templates, "lanshan_autumn_field:field_lantern_ceiling");
  const expandedElement = ((((expanded.variants as JsonObject).ceiling as JsonObject).elements as JsonObject[])[0])!;
  assert.deepEqual(
    { pitch: expandedElement.pitch, yaw: expandedElement.yaw, rotation: expandedElement.rotation },
    { pitch: -90, yaw: undefined, rotation: "0,1,0,0" },
  );
  assert.equal(diagnostics.items.length, 0);
});

test("partial-height placement selects native Barrier grid variants by ray-hit Y", () => {
  const diagnostics = new DiagnosticBag();
  const converted = convertMechanics({ Mechanics: { furniture: {
    limited_placing: { floor: true, roof: false, wall: false },
    properties: { display_transform: "FIXED", offset_against_blocks: false },
    hitbox: { barriers: ["0,0,0"] },
    seats: ["0,0.6,0"],
  } } }, "demo:grid", "demo:item/grid", diagnostics, "fixture.yml", "grid");
  const variants = converted.furniture!.variants as JsonObject;
  const profile = variants._nexo_ground_barrier_grid_8 as JsonObject;
  assert.ok(profile);
  assert.equal(((profile.hitboxes as JsonObject[])[0]!.position), "0,0.5,0");
  assert.equal(((profile.elements as JsonObject[])[0]!.position), "0,0.5,0");
  assert.deepEqual((profile.hitboxes as JsonObject[]).flatMap((hitbox) => hitbox.seats ?? []), ["0,0.6,0"]);
  const allSeatPositions: string[] = [];
  for (const variant of Object.values(variants)) {
    if (!isObject(variant)) continue;
    const seats = (variant.hitboxes as JsonObject[]).flatMap((hitbox) => Array.isArray(hitbox.seats) ? hitbox.seats as string[] : []);
    const positions = seats.map((seat) => seat.trim().split(/\s+/u)[0]!);
    assert.equal(new Set(positions).size, positions.length, "seat positions must be unique inside each active variant");
    allSeatPositions.push(...positions);
  }
  assert.equal(new Set(allSeatPositions).size, allSeatPositions.length, "generated voxel profiles must not repeat literal seat coordinates");
  const vectorY = (value: unknown): number => Number(String(value ?? "0,0,0").split(",")[1]);
  for (let sixteenth = 1; sixteenth < 16; sixteenth++) {
    const shifted = variants["_nexo_ground_barrier_grid_" + sixteenth] as JsonObject;
    const anchorY = sixteenth / 16;
    const elementY = vectorY((shifted.elements as JsonObject[])[0]!.position);
    const barrierY = vectorY((shifted.hitboxes as JsonObject[])[0]!.position);
    const seat = (shifted.hitboxes as JsonObject[]).flatMap((hitbox) => hitbox.seats ?? [])[0];
    assert.ok(Math.abs(anchorY + elementY - 1) < 1e-8);
    assert.ok(Math.abs(anchorY + barrierY - 1) < 1e-8);
    assert.ok(Math.abs(anchorY + vectorY(String(seat).split(/\s+/u)[0]) - 1.1) < 1e-8);
  }
  const events = converted.furniture!.events as JsonObject[];
  assert.equal(events[0]!.on, "place");
  const functions = events[0]!.functions as JsonObject[];
  assert.equal(functions.length, 15);
  assert.equal(functions[7]!.variant, "_nexo_ground_barrier_grid_8");
  assert.equal((((functions[7]!.conditions as JsonObject[])[1]!.expression)),
    "ABS((<arg:position.y>-FLOOR(<arg:position.y>))-0.5)<0.00001");
  assert.equal(diagnostics.items.some((entry) => entry.code.includes("PARTIAL_BLOCK")), false);
});

test("native CE templates compact furniture families and expand to shifted geometry, seats, lights, and events", () => {
  const source: JsonObject = { Mechanics: { furniture: {
    limited_placing: { floor: true, roof: false, wall: false },
    properties: { display_transform: "FIXED" },
    hitbox: { barriers: ["0,0,0"] },
    seats: ["0,0.6,0"],
    lights: { toggleable: true, lights: ["0,1,0 14"] },
  } } };
  const diagnostics = new DiagnosticBag();
  const first = convertMechanics(source, "demo:lamp_a", "demo:item/lamp_a", diagnostics, "fixture.yml", "lamp_a").furniture!;
  const second = convertMechanics(source, "demo:lamp_b", "demo:item/lamp_b", diagnostics, "fixture.yml", "lamp_b").furniture!;
  const compactA = compactFurnitureDefinition(first, "demo:lamp_a");
  const compactB = compactFurnitureDefinition(second, "demo:lamp_b");
  assert.deepEqual(Object.keys(compactA.definition), ["template"]);
  assert.equal(compactA.definition.template, compactB.definition.template, "identical families must share one target-neutral template");
  const mergedTemplates: JsonObject = {};
  mergeFurnitureTemplates(mergedTemplates, compactA.templates);
  const beforeMerge = Object.keys(mergedTemplates).length;
  mergeFurnitureTemplates(mergedTemplates, compactB.templates);
  assert.equal(Object.keys(mergedTemplates).length, beforeMerge, "identical geometry must not duplicate generated templates");
  assert.ok(beforeMerge < 20, "the fixed 15-profile boilerplate should be interned instead of copied per furniture");
  const concreteBytes = Buffer.byteLength(stringify({ furniture: { "demo:lamp_a": first, "demo:lamp_b": second } }));
  const compactBytes = Buffer.byteLength(stringify({
    templates: mergedTemplates,
    furniture: { "demo:lamp_a": compactA.definition, "demo:lamp_b": compactB.definition },
  }));
  assert.ok(compactBytes < concreteBytes * 0.45, "native templates should remove most repeated serialized YAML");

  const expanded = expandCraftEngineTemplateEntry(compactA.definition, mergedTemplates, "demo:lamp_a");
  assert.equal(((expanded.settings as JsonObject).item), "demo:lamp_a");
  assert.equal((((((expanded.loot as JsonObject).pools as JsonObject[])[0]!.entries as JsonObject[])[0]!.item)), "demo:lamp_a");
  const expandedB = expandCraftEngineTemplateEntry(compactB.definition, mergedTemplates, "demo:lamp_b");
  assert.equal(((expandedB.settings as JsonObject).item), "demo:lamp_b");
  assert.equal((((((expandedB.loot as JsonObject).pools as JsonObject[])[0]!.entries as JsonObject[])[0]!.item)), "demo:lamp_b");
  const concreteVariants = first.variants as JsonObject;
  const expandedVariants = expanded.variants as JsonObject;
  assert.equal(Object.keys(expandedVariants).length, 32);
  assert.deepEqual(expandedVariants._nexo_ground_barrier_grid_8, concreteVariants._nexo_ground_barrier_grid_8);
  assert.deepEqual(expandedVariants._nexo_ground_barrier_grid_8_unlit, concreteVariants._nexo_ground_barrier_grid_8_unlit);

  const concreteBehavior = (first.behaviors as JsonObject[])[0]!;
  const expandedBehavior = (expanded.behaviors as JsonObject[])[0]!;
  const concreteLights = concreteBehavior.variants as JsonObject;
  const expandedLights = expandedBehavior.variants as JsonObject;
  assert.deepEqual(expandedLights._nexo_ground_barrier_grid_8, concreteLights._nexo_ground_barrier_grid_8);
  assert.deepEqual(expandedLights._nexo_ground_barrier_grid_8, [{ position: "0,1.5,0", level: 14 }]);
  assert.equal(expandedLights._nexo_ground_barrier_grid_8_unlit, undefined);

  const events = expanded.events as JsonObject[];
  const placeFunctions = events.find((event) => event.on === "place")!.functions as JsonObject[];
  assert.equal(placeFunctions.length, 15);
  assert.equal(placeFunctions[7]!.variant, "_nexo_ground_barrier_grid_8");
  const clickFunctions = events.find((event) => event.on === "right_click")!.functions as JsonObject[];
  const cases = clickFunctions[0]!.cases as JsonObject[];
  assert.equal(cases.length, 32);
  assert.equal(clickFunctions[1]!.type, "update_interaction_tick");
  assert.equal(diagnostics.items.length, 0);
});

test("Pack.generate_model is matched silently because Nexo does not parse it as the decision key", () => {
  const diagnostics = new DiagnosticBag();
  const model = readPackModel({ generate_model: false, model: "demo:item/existing" }, "demo", context(diagnostics));
  assert.equal(model.base?.path, "demo:item/existing");
  assert.equal(diagnostics.items.some((entry) => entry.code === "NEXO_GENERATE_MODEL_IGNORED"), false);
});

test("shulker normalized length uses CE sine-squared inverse, not linear interpolation", () => {
  const diagnostics = new DiagnosticBag();
  const config: JsonObject = { Mechanics: { furniture: { hitbox: { shulkers: ["0,0,0 2 1.25 DOWN false"] } } } };
  const converted = convertMechanics(config, "demo:x", "demo:item/x", diagnostics, "fixture.yml", "x");
  const ground = ((converted.furniture!.variants as JsonObject).ground as JsonObject);
  const hitbox = (ground.hitboxes as JsonObject[])[0]!;
  assert.equal(hitbox.scale, 2);
  assert.equal(hitbox.peek, 33);
  assert.equal(hitbox.direction, "down");
});

test("barrier mapping is a silently supported exact native hard AABB", () => {
  const diagnostics = new DiagnosticBag();
  const config: JsonObject = { Mechanics: { furniture: { hitbox: { barriers: ["0,0,0"] } } } };
  const converted = convertMechanics(config, "demo:x", "demo:item/x", diagnostics, "fixture.yml", "x");
  const ground = (converted.furniture!.variants as JsonObject).ground as JsonObject;
  const hitbox = (ground.hitboxes as JsonObject[])[0]!;
  assert.deepEqual(
    { type: hitbox.type, scale: hitbox.scale, peek: hitbox.peek, direction: hitbox.direction, blocksBuilding: hitbox.blocks_building, projectiles: hitbox.can_be_hit_by_projectile },
    { type: "shulker", scale: 1, peek: 0, direction: "up", blocksBuilding: true, projectiles: true },
  );
  assert.equal(diagnostics.items.some((entry) => entry.code.includes("BARRIER") && entry.field?.includes("hitbox.barriers")), false);
});

test("oversized barrier ranges fail safely without eager Cartesian expansion", () => {
  const diagnostics = new DiagnosticBag();
  const converted = convertMechanics({ Mechanics: { furniture: {
    hitbox: { barriers: ["0..1000000,0..1000000,0..1000000"] },
  } } }, "demo:unsafe", "demo:item/unsafe", diagnostics, "fixture.yml", "unsafe");
  const ground = (converted.furniture!.variants as JsonObject).ground as JsonObject;
  assert.equal((ground.hitboxes as JsonObject[]).length, 0);
  assert.ok(diagnostics.items.some((entry) => entry.code === "BARRIER_RANGE_TOO_LARGE" && entry.severity === "error"));
});

test("custom blocks require a model and never add wrong self loot", () => {
  const missingDiagnostics = new DiagnosticBag();
  const missing = convertMechanics({ Mechanics: { noteblock: {} } }, "demo:block", undefined, missingDiagnostics, "fixture.yml", "block");
  assert.equal(missing.block, undefined);
  assert.deepEqual(missing.behavior, []);
  assert.ok(missingDiagnostics.items.some((entry) => entry.code === "BLOCK_MODEL_MISSING"));

  const customDiagnostics = new DiagnosticBag();
  const custom = convertMechanics({ Mechanics: { noteblock: {
    drop: { loots: [{ minecraft_type: "DIAMOND", probability: 1, amount: 1 }] },
  } } }, "demo:block", "demo:block/model", customDiagnostics, "fixture.yml", "block");
  assert.equal(custom.block?.loot, undefined);
  assert.ok(customDiagnostics.items.some((entry) => entry.code === "CUSTOM_BLOCK_DROP_MANUAL"));

  const self = convertMechanics({ Mechanics: { noteblock: {} } }, "demo:block", "demo:block/model", new DiagnosticBag(), "fixture.yml", "block");
  assert.deepEqual(self.block?.loot, {
    pools: [{ rolls: 1, conditions: [{ type: "survives_explosion" }], entries: [{ type: "item", item: "demo:block" }] }],
  });
});

test("Nexo recipe fields map to CraftEngine recipe semantics", () => {
  const diagnostics = new DiagnosticBag();
  const shaped = convertRecipe("shaped", "chair", {
    result: { nexo_item: "chair", amount: 2 },
    shape: ["XX", " X"],
    ingredients: { X: { minecraft_type: "STICK" } },
  }, "demo", diagnostics, "recipe.yml")!;
  assert.equal(shaped.type, "shaped");
  assert.deepEqual(shaped.result, { id: "demo:chair", count: 2 });
  assert.equal((shaped.ingredients as JsonObject).X, "minecraft:stick");
  const cooking = convertRecipe("furnace", "glass", {
    result: { minecraft_type: "GLASS" }, input: { minecraft_type: "SAND" }, cookingTime: 200, experience: 0.1,
  }, "demo", diagnostics, "recipe.yml")!;
  assert.equal(cooking.type, "smelting");
  assert.equal(cooking.time, 200);
  assert.equal(cooking.experience, 0.1);
  const missing = convertRecipe("shaped", "broken", {
    result: { nexo_item: "chair" }, shape: ["XY"], ingredients: { X: { minecraft_type: "STICK" } },
  }, "demo", diagnostics, "recipe.yml");
  assert.equal(missing, undefined);
  assert.ok(diagnostics.items.some((entry) => entry.code === "SHAPED_INGREDIENT_MISSING"));
});

test("Nexo sound entries become CE sound event maps without .ogg suffix", () => {
  const diagnostics = new DiagnosticBag();
  const sounds = convertSounds({ sounds: [{ id: "demo:music.test", sound: "demo:music/test.ogg", stream: true }] }, diagnostics, "sounds.yml");
  const event = sounds["demo:music.test"] as JsonObject;
  const file = (event.sounds as JsonObject[])[0]!;
  assert.equal(file.name, "demo:music/test");
  assert.equal(file.stream, true);
  assert.equal(file.attenuation_distance, 16);
});

test("author namespaces are inferred from original bundle declarations and Nexo filenames", () => {
  const chinese = inferAuthorNamespaceFromBundlePaths([
    "Nexo/items/lanshan/lanshan_chinese_2.yml",
    "ItemsAdder/contents/lanshan_chinese_2/configs/categories.yml",
    "ItemsAdder/contents/lanshan_chinese_2/resourcepack/assets/lanshan_chinese_2/models/item/demo.json",
  ], "Nexo");
  assert.equal(chinese?.namespace, "lanshan_chinese_2");

  const autumn = inferAuthorNamespaceFromBundlePaths([
    "wrapper/Nexo/items/lanshan/lanshan_autumn_field.yml",
    "wrapper/ItemsAdder/contents/lanshan_autumn_field/configs/1.yml",
  ], "wrapper/Nexo");
  assert.equal(autumn?.namespace, "lanshan_autumn_field");

  const balloon = inferAuthorNamespaceFromBundlePaths([
    "Nexo/item/lanshan/lanshan_happy_ghast_hot_air_balloon_sprite.yml",
    "Nexo/item/lanshan/lanshan_hot_air_balloon.yml",
    "Nexo/item/lanshan/lanshan_hot_air_balloon_sprite.yml",
    "ItemsAdder/contents/lanshan_hot_air_balloon/configs/1.yml",
    "MythicMobs/packs/lanshan_hot_air_balloon/packinfo.yml",
  ], "Nexo");
  assert.equal(balloon?.namespace, "lanshan_hot_air_balloon");

  const nexoOnly = inferAuthorNamespaceFromBundlePaths([
    "Nexo/item/lanshan/lanshan_happy_ghast_hot_air_balloon_sprite.yml",
    "Nexo/item/lanshan/lanshan_hot_air_balloon.yml",
    "Nexo/item/lanshan/lanshan_hot_air_balloon_sprite.yml",
  ], "Nexo");
  assert.equal(nexoOnly?.namespace, "lanshan_hot_air_balloon");
});

test("missing static model typo redirects to one existing near-match without creating assets", async () => {
  const temp = await mkdtemp(join(tmpdir(), "nexo2ce-model-alias-"));
  const input = join(temp, "Nexo");
  const output = join(temp, "CraftEnginePack");
  try {
    await mkdir(join(input, "items"), { recursive: true });
    await mkdir(join(input, "pack", "assets", "minecraft", "models", "demo"), { recursive: true });
    await mkdir(join(input, "pack", "assets", "minecraft", "textures", "demo"), { recursive: true });
    await writeFile(join(input, "items", "demo.yml"), [
      "red_balloon:",
      "  material: PAPER",
      "  Pack:",
      "    generate_model: false",
      "    model: demo/red_balloon_sprite",
      "    custom_model_data: 1234",
      "",
    ].join("\n"), "utf8");
    await writeFile(join(input, "pack", "assets", "minecraft", "models", "demo", "red_balloon_spirit.json"), JSON.stringify({ parent: "minecraft:item/generated", textures: { layer0: "demo/red_balloon" } }), "utf8");
    await writeFile(join(input, "pack", "assets", "minecraft", "textures", "demo", "red_balloon.png"), "fixture", "utf8");
    const result = await convert({ input, output, clientMode: "hybrid", cmdPolicy: "preserve", strict: true, force: false, audit: true });
    assert.equal(result.success, true, result.diagnostics.formatLines().join("\n"));
    assert.equal(result.namespace, "demo");
    assert.equal(result.namespaceMode, "author");
    assert.equal(result.audit?.missingModels, 0);
    assert.ok(result.diagnostics.items.some((entry) => entry.code === "MODEL_REFERENCE_TYPO_RECOVERED" && !entry.lossy));
    const yaml = parse(await readFile(join(output, "configuration", "items.yml"), "utf8")) as JsonObject;
    assert.equal((((yaml.items as JsonObject)["demo:red_balloon"] as JsonObject).model as JsonObject).path, "minecraft:demo/red_balloon_spirit");
    assert.ok(await readFile(join(output, "resourcepack", "assets", "minecraft", "models", "demo", "red_balloon_spirit.json"), "utf8"));
    await assert.rejects(readFile(join(output, "resourcepack", "assets", "minecraft", "models", "demo", "red_balloon_sprite.json"), "utf8"));
  } finally { await rm(temp, { recursive: true, force: true }); }
});

test("ambiguous near-match models are never guessed", async () => {
  const temp = await mkdtemp(join(tmpdir(), "nexo2ce-model-ambiguous-"));
  try {
    const root = join(temp, "pack");
    const models = join(root, "assets", "minecraft", "models", "demo");
    await mkdir(models, { recursive: true });
    await writeFile(join(models, "red_balloon_spritz.json"), "{}", "utf8");
    await writeFile(join(models, "red_balloon_sprita.json"), "{}", "utf8");
    const diagnostics = new DiagnosticBag();
    const aliases = await discoverModelAliases(root, [{
      id: "red_balloon", source: "items.yml", template: false, templateIds: [],
      config: { Pack: { model: "demo/red_balloon_sprite" } },
    }], diagnostics);
    assert.equal(aliases.size, 0);
    assert.equal(diagnostics.items.length, 0);
  } finally { await rm(temp, { recursive: true, force: true }); }
});

test("end-to-end globals drive rotation and converted NoteBlock support profiles", async () => {
  const temp = await mkdtemp(join(tmpdir(), "nexo2ce-globals-"));
  const input = join(temp, "Nexo");
  const output = join(temp, "CraftEnginePack");
  try {
    await mkdir(join(input, "items"), { recursive: true });
    await mkdir(join(input, "pack", "assets"), { recursive: true });
    await writeFile(join(input, "mechanics.yml"), "furniture:\n  default_rotatable_on_sneak: true\n", "utf8");
    await writeFile(join(input, "settings.yml"), "Furniture:\n  allowed_gamemodes_for_rotation:\n    - ADVENTURE\n", "utf8");
    await writeFile(join(input, "items", "demo.yml"), [
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
      "",
    ].join("\n"), "utf8");
    const result = await convert({ input, output, namespace: "demo", clientMode: "modern", cmdPolicy: "preserve", strict: true, force: false, audit: false });
    assert.equal(result.success, true, result.diagnostics.formatLines().join("\n"));
    const yaml = parse(await readFile(join(output, "configuration", "furniture.yml"), "utf8")) as JsonObject;
    const templateYaml = parse(await readFile(join(output, "configuration", "furniture-templates.yml"), "utf8")) as JsonObject;
    const furnitureEntry = (yaml.furniture as JsonObject)["demo:turning"] as JsonObject;
    assert.deepEqual(Object.keys(furnitureEntry), ["template"]);
    const furniture = expandCraftEngineTemplateEntry(
      furnitureEntry, templateYaml.templates as JsonObject, "demo:turning",
    );
    const events = furniture.events as JsonObject[];
    const place = events.find((entry) => entry.on === "place")!;
    const matchBlock = ((((place.functions as JsonObject[])[0]!.conditions as JsonObject[])[2]))!;
    assert.ok((matchBlock.blocks as string[]).includes("demo:support"));
    const click = events.find((entry) => entry.on === "right_click")!;
    const rotate = (click.functions as JsonObject[]).find((entry) => entry.type === "rotate_furniture")!;
    const conditions = rotate.conditions as JsonObject[];
    assert.equal(conditions[0]!.type, "expression");
    assert.deepEqual(((conditions[1]!.terms as JsonObject[]).map((term) => term.value2)), ["ADVENTURE"]);
  } finally { await rm(temp, { recursive: true, force: true }); }
});

test("end-to-end conversion copies resources and passes model/texture graph audit", async () => {
  const temp = await mkdtemp(join(tmpdir(), "nexo2ce-e2e-"));
  const input = join(temp, "Nexo");
  const output = join(temp, "CraftEnginePack");
  try {
    await mkdir(join(input, "items"), { recursive: true });
    await mkdir(join(input, "pack", "assets", "minecraft", "models", "custom"), { recursive: true });
    await mkdir(join(input, "pack", "assets", "minecraft", "textures", "custom"), { recursive: true });
    await writeFile(join(input, "items", "demo.yml"), [
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
    ].join("\n"), "utf8");
    await writeFile(join(input, "pack", "assets", "minecraft", "models", "custom", "demo.json"), JSON.stringify({ parent: "minecraft:item/generated", textures: { layer0: "custom/demo" } }), "utf8");
    await writeFile(join(input, "pack", "assets", "minecraft", "textures", "custom", "demo.png"), "fixture", "utf8");
    const result = await convert({ input, output, namespace: "demo", clientMode: "hybrid", cmdPolicy: "preserve", strict: true, force: false, audit: true });
    assert.equal(result.success, true, result.diagnostics.formatLines().join("\n"));
    assert.equal(result.itemCount, 1);
    assert.equal(result.furnitureCount, 1);
    assert.equal(result.audit?.missingModels, 0);
    assert.equal(result.audit?.missingTextures, 0);
    const yaml = parse(await readFile(join(output, "configuration", "items.yml"), "utf8")) as JsonObject;
    const item = (yaml.items as JsonObject)["demo:demo"] as JsonObject;
    assert.equal(item.item_model, "demo:demo");
    assert.equal(item.custom_model_data, 1234);
    assert.ok(await readFile(join(output, "resourcepack", "assets", "minecraft", "models", "custom", "demo.json")));
    for (const absent of ["blocks.yml", "recipes.yml", "sounds.yml", "images.yml"]) {
      await assert.rejects(readFile(join(output, "configuration", absent), "utf8"));
    }
  } finally { await rm(temp, { recursive: true, force: true }); }
});

test("bbmodel assets relocate to CE blueprint paths and pass graph audit", async () => {
  const temp = await mkdtemp(join(tmpdir(), "nexo2ce-bbmodel-"));
  const input = join(temp, "Nexo");
  const output = join(temp, "CraftEnginePack");
  try {
    await mkdir(join(input, "items"), { recursive: true });
    await mkdir(join(input, "pack", "assets", "demo", "models", "item"), { recursive: true });
    await writeFile(join(input, "items", "demo.yml"), [
      "chair:",
      "  material: PAPER",
      "  Pack:",
      "    bbmodel: demo:item/chair",
      "",
    ].join("\n"), "utf8");
    await writeFile(join(input, "pack", "assets", "demo", "models", "item", "chair.bbmodel"), JSON.stringify({
      meta: { format_version: "4.10", model_format: "free" },
      name: "chair", resolution: { width: 16, height: 16 }, elements: [], outliner: [], textures: [],
    }), "utf8");

    const result = await convert({ input, output, namespace: "demo", clientMode: "hybrid", cmdPolicy: "preserve", strict: false, force: false, audit: true });
    assert.equal(result.success, true, result.diagnostics.formatLines().join("\n"));
    assert.equal(result.audit?.referencedBlueprints, 1);
    assert.equal(result.audit?.missingBlueprints, 0);
    assert.ok(result.diagnostics.items.some((entry) => entry.code === "BBMODEL_CONVERTER_REVIEW"));
    assert.ok(await readFile(join(output, "blueprint", "demo", "item", "chair.bbmodel"), "utf8"));
    await assert.rejects(readFile(join(output, "resourcepack", "assets", "demo", "models", "item", "chair.bbmodel"), "utf8"));
    await assert.rejects(readFile(join(output, "configuration", "furniture.yml"), "utf8"));
    await assert.rejects(readFile(join(output, "configuration", "furniture-templates.yml"), "utf8"));

    const yaml = parse(await readFile(join(output, "configuration", "items.yml"), "utf8")) as JsonObject;
    const item = (yaml.items as JsonObject)["demo:chair"] as JsonObject;
    assert.match(JSON.stringify(item.model), /"path":"demo:item\/chair"/);
    assert.match(JSON.stringify(item.model), /"blueprint":"demo\/item\/chair"/);
  } finally { await rm(temp, { recursive: true, force: true }); }
});

test("modern model tints and player-head special rendering match Nexo and Minecraft 1.21.11", () => {
  const diagnostics = new DiagnosticBag();
  const horse = readPackModel({ model: "demo:item/horse" }, "horse", context(diagnostics));
  const inherited = convertModels(horse, undefined, "leather_horse_armor", undefined, "modern", context(diagnostics)).model as JsonObject;
  assert.deepEqual(inherited.tints, [{ type: "dye", default: -6265536 }]);
  const colored = convertModels(horse, undefined, "leather_horse_armor", "255,0,0", "modern", context(diagnostics)).model as JsonObject;
  assert.deepEqual(colored.tints, [{ type: "dye", default: 0xff0000 }]);
  const head = convertModels(horse, undefined, "player_head", undefined, "modern", context(diagnostics)).model as JsonObject;
  assert.deepEqual(head, { type: "special", base: "demo:item/horse", model: { type: "player_head" } });
});

test("Nexo attribute and PDC schemas become loadable CraftEngine processors", () => {
  const diagnostics = new DiagnosticBag();
  const source: ResolvedItem = {
    id: "blade", source: "items.yml", template: false, templateIds: [],
    config: {
      material: "IRON_SWORD",
      AttributeModifiers: [{ attribute: "GENERIC_ATTACK_DAMAGE", amount: 3, operation: "ADD_SCALAR", slot: "MAINHAND" }],
      PersistentData: [{ key: "demo:value", type: "INTEGER", value: 7 }],
    },
  };
  const result = convertItem(source, { namespace: "demo", clientMode: "modern" }, undefined, diagnostics)!;
  const data = result.config.data as JsonObject;
  assert.deepEqual(data.attribute_modifiers, [{
    type: "minecraft:attack_damage", slot: "mainhand", id: "nexo:blade_attack_damage", amount: 3, operation: "add_multiplied_base",
  }]);
  assert.deepEqual(data.pdc, { "demo:value": 7 });
});

test("glyph grids count supplementary code points and allocate per font", async () => {
  const root = await mkdtemp(join(tmpdir(), "nexo2ce-glyph-unicode-"));
  try {
    await mkdir(join(root, "glyphs"), { recursive: true });
    await writeFile(join(root, "glyphs", "unicode.yml"), [
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
    ].join("\n"), "utf8");
    const diagnostics = new DiagnosticBag();
    const glyphs = await convertGlyphs(root, "demo", diagnostics, "nexo:default", "");
    assert.deepEqual((glyphs.images["demo:astral"] as JsonObject).chars, ["😀😁"]);
    assert.equal(glyphs.entries.astral!.columns, 2);
    assert.equal(rewriteGlyphTags("<glyph:astral_second>", glyphs.entries, diagnostics, "item.yml", "demo"), "<white><image:demo:astral:0:1></white>");
    assert.equal(glyphs.entries.auto_a!.chars[0], "\uA411");
    assert.equal(glyphs.entries.auto_c!.chars[0], "\uA410");
    assert.equal(diagnostics.items.some((entry) => entry.code === "GLYPH_CHAR_CONFLICT"), false);
    assert.equal(diagnostics.items.some((entry) => entry.code === "GLYPH_SUPPLEMENTARY_CHAR_REVIEW"), false);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("glyph allocator preserves explicit codepoints and rewrites Nexo tags to CE images", async () => {
  const root = await mkdtemp(join(tmpdir(), "nexo2ce-glyph-"));
  try {
    await mkdir(join(root, "glyphs"), { recursive: true });
    await writeFile(join(root, "glyphs", "a.yml"), "reserved:\n  texture: demo:font/reserved\n  char: \"\\uA410\"\nslice:\n  reference: auto\n  index: 1..2\n", "utf8");
    await writeFile(join(root, "glyphs", "b.yml"), "auto:\n  texture: demo:font/auto\n  rows: 1\n  columns: 2\n", "utf8");
    const diagnostics = new DiagnosticBag();
    const glyphs = await convertGlyphs(root, "demo", diagnostics, "nexo:default", "");
    const auto = glyphs.entries.auto!;
    assert.equal(auto.chars[0], "\uA411\uA412");
    assert.equal((glyphs.images["demo:auto"] as JsonObject).font, "nexo:default");
    assert.equal(rewriteGlyphTags("x<glyph:auto>y", glyphs.entries, diagnostics, "item.yml", "demo"), "x<white><image:demo:auto:0:0></white><shift:-1><white><image:demo:auto:0:1></white>y");
    assert.equal(rewriteGlyphTags("<g:auto:2:colorable>", glyphs.entries, diagnostics, "item.yml", "demo"), "<image:demo:auto:0:1>");
    assert.equal(rewriteGlyphTags("<glyph:slice>", glyphs.entries, diagnostics, "item.yml", "demo"), "<white><image:demo:auto:0:0></white><shift:-1><white><image:demo:auto:0:1></white>");
    assert.equal(rewriteGlyphTags("<glyph:auto:1..2>", glyphs.entries, diagnostics, "item.yml", "demo"), "<white><image:demo:auto:0:0></white><shift:-1><white><image:demo:auto:0:1></white><shift:-1>");
    assert.equal(rewriteGlyphTags("\\<glyph:auto>", glyphs.entries, diagnostics, "item.yml", "demo"), "\\<glyph:auto>");
  } finally { await rm(root, { recursive: true, force: true }); }
});
