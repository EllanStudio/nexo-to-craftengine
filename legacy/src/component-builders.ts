import type { DiagnosticBag } from "./diagnostics.js";
import { MINECRAFT_BLOCK_STATE_PROPERTIES_1_21_11 } from "./minecraft-block-states-1.21.11.js";
import { MINECRAFT_CONSUMABLE_COMPONENTS_1_21_11 } from "./minecraft-consumables-1.21.11.js";
import { MINECRAFT_DAMAGE_TYPE_IDS_1_21_11, MINECRAFT_DAMAGE_TYPE_TAG_VALUES_1_21_11 } from "./minecraft-damage-types-1.21.11.js";
import { MINECRAFT_BLOCK_IDS_1_21_11, MINECRAFT_ENTITY_TYPE_IDS_1_21_11, MINECRAFT_ITEM_IDS_1_21_11, MINECRAFT_JUKEBOX_SONG_IDS_1_21_11, MINECRAFT_MOB_EFFECT_IDS_1_21_11, MINECRAFT_SOUND_EVENT_IDS_1_21_11 } from "./minecraft-registries-1.21.11.js";
import { normalizeLocation } from "./resource-location.js";
import { deepClone, isObject, type JsonObject, type JsonValue } from "./types.js";

export interface BuilderComponentResult {
  status: "converted" | "manual";
  value?: JsonValue;
  reason?: string;
}

interface Context {
  diagnostics: DiagnosticBag;
  source: string;
  item: string;
  key: string;
  components: JsonObject;
  material: string;
}

const BUILDER_KEYS = new Set([
  "can_place_on", "can_break", "tool", "jukebox_playable", "use_remainder", "death_protection", "consumable", "equippable", "repairable",
  "weapon", "blocks_attacks", "attack_range", "kinetic_weapon", "piercing_weapon", "swing_animation", "use_effects",
]);

function converted(value?: JsonValue): BuilderComponentResult {
  return value === undefined ? { status: "converted" } : { status: "converted", value };
}

function manual(reason: string): BuilderComponentResult {
  return { status: "manual", reason };
}

function strings(value: JsonValue | undefined): string[] {
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") return String(value).length > 0 ? [String(value)] : [];
  if (!Array.isArray(value)) return [];
  return value.filter((entry): entry is string | number | boolean => typeof entry === "string" || typeof entry === "number" || typeof entry === "boolean").map(String).filter(Boolean);
}

function sections(value: JsonValue | undefined, directKeys: readonly string[]): JsonObject[] {
  if (Array.isArray(value)) return value.filter(isObject);
  if (!isObject(value)) return [];
  if (directKeys.some((key) => Object.prototype.hasOwnProperty.call(value, key))) return [value];
  return Object.values(value).filter(isObject);
}

function finite(value: JsonValue | undefined, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function integer(value: JsonValue | undefined, fallback: number): number {
  return Math.trunc(finite(value, fallback));
}

function boolean(value: JsonValue | undefined, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

function durationSeconds(value: JsonValue | undefined, fallback = 0): number {
  if (typeof value === "number" && Number.isFinite(value)) return Math.max(0, value);
  if (typeof value !== "string") return fallback;
  const match = /^\s*(-?(?:\d+(?:\.\d*)?|\.\d+))\s*(ms|ticks?|t|s|sec(?:onds?)?|m|min(?:utes?)?|h|hours?)?\s*$/i.exec(value);
  if (!match) return fallback;
  const amount = Number(match[1]);
  const unit = (match[2] ?? "s").toLowerCase();
  const multiplier = unit === "ms" ? 0.001 : unit === "t" || unit.startsWith("tick") ? 0.05 : unit === "m" || unit.startsWith("min") ? 60 : unit === "h" || unit.startsWith("hour") ? 3600 : 1;
  return Math.max(0, amount * multiplier);
}

function durationTicks(value: JsonValue | undefined, fallback: number): number {
  return Math.max(0, Math.trunc(durationSeconds(value, fallback / 20) * 20));
}

function resource(value: JsonValue | undefined, context: Context, field: string): string | undefined {
  if (typeof value !== "string" || value.trim().length === 0) return undefined;
  return normalizeLocation(value.trim().toLowerCase(), context.diagnostics, {
    source: context.source,
    item: context.item,
    field: "Components." + context.key + "." + field,
  });
}

function soundHolder(value: JsonValue | undefined, context: Context, field: string): JsonValue | undefined {
  const id = resource(value, context, field);
  if (!id) return undefined;
  return MINECRAFT_SOUND_EVENT_IDS_1_21_11.has(id) ? id : { sound_id: id };
}

function taggedResource(value: string, context: Context, field: string, forceTag = false): string | undefined {
  const tagged = forceTag || value.startsWith("#");
  const id = resource(tagged ? value.replace(/^#/, "") : value, context, field);
  return id ? (tagged ? "#" + id : id) : undefined;
}

function findValue(object: JsonObject, key: string): JsonValue | undefined {
  const match = Object.keys(object).find((entry) => entry.toLowerCase() === key.toLowerCase());
  return match === undefined ? undefined : object[match];
}

function registryId(value: string): string {
  const plain = value.replace(/^#/, "").toLowerCase();
  return plain.includes(":") ? plain : "minecraft:" + plain;
}

function isKnownItemId(value: string): boolean {
  return MINECRAFT_ITEM_IDS_1_21_11.has(registryId(value));
}

function isKnownBlockId(value: string): boolean {
  return MINECRAFT_BLOCK_IDS_1_21_11.has(registryId(value));
}

function convertBlockPredicates(raw: JsonValue, context: Context): BuilderComponentResult {
  const entries = sections(raw, ["block", "blocks", "nexo_block", "state"]);
  if (entries.length === 0) return manual("需要至少一个 block predicate section");
  const predicates: JsonObject[] = [];
  for (const [index, entry] of entries.entries()) {
    if (strings(entry.nexo_block).length > 0) return manual("nexo_block 需要解析运行时自定义方块状态");
    const stateSource = isObject(entry.state) ? entry.state : undefined;
    const direct: string[] = [];
    const tags: string[] = [];
    for (const block of strings(entry.block ?? entry.blocks)) {
      const normalized = taggedResource(block, context, "predicates[" + index + "].block", block.startsWith("#") || !isKnownBlockId(block));
      if (!normalized) return manual("存在无效方块或方块标签 ID");
      (normalized.startsWith("#") ? tags : direct).push(normalized);
    }
    if (direct.length > 0) {
      const predicate: JsonObject = { blocks: direct.length === 1 ? direct[0]! : direct };
      if (stateSource) {
        const allowed = new Set(MINECRAFT_BLOCK_STATE_PROPERTIES_1_21_11[direct[0]!] ?? []);
        const state: JsonObject = {};
        for (const [name, value] of Object.entries(stateSource)) {
          if (!allowed.has(name)) {
            context.diagnostics.warning("COMPONENT_BLOCK_STATE_PROPERTY_IGNORED", "Nexo ignores unknown block-state property " + name + " for " + direct[0], { source: context.source, item: context.item, field: "Components." + context.key + ".predicates[" + index + "].state." + name });
            continue;
          }
          if (typeof value !== "string" && typeof value !== "number" && typeof value !== "boolean") return manual("state 中包含无法静态编码的非标量属性值");
          state[name] = String(value);
        }
        if (Object.keys(state).length > 0) predicate.state = state;
      }
      predicates.push(predicate);
    }
    for (const tag of tags) predicates.push({ blocks: tag });
  }
  if (predicates.length === 0) return manual("没有可编码的原版方块或标签");
  return converted(predicates.length === 1 ? predicates[0]! : predicates);
}

function convertTool(raw: JsonValue, context: Context): BuilderComponentResult {
  if (!isObject(raw)) return manual("tool 必须是 section");
  const result: JsonObject = { rules: [], can_destroy_blocks_in_creative: false };
  const miningSpeed = Math.max(0, finite(raw.default_mining_speed, 1));
  const damage = Math.max(0, integer(raw.damage_per_block, 1));
  if (miningSpeed !== 1) result.default_mining_speed = miningSpeed;
  if (damage !== 1) result.damage_per_block = damage;
  const outputRules: JsonObject[] = [];
  for (const [index, rule] of sections(raw.rules, ["material", "materials", "tag", "tags", "speed", "correct_for_drops"]).entries()) {
    const speed = finite(rule.speed, 1);
    if (speed <= 0) return manual("tool rule speed 必须大于 0 才能通过 1.21.11 codec");
    const correct = boolean(rule.correct_for_drops, false);
    const materials: string[] = [];
    for (const material of strings(rule.material ?? rule.materials)) {
      if (!isKnownBlockId(material)) {
        context.diagnostics.warning("COMPONENT_TOOL_BLOCK_INVALID", "Nexo ignores a tool material that is not a Minecraft 1.21.11 block: " + material, { source: context.source, item: context.item, field: "Components.tool.rules[" + index + "].materials" });
        continue;
      }
      const normalized = resource(material, context, "rules[" + index + "].materials");
      if (normalized) materials.push(normalized);
    }
    if (materials.length > 0) outputRules.push({ blocks: materials.length === 1 ? materials[0]! : materials, speed, correct_for_drops: correct });
    for (const tag of strings(rule.tag ?? rule.tags)) {
      const normalized = taggedResource(tag, context, "rules[" + index + "].tags", true);
      if (normalized) outputRules.push({ blocks: normalized, speed, correct_for_drops: correct });
    }
  }
  result.rules = outputRules;
  return converted(result);
}

function convertJukebox(raw: JsonValue, context: Context): BuilderComponentResult {
  const songValue = isObject(raw) ? raw.song ?? raw.song_key : raw;
  const song = resource(songValue, context, "song");
  if (!song) return manual("jukebox_playable 缺少可编码的 song key");
  if (song.startsWith("minecraft:") && !MINECRAFT_JUKEBOX_SONG_IDS_1_21_11.has(song)) return manual("未知的 vanilla jukebox song 需要运行时 registry");
  return converted(song);
}

function convertUseRemainder(raw: JsonValue, context: Context): BuilderComponentResult {
  if (typeof raw === "string") {
    if (!isKnownItemId(raw)) return manual("use_remainder 的 minecraft_type 不是 1.21.11 item registry entry");
    const id = resource(raw, context, "minecraft_type");
    return id ? converted({ id, count: 1 }) : manual("use_remainder 的物品 ID 无效");
  }
  if (!isObject(raw)) return manual("use_remainder 必须是 section");
  if (raw.nexo_item !== undefined || raw.crucible_item !== undefined || raw.mmoitems_id !== undefined || raw.mmoitems_type !== undefined || raw.minecraft_item !== undefined) {
    return manual("自定义或序列化 ItemStack 余留物需要运行时物品注册表");
  }
  if (typeof raw.minecraft_type !== "string" || !isKnownItemId(raw.minecraft_type)) return manual("仅有效的 Minecraft 1.21.11 minecraft_type 余留物可安全静态转换");
  const id = resource(raw.minecraft_type, context, "minecraft_type");
  if (!id) return manual("仅 minecraft_type 余留物可安全静态转换");
  return converted({ id, count: clamp(integer(raw.amount, 1), 1, 99) });
}

interface EffectsResult {
  effects: JsonObject[];
  unknown: string[];
}

function convertEffects(raw: JsonValue | undefined, context: Context, field: string): EffectsResult {
  if (!isObject(raw)) return { effects: [], unknown: [] };
  const output: JsonObject[] = [];
  const known = new Set(["apply_effects", "remove_effects", "clear_all_effects", "teleport_randomly", "play_sound"]);
  const unknown = Object.keys(raw).filter((key) => !known.has(key.toLowerCase()));
  const applyRaw = findValue(raw, "apply_effects");
  if (isObject(applyRaw)) {
    for (const [effectName, effectValue] of Object.entries(applyRaw)) {
      if (!isObject(effectValue)) continue;
      const id = resource(effectName, context, field + ".APPLY_EFFECTS." + effectName);
      if (!id) continue;
      if (!MINECRAFT_MOB_EFFECT_IDS_1_21_11.has(id)) {
        context.diagnostics.warning("COMPONENT_EFFECT_UNKNOWN_IGNORED", "Nexo ignores unknown mob effect " + id, { source: context.source, item: context.item, field: "Components." + context.key + "." + field + ".APPLY_EFFECTS." + effectName });
        continue;
      }
      const effect = effectValue;
      const instance: JsonObject = {
        id,
        amplifier: integer(effect.amplifier, 0),
        duration: durationTicks(effect.duration, 0),
        ambient: boolean(effect.ambient, true),
        show_particles: boolean(effect.show_particles, true),
        show_icon: boolean(effect.show_icon, true),
      };
      output.push({ type: "minecraft:apply_effects", effects: [instance], probability: clamp(finite(effect.probability, 1), 0, 1) });
    }
  }
  const removed: string[] = [];
  const removeRaw = findValue(raw, "remove_effects");
  for (const effectName of strings(Array.isArray(removeRaw) ? removeRaw : undefined)) {
    const id = resource(effectName, context, field + ".REMOVE_EFFECTS");
    if (id && MINECRAFT_MOB_EFFECT_IDS_1_21_11.has(id)) removed.push(id);
    else if (id) context.diagnostics.warning("COMPONENT_EFFECT_UNKNOWN_IGNORED", "Nexo ignores unknown mob effect " + id, { source: context.source, item: context.item, field: "Components." + context.key + "." + field + ".REMOVE_EFFECTS" });
  }
  if (removed.length > 0) output.push({ type: "minecraft:remove_effects", effects: removed.length === 1 ? removed[0]! : removed });
  if (findValue(raw, "clear_all_effects") !== undefined) output.push({ type: "minecraft:clear_all_effects" });
  const teleportRaw = findValue(raw, "teleport_randomly");
  if (isObject(teleportRaw)) {
    const diameter = finite(teleportRaw.diameter, 16);
    if (diameter <= 0) unknown.push("TELEPORT_RANDOMLY.diameter 必须大于 0");
    else output.push({ type: "minecraft:teleport_randomly", diameter });
  }
  const soundRaw = findValue(raw, "play_sound");
  if (isObject(soundRaw)) {
    const soundId = resource(soundRaw.sound ?? soundRaw.sound_id, context, field + ".PLAY_SOUND.sound");
    if (soundId && MINECRAFT_SOUND_EVENT_IDS_1_21_11.has(soundId)) output.push({ type: "minecraft:play_sound", sound: soundId });
    else if (soundId) context.diagnostics.warning("COMPONENT_SOUND_UNKNOWN_IGNORED", "Nexo ignores unknown sound event " + soundId, { source: context.source, item: context.item, field: "Components." + context.key + "." + field + ".PLAY_SOUND.sound" });
  }
  return { effects: output, unknown };
}

function convertDeathProtection(raw: JsonValue, context: Context): BuilderComponentResult {
  if (!isObject(raw)) return manual("death_protection 必须是 section");
  const convertedEffects = convertEffects(raw.death_effects, context, "death_effects");
  if (convertedEffects.unknown.length > 0) return manual("包含未知 death effect: " + convertedEffects.unknown.join(", "));
  return converted({ death_effects: convertedEffects.effects });
}

function convertConsumable(raw: JsonValue, context: Context): BuilderComponentResult {
  if (!isObject(raw)) return manual("consumable 必须是 section");
  const template = MINECRAFT_CONSUMABLE_COMPONENTS_1_21_11[registryId(context.material)];
  const result: JsonObject = template ? deepClone(template) : {};
  if (raw.consume_duration !== undefined || raw.consume_seconds !== undefined) {
    result.consume_seconds = raw.consume_duration !== undefined ? durationSeconds(raw.consume_duration, 1.6) : Math.max(0, finite(raw.consume_seconds, 1.6));
  }
  if (raw.animation !== undefined) {
    const animationText = typeof raw.animation === "string" ? raw.animation.toLowerCase() : "eat";
    const animations = new Set(["none", "eat", "drink", "block", "bow", "spear", "crossbow", "spyglass", "toot_horn", "brush", "bundle"]);
    result.animation = animations.has(animationText) ? animationText : "eat";
  }
  if (raw.consume_particles !== undefined || raw.has_consume_particles !== undefined) {
    result.has_consume_particles = boolean(raw.consume_particles ?? raw.has_consume_particles, true);
  }
  if (raw.sound !== undefined) {
    const sound = soundHolder(raw.sound, context, "sound");
    if (sound) result.sound = sound;
  }
  if (raw.effects !== undefined || raw.on_consume_effects !== undefined) {
    const convertedEffects = convertEffects(raw.effects ?? raw.on_consume_effects, context, "effects");
    if (convertedEffects.unknown.length > 0) return manual("包含未知 consume effect: " + convertedEffects.unknown.join(", "));
    if (convertedEffects.effects.length > 0) result.on_consume_effects = convertedEffects.effects;
    else delete result.on_consume_effects;
  }
  return converted(result);
}

function convertEquippable(raw: JsonValue, context: Context): BuilderComponentResult {
  if (!isObject(raw)) return manual("equippable 必须是 section");
  if (typeof raw.slot !== "string") return manual("equippable.slot 缺失");
  const sourceSlot = raw.slot.toLowerCase();
  const slotAliases: Record<string, string> = { hand: "mainhand", off_hand: "offhand" };
  const slot = slotAliases[sourceSlot] ?? sourceSlot;
  if (!new Set(["mainhand", "offhand", "feet", "legs", "chest", "head", "body", "saddle"]).has(slot)) return manual("equippable.slot 不是 Minecraft 1.21.11 的有效槽位");
  const result: JsonObject = { slot };
  const allowed: string[] = [];
  for (const entity of strings(raw.allowed_entity_types ?? raw.allowed_entity_type)) {
    const id = resource(entity, context, "allowed_entity_types");
    if (id && !MINECRAFT_ENTITY_TYPE_IDS_1_21_11.has(id)) return manual("allowed_entity_types 包含未知的 1.21.11 entity type: " + id);
    if (id) allowed.push(id);
  }
  if (allowed.length > 0) result.allowed_entities = allowed.length === 1 ? allowed[0]! : allowed;
  const asset = resource(raw.asset_id, context, "asset_id");
  if (asset) result.asset_id = asset;
  const overlay = resource(raw.camera_overlay, context, "camera_overlay");
  if (overlay) result.camera_overlay = overlay;
  const gliderLike = boolean(context.components.glider, context.material !== "elytra");
  const equipSound = soundHolder(raw.equip_sound, context, "equip_sound") ?? (gliderLike ? "minecraft:item.armor.equip_elytra" : "minecraft:item.armor.equip_generic");
  if (equipSound !== "minecraft:item.armor.equip_generic") result.equip_sound = equipSound;
  const shearSound = soundHolder(raw.shear_sound ?? raw.shearing_sound, context, "shear_sound");
  if (shearSound && shearSound !== "minecraft:item.shears.snip") result.shearing_sound = shearSound;
  for (const [sourceKey, targetKey, nexoDefault, codecDefault] of [
    ["dispensable", "dispensable", true, true], ["swappable", "swappable", true, true], ["damage_on_hurt", "damage_on_hurt", gliderLike, true],
    ["equip_on_interact", "equip_on_interact", false, false], ["can_be_sheared", "can_be_sheared", context.item.includes("harness"), false],
  ] as const) {
    const value = boolean(raw[sourceKey], nexoDefault);
    if (value !== codecDefault) result[targetKey] = value;
  }
  return converted(result);
}

function convertRepairable(raw: JsonValue, context: Context): BuilderComponentResult {
  const direct: string[] = [];
  const tags: string[] = [];
  for (const entry of strings(raw)) {
    const forceTag = entry.startsWith("#") || !isKnownItemId(entry);
    const id = taggedResource(entry, context, "items", forceTag);
    if (id) (id.startsWith("#") ? tags : direct).push(id);
  }
  if (direct.length === 0 && tags.length === 0) return manual("repairable 没有可解析的原版物品或标签");
  if (tags.length > 1 || (tags.length > 0 && direct.length > 0)) return manual("多个标签或标签与物品混合需要展开运行时 item registry");
  return converted({ items: tags[0] ?? (direct.length === 1 ? direct[0]! : direct) });
}

function convertWeapon(raw: JsonValue): BuilderComponentResult {
  if (!isObject(raw)) return manual("weapon 必须是 section");
  const result: JsonObject = {};
  const damage = Math.max(0, integer(raw.damage_per_attack ?? raw.item_damage_per_attack, 1));
  const disable = durationSeconds(raw.disable_blocking ?? raw.disable_blocking_for_seconds, 0);
  if (damage !== 1) result.item_damage_per_attack = damage;
  if (disable !== 0) result.disable_blocking_for_seconds = disable;
  return converted(result);
}

function convertBlocksAttacks(raw: JsonValue, context: Context): BuilderComponentResult {
  if (!isObject(raw)) return manual("blocks_attacks 必须是 section");
  const result: JsonObject = {};
  const delay = durationSeconds(raw.block_delay ?? raw.block_delay_seconds, 0);
  const cooldown = Math.max(0, finite(raw.disable_cooldown_scale, 1));
  if (delay !== 0) result.block_delay_seconds = delay;
  if (cooldown !== 1) result.disable_cooldown_scale = cooldown;
  const blockSound = soundHolder(raw.block_sound, context, "block_sound");
  if (blockSound) result.block_sound = blockSound;
  const disableSound = soundHolder(raw.disable_sound ?? raw.disabled_sound, context, "disable_sound");
  if (disableSound) result.disabled_sound = disableSound;
  if (typeof raw.bypassed_by === "string") {
    const bypassed = taggedResource(raw.bypassed_by, context, "bypassed_by", true);
    if (bypassed) result.bypassed_by = bypassed;
  }
  if (isObject(raw.item_damage)) {
    result.item_damage = {
      threshold: Math.max(0, finite(raw.item_damage.threshold, 0)),
      base: finite(raw.item_damage.base, 1),
      factor: finite(raw.item_damage.factor, 1),
    };
  }
  const reductions: JsonObject[] = [];
  for (const [index, reduction] of sections(raw.damage_reductions, ["base", "factor", "horizontal_blocking", "type", "types"]).entries()) {
    const encoded: JsonObject = { base: finite(reduction.base, 1), factor: finite(reduction.factor, 1) };
    const angle = Math.max(0, finite(reduction.horizontal_blocking ?? reduction.horizontal_blocking_angle, 90));
    if (angle <= 0) return manual("damage reduction horizontal_blocking 必须大于 0 才能通过 1.21.11 codec");
    if (angle !== 90) encoded.horizontal_blocking_angle = angle;
    const directTypes: string[] = [];
    const typeTags: string[] = [];
    for (const type of strings(reduction.type ?? reduction.types)) {
      const id = resource(type.replace(/^#/, ""), context, "damage_reductions[" + index + "].type");
      if (!id) continue;
      if (type.startsWith("#") || Object.prototype.hasOwnProperty.call(MINECRAFT_DAMAGE_TYPE_TAG_VALUES_1_21_11, id)) typeTags.push("#" + id);
      else if (MINECRAFT_DAMAGE_TYPE_IDS_1_21_11.has(id)) directTypes.push(id);
      else return manual("damage_reductions.type 包含未知的运行时 damage type: " + id);
    }
    if (typeTags.length > 1 || (typeTags.length > 0 && directTypes.length > 0)) return manual("多个 damage type 标签或标签与具体类型混合需要展开运行时 registry");
    if (typeTags.length === 1) encoded.type = typeTags[0]!;
    else if (directTypes.length > 0) encoded.type = directTypes.length === 1 ? directTypes[0]! : directTypes;
    reductions.push(encoded);
  }
  if (reductions.length > 0) result.damage_reductions = reductions;
  return converted(result);
}

function parseRange(raw: JsonObject, minKey: string, maxKey: string, combinedKey: string, fallbackMin: number, fallbackMax: number): [number, number] {
  let min = fallbackMin;
  let max = fallbackMax;
  const combined = raw[combinedKey];
  if (typeof combined === "number" && Number.isFinite(combined)) max = combined;
  if (typeof combined === "string") {
    const parts = combined.split("..");
    if (parts.length === 2 && Number.isFinite(Number(parts[0])) && Number.isFinite(Number(parts[1]))) {
      min = Number(parts[0]);
      max = Number(parts[1]);
    } else if (Number.isFinite(Number(combined))) max = Number(combined);
  }
  if (typeof raw[minKey] === "number" && Number.isFinite(raw[minKey])) min = raw[minKey];
  if (typeof raw[maxKey] === "number" && Number.isFinite(raw[maxKey])) max = raw[maxKey];
  return [clamp(min, 0, 64), clamp(max, 0, 64)];
}

function convertAttackRange(raw: JsonValue): BuilderComponentResult {
  if (!isObject(raw)) return manual("attack_range 必须是 section");
  const [minReach, maxReach] = parseRange(raw, "min_reach", "max_reach", "reach", 0, 3);
  const [minCreative, maxCreative] = parseRange(raw, "min_creative_reach", "max_creative_reach", "creative_reach", 0, 5);
  const result: JsonObject = {};
  if (minReach !== 0) result.min_reach = minReach;
  if (maxReach !== 3) result.max_reach = maxReach;
  if (minCreative !== 0) result.min_creative_reach = minCreative;
  if (maxCreative !== 5) result.max_creative_reach = maxCreative;
  const margin = clamp(finite(raw.hitbox_margin, 0.3), 0, 1);
  const factor = clamp(finite(raw.mob_factor, 1), 0, 2);
  if (margin !== 0.3) result.hitbox_margin = margin;
  if (factor !== 1) result.mob_factor = factor;
  return converted(result);
}

function kineticCondition(raw: JsonValue | undefined): JsonObject | undefined {
  if (!isObject(raw)) return undefined;
  const duration = durationTicks(raw.max_duration ?? raw.max_duration_ticks, 0);
  const result: JsonObject = { max_duration_ticks: duration };
  const speed = finite(raw.min_speed, 0);
  const relative = finite(raw.min_relative_speed, 0);
  if (speed !== 0) result.min_speed = speed;
  if (relative !== 0) result.min_relative_speed = relative;
  return result;
}

function convertKinetic(raw: JsonValue, context: Context): BuilderComponentResult {
  if (!isObject(raw)) return manual("kinetic_weapon 必须是 section");
  const result: JsonObject = {};
  const contact = durationTicks(raw.contact_cooldown ?? raw.contact_cooldown_ticks, 10);
  const delay = durationTicks(raw.delay ?? raw.delay_ticks, 0);
  const movement = finite(raw.forward_movement, 0);
  const multiplier = finite(raw.damage_multiplier, 1);
  if (contact !== 10) result.contact_cooldown_ticks = contact;
  if (delay !== 0) result.delay_ticks = delay;
  if (movement !== 0) result.forward_movement = movement;
  if (multiplier !== 1) result.damage_multiplier = multiplier;
  const sound = soundHolder(raw.sound, context, "sound");
  if (sound) result.sound = sound;
  const hitSound = soundHolder(raw.hit_sound, context, "hit_sound");
  if (hitSound) result.hit_sound = hitSound;
  for (const key of ["dismount_conditions", "knockback_conditions", "damage_conditions"] as const) {
    const condition = kineticCondition(raw[key]);
    if (condition) result[key] = condition;
  }
  return converted(result);
}

function convertPiercing(raw: JsonValue, context: Context): BuilderComponentResult {
  if (!isObject(raw)) return manual("piercing_weapon 必须是 section");
  const result: JsonObject = {};
  if (!boolean(raw.deals_knockback, true)) result.deals_knockback = false;
  if (boolean(raw.dismounts, false)) result.dismounts = true;
  const sound = soundHolder(raw.sound, context, "sound");
  if (sound) result.sound = sound;
  const hitSound = soundHolder(raw.hit_sound, context, "hit_sound");
  if (hitSound) result.hit_sound = hitSound;
  return converted(result);
}

function convertSwing(raw: JsonValue): BuilderComponentResult {
  if (!isObject(raw)) return manual("swing_animation 必须是 section");
  const result: JsonObject = {};
  const typeText = typeof raw.type === "string" ? raw.type.toLowerCase() : "whack";
  const type = new Set(["none", "whack", "stab"]).has(typeText) ? typeText : "whack";
  const duration = durationTicks(raw.duration, 6);
  if (duration <= 0) return manual("swing_animation.duration 必须至少为 1 tick");
  if (type !== "whack") result.type = type;
  if (duration !== 6) result.duration = duration;
  return converted(result);
}

function convertUseEffects(raw: JsonValue): BuilderComponentResult {
  if (!isObject(raw)) return manual("use_effects 必须是 section");
  const result: JsonObject = {};
  const sprint = boolean(raw.can_sprint, false);
  const vibrations = boolean(raw.interact_vibrations, true);
  const speed = clamp(finite(raw.speed_multiplier, 0.2), 0, 1);
  if (sprint) result.can_sprint = true;
  if (!vibrations) result.interact_vibrations = false;
  if (speed !== 0.2) result.speed_multiplier = speed;
  return converted(result);
}

export function convertNexoBuilderComponent(
  key: string,
  raw: JsonValue,
  diagnostics: DiagnosticBag,
  source: string,
  item: string,
  components: JsonObject,
  material: string,
): BuilderComponentResult | undefined {
  if (!BUILDER_KEYS.has(key)) return undefined;
  const context: Context = { diagnostics, source, item, key, components, material };
  switch (key) {
    case "can_place_on":
    case "can_break": return convertBlockPredicates(raw, context);
    case "tool": return convertTool(raw, context);
    case "jukebox_playable": return convertJukebox(raw, context);
    case "use_remainder": return convertUseRemainder(raw, context);
    case "death_protection": return convertDeathProtection(raw, context);
    case "consumable": return convertConsumable(raw, context);
    case "equippable": return convertEquippable(raw, context);
    case "repairable": return convertRepairable(raw, context);
    case "weapon": return convertWeapon(raw);
    case "blocks_attacks": return convertBlocksAttacks(raw, context);
    case "attack_range": return convertAttackRange(raw);
    case "kinetic_weapon": return convertKinetic(raw, context);
    case "piercing_weapon": return convertPiercing(raw, context);
    case "swing_animation": return convertSwing(raw);
    case "use_effects": return convertUseEffects(raw);
  }
}
