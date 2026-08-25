import type { DiagnosticBag } from "./diagnostics.js";
import { normalizeSoundLocation } from "./resource-location.js";
import {
  asStringList,
  deepClone,
  deepMerge,
  getBoolean,
  getNumber,
  getObject,
  getString,
  getValue,
  isObject,
  type JsonObject,
  type JsonValue,
} from "./types.js";

export interface MechanicsConversion {
  behavior: JsonObject[];
  furniture?: JsonObject;
  block?: JsonObject;
  semantics: JsonObject;
}

interface Context {
  source: string;
  item: string;
  targetId: string;
  diagnostics: DiagnosticBag;
}

function detail(context: Context, field: string, lossy = false): { source: string; item: string; field: string; lossy: boolean } {
  return { source: context.source, item: context.item, field, lossy };
}

function parseNumberList(value: JsonValue | undefined): number[] | undefined {
  if (typeof value === "number") return [value];
  if (typeof value === "string") {
    const numbers = value.split(",").map((part) => Number(part.trim()));
    return numbers.every(Number.isFinite) ? numbers : undefined;
  }
  if (Array.isArray(value)) {
    const numbers = value.map(Number);
    return numbers.every(Number.isFinite) ? numbers : undefined;
  }
  return undefined;
}

function splitWithLast(value: string, separator: string, limit: number): string[] {
  if (limit <= 1) return [value];
  const result: string[] = [];
  let rest = value;
  while (result.length < limit - 1) {
    const index = rest.indexOf(separator);
    if (index < 0) break;
    result.push(rest.slice(0, index));
    rest = rest.slice(index + separator.length);
  }
  result.push(rest);
  return result;
}

// NexoYaml.vector3f parses a scalar/string component-by-component and fills
// missing or invalid components with the supplied default. It does not treat a
// scalar as a uniform vector.
function configVector(value: JsonValue | undefined, fallback: number): [number, number, number] {
  if (typeof value !== "number" && typeof value !== "string") return [fallback, fallback, fallback];
  const parts = splitWithLast(String(value), ",", 3);
  const component = (index: number): number => {
    const raw = parts[index]?.trim();
    if (raw === undefined || raw === "") return fallback;
    const parsed = Number(raw);
    return Number.isFinite(parsed) ? parsed : fallback;
  };
  return [component(0), component(1), component(2)];
}

// VectorUtils.vector3fFromString/vectorFromString use per-component zero
// fallbacks and retain only the first three components.
function compactVector(value: string | undefined, fallback = 0): [number, number, number] {
  if (value === undefined) return [fallback, fallback, fallback];
  const parts = value.replaceAll(" ", "").split(",");
  const component = (index: number): number => {
    const raw = parts[index];
    if (raw === undefined || raw === "") return fallback;
    const parsed = Number(raw);
    return Number.isFinite(parsed) ? parsed : fallback;
  };
  return [component(0), component(1), component(2)];
}

function vectorString(value: JsonValue | undefined, fallback: number): string {
  return configVector(value, fallback).join(",");
}

interface Quaternion { x: number; y: number; z: number; w: number }
const IDENTITY: Quaternion = { x: 0, y: 0, z: 0, w: 1 };

function axisQuaternion(axis: "x" | "y", degrees: number): Quaternion {
  const half = degrees * Math.PI / 360;
  const sine = Math.sin(half);
  return axis === "x" ? { x: sine, y: 0, z: 0, w: Math.cos(half) } : { x: 0, y: sine, z: 0, w: Math.cos(half) };
}

function parseQuaternion(value: JsonValue | undefined, side: "left" | "right"): Quaternion {
  if (typeof value === "number" && Number.isFinite(value)) return axisQuaternion(side === "left" ? "y" : "x", value);
  if (typeof value !== "string") return IDENTITY;
  const parts = splitWithLast(value, ",", 4);
  if (parts.length < 4) return IDENTITY;
  const component = (index: number, fallback: number): number => {
    const parsed = Number(parts[index]?.trim());
    return Number.isFinite(parsed) ? parsed : fallback;
  };
  return { x: component(0, 0), y: component(1, 0), z: component(2, 0), w: component(3, 1) };
}

function multiplyQuaternion(a: Quaternion, b: Quaternion): Quaternion {
  return {
    x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
    y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
    z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
    w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
  };
}

function quaternionIdentity(value: Quaternion): boolean {
  return Math.abs(value.x) < 1e-8 && Math.abs(value.y) < 1e-8 && Math.abs(value.z) < 1e-8 && Math.abs(value.w - 1) < 1e-8;
}

function quaternionString(value: Quaternion): string {
  return [value.x, value.y, value.z, value.w].map((part) => Number(part.toFixed(8))).join(",");
}

function uniformScale(value: JsonValue | undefined, fallback: number): boolean {
  const [x, y, z] = configVector(value, fallback);
  return Math.abs(y - x) < 1e-8 && Math.abs(z - x) < 1e-8;
}

function canRecomposeFixedQuarterTurn(properties: JsonObject | undefined): boolean {
  if (!properties) return true;
  const [translationX, , translationZ] = configVector(getValue(properties, "translation"), 0);
  if (Math.abs(translationX) >= 1e-8 || Math.abs(translationZ) >= 1e-8) return false;
  return quaternionIdentity(parseQuaternion(getValue(properties, "left_rotation"), "left"))
    && quaternionIdentity(parseQuaternion(getValue(properties, "right_rotation"), "right"));
}

function mapElement(properties: JsonObject | undefined, context: Context): JsonObject {
  const transform = (properties ? getString(properties, "display_transform") : undefined)?.toLowerCase() ?? "none";
  const defaultScale = transform === "fixed" ? 0.5 : 1;
  const element: JsonObject = {
    type: "item_display",
    item: context.targetId,
    // Nexo stores the placed source item's color and applies it to the display
    // stack. CraftEngine's tint source performs the equivalent component copy.
    tint_source: ["minecraft:dyed_color"],
    translation: vectorString(properties ? getValue(properties, "translation") : undefined, 0),
    scale: vectorString(properties ? getValue(properties, "scale") : undefined, defaultScale),
    display_transform: transform,
    billboard: (properties ? getString(properties, "tracking_rotation") : undefined)?.toLowerCase() ?? "fixed",
  };
  if (!properties) return element;
  const left = parseQuaternion(getValue(properties, "left_rotation"), "left");
  const right = parseQuaternion(getValue(properties, "right_rotation"), "right");
  if (!quaternionIdentity(left) || !quaternionIdentity(right)) {
    const combined = multiplyQuaternion(left, right);
    element.rotation = quaternionString(combined);
    if (!quaternionIdentity(right) && !uniformScale(getValue(properties, "scale"), defaultScale)) {
      context.diagnostics.warning("FURNITURE_RIGHT_ROTATION_NON_UNIFORM", "Nexo applies Minecraft Translation*LeftRotation*Scale*RightRotation, but CraftEngine exposes one pre-scale rotation; moving a non-identity right rotation before non-uniform scale is not exact", detail(context, "Mechanics.furniture.properties", true));
    }
  }
  const passthrough = ["view_range", "shadow_strength", "shadow_radius", "glow_color"];
  for (const key of passthrough) {
    const value = getValue(properties, key);
    if (value !== undefined) element[key] = deepClone(value);
  }
  const brightness = getObject(properties, "brightness");
  if (brightness) element.brightness = deepClone(brightness);
  for (const unsupported of ["display_width", "display_height", "delay", "cullable"]) {
    if (getValue(properties, unsupported) !== undefined) {
      context.diagnostics.warning("FURNITURE_DISPLAY_PROPERTY_UNSUPPORTED", "CraftEngine 26.8 item-display furniture has no equivalent for Nexo " + unsupported, detail(context, "Mechanics.furniture.properties." + unsupported, true));
    }
  }
  return element;
}

function splitWords(value: string): string[] {
  return value.trim().split(/\s+/).filter(Boolean);
}

// Nexo rotates furniture offsets as (x*cos + z*sin, x*sin - z*cos),
// while CraftEngine's local furniture coordinates use the opposite X/Z basis.
function nexoPosition(value: string | undefined): string {
  const [x, y, z] = compactVector(value);
  return [-x, y, -z].join(",");
}

function nexoSeat(value: string): string {
  // Nexo's seats list is parsed as a plain Vector; there is no seat-yaw token.
  return nexoPosition(value);
}

function finiteToken(value: string | undefined, fallback: number): number {
  if (value === undefined || value.trim() === "") return fallback;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function firstWordAndRest(value: string): [string, string | undefined] {
  const trimmed = value.trim();
  const match = /\s/.exec(trimmed);
  if (!match || match.index === undefined) return [trimmed || "0,0,0", undefined];
  return [trimmed.slice(0, match.index), trimmed.slice(match.index).trim() || undefined];
}

function parseInteraction(value: string, context: Context): JsonObject {
  const [position, size] = firstWordAndRest(value);
  const comma = size?.indexOf(",") ?? -1;
  const widthRaw = size === undefined ? undefined : comma < 0 ? size : size.slice(0, comma);
  // Kotlin substringAfter uses the whole source as its missing-delimiter value,
  // so a single size number means equal width and height.
  const heightRaw = size === undefined ? undefined : comma < 0 ? size : size.slice(comma + 1);
  const width = finiteToken(widthRaw, 1);
  const height = finiteToken(heightRaw, 1);
  return {
    type: "interaction",
    position: nexoPosition(position),
    width,
    height,
    interactive: true,
    blocks_building: true,
    can_use_item_on: true,
    can_be_hit_by_projectile: true,
    invisible: false,
  };
}

const BUKKIT_BLOCK_FACES = new Set([
  "NORTH", "EAST", "SOUTH", "WEST", "UP", "DOWN", "NORTH_EAST", "NORTH_WEST", "SOUTH_EAST", "SOUTH_WEST",
  "WEST_NORTH_WEST", "NORTH_NORTH_WEST", "NORTH_NORTH_EAST", "EAST_NORTH_EAST", "EAST_SOUTH_EAST", "SOUTH_SOUTH_EAST",
  "SOUTH_SOUTH_WEST", "WEST_SOUTH_WEST", "SELF",
]);
const CE_DIRECTIONS = new Set(["NORTH", "EAST", "SOUTH", "WEST", "UP", "DOWN"]);

function parseShulker(value: string, context: Context): JsonObject {
  const words = splitWords(value);
  const position = words[0] ?? "0,0,0";
  const scale = finiteToken(words[1], 1);
  const rawLength = finiteToken(words[2], 1);
  const length = Math.max(1, Math.min(2, rawLength));
  const rawDirection = words[3];
  let direction = rawDirection && CE_DIRECTIONS.has(rawDirection) ? rawDirection.toLowerCase() : "down";
  if (rawDirection && BUKKIT_BLOCK_FACES.has(rawDirection) && !CE_DIRECTIONS.has(rawDirection)) {
    context.diagnostics.warning("SHULKER_DIRECTION_UNSUPPORTED", "Nexo accepts diagonal/SELF BlockFace directions that CraftEngine's shulker hitbox cannot represent; DOWN was used", detail(context, "Mechanics.furniture.hitbox.shulkers", true));
    direction = "down";
  }
  const visibleRaw = words[4] ?? words[3] ?? "false";
  const visible = visibleRaw.toLowerCase() === "true";
  const peek = Math.round(100 / Math.PI * Math.acos(Math.max(-1, Math.min(1, 3 - 2 * length))));
  if (visible) context.diagnostics.warning("SHULKER_VISIBLE_UNSUPPORTED", "CraftEngine 26.8 always makes the Shulker entity invisible; its invisible key only affects an optional Interaction entity", detail(context, "Mechanics.furniture.hitbox.shulkers", true));
  return {
    type: "shulker",
    position: nexoPosition(position),
    scale,
    peek,
    direction,
    interaction_entity: false,
    interactive: true,
    blocks_building: true,
    can_use_item_on: true,
    can_be_hit_by_projectile: true,
    invisible: false,
  };
}

function parseGhast(value: string, context: Context): JsonObject {
  const words = splitWords(value);
  const rotation = finiteToken(words[2], 0);
  // Kotlin toBooleanStrictOrNull is case-sensitive. When rotation is omitted,
  // the final true/false token is also inspected as the visibility shorthand.
  const visible = (words.at(-1) ?? "") === "true";
  if (rotation !== 0) context.diagnostics.warning("GHAST_ROTATION_UNSUPPORTED", "CraftEngine happy_ghast hitbox has no rotation setting", detail(context, "Mechanics.furniture.hitbox.ghasts", true));
  if (visible) context.diagnostics.warning("GHAST_VISIBLE_UNSUPPORTED", "CraftEngine happy_ghast hitbox has no Nexo-compatible visible debug state", detail(context, "Mechanics.furniture.hitbox.ghasts", true));
  return {
    type: "happy_ghast",
    position: nexoPosition(words[0]),
    scale: finiteToken(words[1], 0.25),
    hard_collision: true,
    blocks_building: true,
    can_use_item_on: true,
    can_be_hit_by_projectile: true,
  };
}

function parseInteger(value: string | undefined): number {
  const normalized = value?.replaceAll(" ", "") ?? "";
  return /^-?\d+$/.test(normalized) ? Number(normalized) : 0;
}

function parseBarrierPosition(value: string): string {
  if (value === "origin") return nexoPosition("0,0,0");
  const parts = value.split(",");
  return nexoPosition([parseInteger(parts[0]), parseInteger(parts[1]), parseInteger(parts[2])].join(","));
}

const MAX_FURNITURE_BARRIER_HITBOXES = 4096;

function parseBarrierPositions(value: string, context: Context): string[] {
  if (value === "origin") return [nexoPosition("0,0,0")];
  if (!value.includes("..")) return [parseBarrierPosition(value)];
  const parts = splitWithLast(value, ",", 3);
  if (parts.length < 3) {
    context.diagnostics.error("BARRIER_RANGE_INVALID", "Nexo barrier range must contain x,y,z coordinates: " + value, detail(context, "Mechanics.furniture.hitbox.barriers"));
    return [];
  }
  const range = (raw: string): [number, number] => {
    const normalized = raw.replaceAll(" ", "");
    const match = /^(-?\d+)\.\.(-?\d+)$/.exec(normalized);
    if (match) return [Number(match[1]), Number(match[2])];
    const point = parseInteger(normalized);
    return [point, point];
  };
  const [xs, ys, zs] = parts.map(range) as [[number, number], [number, number], [number, number]];
  const endpoints = [...xs, ...ys, ...zs];
  const cardinality = Math.max(0, xs[1] - xs[0] + 1)
    * Math.max(0, ys[1] - ys[0] + 1)
    * Math.max(0, zs[1] - zs[0] + 1);
  if (!endpoints.every(Number.isSafeInteger) || !Number.isSafeInteger(cardinality) || cardinality > MAX_FURNITURE_BARRIER_HITBOXES) {
    context.diagnostics.error(
      "BARRIER_RANGE_TOO_LARGE",
      "Nexo barrier range exceeds the safe " + MAX_FURNITURE_BARRIER_HITBOXES + "-position limit: " + value,
      detail(context, "Mechanics.furniture.hitbox.barriers"),
    );
    return [];
  }
  const positions: string[] = [];
  for (let x = xs[0]; x <= xs[1]; x++) for (let y = ys[0]; y <= ys[1]; y++) for (let z = zs[0]; z <= zs[1]; z++) {
    positions.push(nexoPosition([x, y, z].join(",")));
  }
  return positions;
}

function hitboxValues(section: JsonObject, singular: string): string[] {
  const primary = asStringList(getValue(section, singular));
  return primary.length > 0 ? primary : asStringList(getValue(section, singular + "s"));
}

function barrierHitbox(position: string): JsonObject {
  return {
    // scale 1 + peek 0 is an exact axis-aligned 1×1×1 hard collider in CE.
    // It intentionally stays an entity-backed collider: CE has no declarative
    // owner-tracked virtual-block hitbox type equivalent to Nexo's packets.
    type: "shulker", position, scale: 1, peek: 0, direction: "up", _nexo_barrier: true,
    interaction_entity: false, interactive: true, blocks_building: true, can_use_item_on: true, can_be_hit_by_projectile: true,
  };
}

function parseLegacyHitbox(value: string, context: Context): JsonObject[] {
  const words = splitWords(value);
  const type = words.at(-1) ?? "";
  const body = words.slice(0, -1).join(" ");
  if (type === "B" || type === "BARRIER") return [barrierHitbox(parseBarrierPosition(body))];
  if (type === "I" || type === "INTERACTION") return [parseInteraction(body, context)];
  if (type === "S" || type === "SHULKER") return [parseShulker(body, context)];
  if (type === "G" || type === "GHAST") return [parseGhast(body, context)];
  return [];
}

function mapHitboxes(rawHitbox: JsonValue | undefined, seats: string[], context: Context): JsonValue[] {
  const result: JsonObject[] = [];
  if (rawHitbox === undefined) {
    // Nexo adds a default 1x1 Interaction only when the hitbox key is absent.
    result.push(parseInteraction("0,0,0", context));
  } else if (isObject(rawHitbox)) {
    for (const value of hitboxValues(rawHitbox, "interaction")) result.push(parseInteraction(value, context));
    for (const value of hitboxValues(rawHitbox, "shulker")) result.push(parseShulker(value, context));
    for (const value of hitboxValues(rawHitbox, "ghast")) result.push(parseGhast(value, context));
    const barrierPositions: string[] = [];
    for (const value of hitboxValues(rawHitbox, "barrier")) {
      const parsed = parseBarrierPositions(value, context);
      if (barrierPositions.length + parsed.length > MAX_FURNITURE_BARRIER_HITBOXES) {
        context.diagnostics.error(
          "BARRIER_COUNT_TOO_LARGE",
          "Combined Nexo barrier positions exceed the safe " + MAX_FURNITURE_BARRIER_HITBOXES + "-position limit",
          detail(context, "Mechanics.furniture.hitbox.barriers"),
        );
        barrierPositions.length = 0;
        break;
      }
      barrierPositions.push(...parsed);
    }
    for (const position of barrierPositions) result.push(barrierHitbox(position));
  } else {
    for (const value of asStringList(rawHitbox)) {
      const parsed = parseLegacyHitbox(value, context);
      result.push(...parsed);
    }
  }
  const seatPositions = seats.map(nexoSeat);
  if (seatPositions.length > 0 && result.length > 0) {
    // CE mounts only seats owned by the hitbox that received the click. Put the
    // same root-relative seats on every converted hitbox so an outer shulker or
    // another visible part cannot hide the mount action. CE deduplicates equal
    // seat positions across hitboxes into one runtime Seat instance.
    for (const hitbox of result) hitbox.seats = [...seatPositions];
  } else {
    // An explicit empty/invalid Nexo hitbox still has its 0.1x0.1 seat entities.
    // Keep tiny CE proxies only for this no-clickable-hitbox fallback case.
    for (const position of seatPositions) {
      result.push({
        type: "interaction", position, width: 0.1, height: 0.1, interactive: true,
        blocks_building: false, can_use_item_on: false, can_be_hit_by_projectile: false,
        _nexo_seat_proxy: true, seats: [position],
      });
    }
  }
  return result;
}

interface FurnitureLightMapping {
  lights: JsonObject[];
  toggleable: boolean;
}

function parseLightLevel(raw: string | undefined, ranged: boolean): number {
  const normalized = raw?.trim() ?? "";
  const parsed = /^-?\d+$/.test(normalized) ? Number(normalized) : 15;
  return Math.max(ranged ? 1 : 0, Math.min(15, parsed));
}

function parseLightPositions(value: string, context: Context): JsonObject[] {
  const [rawPosition, rawLevel] = firstWordAndRest(value);
  const coordinate = rawPosition === "origin" ? "0,0,0" : rawPosition;
  const ranged = coordinate.includes("..");
  const level = parseLightLevel(rawLevel, ranged);
  if (level === 0) return [];
  if (!ranged) return [{ position: parseBarrierPosition(coordinate), level }];
  const parts = splitWithLast(coordinate, ",", 3);
  if (parts.length < 3) {
    context.diagnostics.error("FURNITURE_LIGHT_RANGE_INVALID", "Nexo light range must contain x,y,z coordinates: " + value, detail(context, "Mechanics.furniture.lights.lights"));
    return [];
  }
  const range = (raw: string): [number, number] => {
    const normalized = raw.replaceAll(" ", "");
    const match = /^(-?\d+)\.\.(-?\d+)$/.exec(normalized);
    if (match) return [Number(match[1]), Number(match[2])];
    const point = parseInteger(normalized);
    return [point, point];
  };
  const [xs, ys, zs] = parts.map(range) as [[number, number], [number, number], [number, number]];
  const count = Math.max(0, xs[1] - xs[0] + 1) * Math.max(0, ys[1] - ys[0] + 1) * Math.max(0, zs[1] - zs[0] + 1);
  if (count > 4096) {
    context.diagnostics.error("FURNITURE_LIGHT_RANGE_TOO_LARGE", "Nexo light range expands to more than 4096 positions", detail(context, "Mechanics.furniture.lights.lights"));
    return [];
  }
  const result: JsonObject[] = [];
  for (let x = xs[0]; x <= xs[1]; x++) for (let y = ys[0]; y <= ys[1]; y++) for (let z = zs[0]; z <= zs[1]; z++) {
    result.push({ position: nexoPosition([x, y, z].join(",")), level });
  }
  return result;
}

function mapFurnitureLights(furniture: JsonObject, hitboxes: JsonValue[], context: Context): FurnitureLightMapping | undefined {
  const section = getObject(furniture, "lights");
  if (!section) return undefined;
  const barrierPositions = new Set(hitboxes.filter((entry) => isObject(entry) && entry._nexo_barrier === true).map((entry) => String((entry as JsonObject).position)));
  const parsed = asStringList(getValue(section, "lights")).flatMap((value) => parseLightPositions(value, context));
  const lights = parsed.filter((entry) => !barrierPositions.has(String(entry.position)));
  if (lights.length < parsed.length) {
    context.diagnostics.info("NEXO_LIGHT_BARRIER_OVERLAP_IGNORED", "Nexo ignores light blocks that overlap its barrier hitboxes; the same overlapping lights were omitted", detail(context, "Mechanics.furniture.lights.lights"));
  }
  if (getValue(section, "toggled_model") !== undefined || getValue(section, "toggled_item_model") !== undefined) {
    context.diagnostics.warning("FURNITURE_TOGGLED_LIGHT_MODEL_UNSUPPORTED", "CraftEngine can toggle the light state, but Nexo's alternate toggled display item needs a separately converted item model", detail(context, "Mechanics.furniture.lights", true));
  }
  if (lights.length === 0) return undefined;
  return { lights, toggleable: getBoolean(section, "toggleable", false) };
}

function nestedNumber(section: JsonObject, event: string, property: string, fallback: number): number {
  const literal = getNumber(section, event + "." + property);
  if (literal !== undefined) return literal;
  const nested = getObject(section, event);
  return nested ? getNumber(nested, property) ?? fallback : fallback;
}

function nestedSound(section: JsonObject, event: string): string | undefined {
  return getString(section, event + "_sound") ?? getString(section, event + ".sound") ?? (getObject(section, event) ? getString(getObject(section, event)!, "sound") : undefined);
}

function mapFurnitureSounds(section: JsonObject | undefined, context: Context): JsonObject | undefined {
  if (!section) return undefined;
  const defaults: Record<string, [number, number]> = { place: [1, 0.8], break: [1, 0.8], hit: [0.25, 0.5] };
  const sounds: JsonObject = {};
  for (const event of ["place", "break", "hit"]) {
    const sound = nestedSound(section, event);
    if (!sound) continue;
    const id = normalizeSoundLocation(sound, context.diagnostics, detail(context, "Mechanics.furniture.block_sounds." + event)) ?? sound;
    const fallback = defaults[event] ?? [1, 1];
    sounds[event] = { id, volume: nestedNumber(section, event, "volume", fallback[0]), pitch: nestedNumber(section, event, "pitch", fallback[1]) };
  }
  if (nestedSound(section, "step") || nestedSound(section, "fall")) {
    context.diagnostics.warning("FURNITURE_STEP_FALL_SOUND_UNSUPPORTED", "CraftEngine furniture settings have no equivalent step/fall trigger", detail(context, "Mechanics.furniture.block_sounds", true));
  }
  return Object.keys(sounds).length > 0 ? sounds : undefined;
}

interface PlacementMapping {
  variants: string[];
  rules: JsonObject;
  hasLimited: boolean;
  floor: boolean;
  roof: boolean;
  wall: boolean;
  rotationStep: number;
}

function mapPlacement(furniture: JsonObject, context: Context): PlacementMapping {
  const limited = getObject(furniture, "limited_placing");
  // Nexo 1.26 computes anyRestrictions with nested Bukkit defaults:
  // floor defaults to roof, roof defaults to wall, and wall defaults false.
  // This deliberately preserves edge cases such as floor:false + roof:true,
  // where an unspecified wall still defaults to enabled.
  const anyRestrictions = limited
    ? getBoolean(limited, "floor", getBoolean(limited, "roof", getBoolean(limited, "wall", false)))
    : false;
  const enabled = (key: string): boolean => limited ? getBoolean(limited, key, !anyRestrictions) : true;
  const floor = enabled("floor");
  const roof = enabled("roof");
  const wall = enabled("wall");
  const pairs: Array<[boolean, string]> = [[floor, "ground"], [roof, "ceiling"], [wall, "wall"]];
  const variants = pairs.filter(([allowed]) => allowed).map(([, ce]) => ce);
  const restricted = (getString(furniture, "restricted_rotation") ?? "STRICT").toUpperCase();
  // Nexo 1.26 initially quantizes NONE and STRICT to the same eight Bukkit
  // Rotation values. VERY_STRICT removes the diagonal values.
  const rotation = restricted === "VERY_STRICT" ? "four" : "eight";
  if (!new Set(["VERY_STRICT", "STRICT", "NONE"]).has(restricted)) context.diagnostics.warning("RESTRICTED_ROTATION_UNKNOWN", "Unknown Nexo restricted_rotation; STRICT/eight used", detail(context, "Mechanics.furniture.restricted_rotation", true));
  const rules: JsonObject = {};
  for (const variant of variants) rules[variant] = { rotation, alignment: "center" };
  if (limited && ["type", "block_types", "block_tags", "nexo_blocks", "radius_limitation", "world"].some((key) => getValue(limited, key) !== undefined)) {
    context.diagnostics.warning("LIMITED_PLACING_CONDITIONS_UNSUPPORTED", "Block allow/deny lists, worlds, and radius restrictions need CraftEngine conditions or an API extension", detail(context, "Mechanics.furniture.limited_placing", true));
  }
  // Nexo placement uses 4/8 facings, while a later rotatable click advances
  // by half that placement interval (VERY_STRICT=45°, otherwise 22.5°).
  const rotationStep = restricted === "VERY_STRICT" ? 45 : 22.5;
  return { variants, rules, hasLimited: limited !== undefined, floor, roof, wall, rotationStep };
}

export interface FurnitureRuntimeSettings {
  defaultRotatableOnSneak?: boolean;
  rotationGamemodes?: string[];
}

interface RotatableMapping {
  enabled: boolean;
  onSneak: boolean;
  degree: number;
  conditions: JsonObject[];
}

function mapRotatable(
  furniture: JsonObject,
  placement: PlacementMapping,
  runtime: FurnitureRuntimeSettings | undefined,
): RotatableMapping {
  const raw = getValue(furniture, "rotatable");
  let enabled = false;
  let onSneak = runtime?.defaultRotatableOnSneak ?? false;
  if (typeof raw === "boolean") {
    enabled = raw;
  } else if (isObject(raw)) {
    // Bukkit ConfigurationSection#getBoolean has a false default for both
    // nested keys. Nexo only applies the factory default to scalar booleans.
    enabled = getBoolean(raw, "rotatable", false);
    onSneak = getBoolean(raw, "on_sneak", false);
  }
  if (!enabled) return { enabled: false, onSneak, degree: placement.rotationStep, conditions: [] };

  // Nexo compares the configured strings directly with Bukkit GameMode.name();
  // preserve case and unknown values rather than normalizing them into matches.
  const modes = runtime?.rotationGamemodes ?? ["SURVIVAL", "CREATIVE"];
  const conditions: JsonObject[] = [
    { type: onSneak ? "expression" : "!expression", expression: "<arg:player.is_sneaking>" },
  ];
  // Nexo uses an empty list to disable player rotation in every game mode.
  // An any_of with zero terms means true in CE, so emit an always-false equals.
  const terms = (modes.length > 0 ? modes : ["__NEXO_NO_GAMEMODE__"]).map((mode) => ({
    type: "equals", value1: "<arg:player.gamemode>", value2: mode,
  }));
  conditions.push({ type: "any_of", terms });
  return { enabled: true, onSneak, degree: placement.rotationStep, conditions };
}

function shiftedVector(value: JsonValue | undefined, offset: readonly [number, number, number]): string {
  const original = parseNumberList(value) ?? [0, 0, 0];
  return [0, 1, 2].map((index) => Number(((original[index] ?? 0) + offset[index]!).toFixed(8))).join(",");
}

function shiftedSeat(value: string, offset: readonly [number, number, number]): string {
  const words = splitWords(value);
  const position = shiftedVector(words[0], offset);
  return [position, ...words.slice(1)].join(" ");
}

function omitDefaultHitboxFields(hitbox: JsonObject): void {
  // CraftEngine 26.8 already applies these parser defaults. Keeping only values
  // that differ follows the reference converter and avoids noisy boilerplate.
  for (const key of ["interactive", "blocks_building", "can_use_item_on", "can_be_hit_by_projectile"] as const) {
    if (hitbox[key] === true) delete hitbox[key];
  }
  if (hitbox.invisible === false) delete hitbox.invisible;
  if (hitbox.type === "interaction") {
    if (hitbox.width === 1) delete hitbox.width;
    if (hitbox.height === 1) delete hitbox.height;
  } else if (hitbox.type === "shulker") {
    if (hitbox.scale === 1) delete hitbox.scale;
    if (hitbox.peek === 0) delete hitbox.peek;
    if (hitbox.direction === "up") delete hitbox.direction;
    if (hitbox.interaction_entity === true) delete hitbox.interaction_entity;
  } else if (hitbox.type === "happy_ghast") {
    if (hitbox.scale === 1) delete hitbox.scale;
    if (hitbox.hard_collision === true) delete hitbox.hard_collision;
  }
}

function shiftedHitboxes(
  hitboxes: JsonValue[],
  baseOffset: readonly [number, number, number],
  interactionOffset: readonly [number, number, number],
  barrierOffset: readonly [number, number, number],
  seatEntityOffset: readonly [number, number, number],
  seatPlayerOffset: readonly [number, number, number],
): JsonValue[] {
  return hitboxes.map((raw) => {
    if (!isObject(raw)) return deepClone(raw);
    const hitbox = deepClone(raw);
    const seatProxy = hitbox._nexo_seat_proxy === true;
    const nexoBarrier = hitbox._nexo_barrier === true;
    delete hitbox._nexo_seat_proxy;
    delete hitbox._nexo_barrier;
    const offset = seatProxy ? seatEntityOffset : nexoBarrier ? barrierOffset : hitbox.type === "interaction" ? interactionOffset : baseOffset;
    hitbox.position = shiftedVector(hitbox.position, offset);
    if (Array.isArray(hitbox.seats)) hitbox.seats = hitbox.seats.map((seat) => typeof seat === "string" ? shiftedSeat(seat, seatPlayerOffset) : deepClone(seat));
    omitDefaultHitboxFields(hitbox);
    return hitbox;
  });
}

function shiftedFurnitureLights(
  lights: JsonObject[],
  offset: readonly [number, number, number],
): JsonObject[] {
  return lights.map((rawLight) => {
    const light = deepClone(rawLight);
    light.position = shiftedVector(light.position, offset);
    return light;
  });
}

function isSimpleSelfLoot(drop: JsonObject | undefined, sourceItem: string): boolean {
  if (!drop) return true;
  const raw = getValue(drop, "loots");
  if (!Array.isArray(raw) || raw.length !== 1 || !isObject(raw[0])) return false;
  const entry = raw[0];
  return getString(entry, "nexo_item") === sourceItem && (getNumber(entry, "probability") ?? 1) === 1 && (getNumber(entry, "amount") ?? 1) === 1;
}

function mapFurnitureLoot(drop: JsonObject | undefined, context: Context): JsonObject | undefined {
  if (isSimpleSelfLoot(drop, context.item)) return {
    pools: [{ rolls: 1, entries: [{ type: "furniture_item", item: context.targetId }] }],
  };
  if (drop && Array.isArray(getValue(drop, "loots")) && (getValue(drop, "loots") as JsonValue[]).length === 0) return undefined;
  context.diagnostics.warning("FURNITURE_LOOT_COMPLEX", "Complex Nexo probability/tool/silk-touch loot needs manual CraftEngine loot-table conditions", detail(context, "Mechanics.furniture.drop", true));
  return undefined;
}

function convertFurniture(
  furniture: JsonObject,
  context: Context,
  defaultProperties?: JsonObject,
  runtime?: FurnitureRuntimeSettings,
): { definition: JsonObject; behavior: JsonObject; semantics: JsonObject } {
  const placement = mapPlacement(furniture, context);
  const rotatable = mapRotatable(furniture, placement, runtime);
  const localProperties = getObject(furniture, "properties");
  const properties = defaultProperties ? deepMerge(defaultProperties, localProperties ?? {}) : localProperties;
  const element = mapElement(properties, context);
  const placedItem = getString(furniture, "item");
  if (placedItem) {
    const namespace = context.targetId.slice(0, context.targetId.indexOf(":"));
    element.item = placedItem.includes(":") ? placedItem.toLowerCase() : namespace + ":" + placedItem.toLowerCase();
  }
  if (getString(furniture, "item_model")) {
    context.diagnostics.warning("FURNITURE_ITEM_MODEL_OVERRIDE", "Nexo furniture item_model changes the placed stack independently; CraftEngine needs a dedicated display item for an exact equivalent", detail(context, "Mechanics.furniture.item_model", true));
  }
  const seats = asStringList(getValue(furniture, "seats"));
  const hitboxes = mapHitboxes(getValue(furniture, "hitbox"), seats, context);
  const lightMapping = mapFurnitureLights(furniture, hitboxes, context);
  const variants: JsonObject = {};
  // Light positions follow each explicit ground/ceiling/wall anchor directly.
  const variantOffsets = new Map<string, readonly [number, number, number]>();
  const fixed = element.display_transform === "fixed";
  const scale = configVector(properties ? getValue(properties, "scale") : undefined, fixed ? 0.5 : 1);
  const offsetAgainstBlocks = properties ? getBoolean(properties, "offset_against_blocks", true) : true;
  const translation = configVector(properties ? getValue(properties, "translation") : undefined, 0);
  const recomposeFixedQuarterTurn = fixed && canRecomposeFixedQuarterTurn(properties);
  const hasOrdinaryInteraction = hitboxes.some((entry) => isObject(entry)
    && entry.type === "interaction" && entry._nexo_seat_proxy !== true);
  if (offsetAgainstBlocks && hasOrdinaryInteraction && Math.abs(translation[1]) > 1e-8) {
    context.diagnostics.warning(
      "FURNITURE_INTERACTION_PARTIAL_TRANSLATION_DYNAMIC",
      "Nexo conditionally removes display translation.y from Interaction hitboxes above partial-height support; the concise CraftEngine base variant preserves the ordinary local hitbox offset",
      detail(context, "Mechanics.furniture.properties.translation", true),
    );
  }
  for (const variant of placement.variants) {
    const variantElement = deepClone(element);
    let offset: [number, number, number] = [0, 0, 0];
    let interactionOffset: [number, number, number] = [0, 0, 0];
    let barrierOffset: [number, number, number] = [0, 0, 0];
    if (variant === "ground") {
      // FIXED always uses Nexo's block-center helper on an UP face, even when
      // limited_placing is absent. Other transforms use the block's full center.
      offset = [0, fixed ? 0 : 0.5, 0];
      const pitch = fixed && placement.hasLimited && placement.floor ? -90 : 0;
      const yawHalfTurn = fixed && (!placement.hasLimited || placement.roof);
      if (pitch !== 0 && yawHalfTurn && recomposeFixedQuarterTurn) {
        // Yπ·X(pitch)·M·Yπ = X(-pitch)·(Yπ·M)·Yπ. For Nexo's
        // common M=T_y·S transform this is runtime-identical while remaining
        // correctly oriented in CE tooling instead of being vertically inverted.
        variantElement.pitch = -pitch;
        variantElement.rotation = "0,1,0,0";
      } else {
        if (pitch !== 0) variantElement.pitch = pitch;
        if (yawHalfTurn) variantElement.yaw = -180;
      }
    } else if (variant === "ceiling") {
      offset = [0, placement.hasLimited && placement.roof ? -0.01 : -0.5, 0];
      // A Nexo Barrier is the target block cell. CE's shulker position is its
      // bottom-center, one full block below a ceiling click plane.
      barrierOffset = [0, -1, 0];
      const pitch = fixed && placement.hasLimited && placement.roof ? 90 : 0;
      const yawHalfTurn = fixed && (!placement.hasLimited || placement.roof);
      if (pitch !== 0 && yawHalfTurn && recomposeFixedQuarterTurn) {
        variantElement.pitch = -pitch;
        variantElement.rotation = "0,1,0,0";
      } else {
        if (pitch !== 0) variantElement.pitch = pitch;
        if (yawHalfTurn) variantElement.yaw = -180;
      }
    } else {
      // CE roots wall furniture on the hit plane. Nexo roots it in the target
      // cell, moving a FIXED display toward the wall whenever no solid support
      // exists below; Nexo performs this before offset_against_blocks is checked.
      const wallVisualZ = fixed && placement.hasLimited && placement.wall
        ? Number((0.5 - 0.98 * scale[1]).toFixed(8))
        : 0.5;
      offset = [0, 0, wallVisualZ];
      // Nexo Barrier coordinates are block-cell locations, independent from
      // the ItemDisplay's wall translation. Keep them at target-cell center.
      barrierOffset = [0, -0.5, 0.5];
    }
    // Nexo's packet-backed Interaction origin is the ItemDisplay location minus
    // 0.5Y, plus the display translation component rotated onto world Y. CE's
    // Interaction position is likewise the bottom-center of its AABB.
    const interactionTranslationY = fixed
      ? (variant === "ceiling" ? -translation[2] : translation[2])
      : translation[1];
    interactionOffset = [offset[0], offset[1] - 0.5 + interactionTranslationY, offset[2]];
    if (offset.some((part) => part !== 0)) variantElement.position = shiftedVector(undefined, offset);
    const seatEntityOffset: [number, number, number] = [offset[0], offset[1] + translation[1], offset[2]];
    // Nexo spawns its seat Interaction at the configured Y. CE's BukkitSeat
    // unconditionally adds 0.6 before spawning the vehicle, so subtract exactly
    // 0.6 here to keep the final riding anchor at Nexo's configured height.
    const seatPlayerOffset: [number, number, number] = [offset[0], offset[1] + translation[1] - 0.6, offset[2]];
    if (variant === "ground") barrierOffset = offset;
    variants[variant] = { elements: [variantElement], hitboxes: shiftedHitboxes(hitboxes, offset, interactionOffset, barrierOffset, seatEntityOffset, seatPlayerOffset) };
    // CE glowing positions are furniture-root-relative to this placement anchor.
    variantOffsets.set(variant, offset);
  }
  if (seats.length > 0) {
    const translation = configVector(properties ? getValue(properties, "translation") : undefined, 0);
    if (Math.abs(translation[0]) > 1e-8 || Math.abs(translation[2]) > 1e-8) {
      context.diagnostics.warning("FURNITURE_SEAT_HORIZONTAL_TRANSLATION", "Nexo adds display translation to seats in world axes, which cannot be represented for every rotated CraftEngine placement", detail(context, "Mechanics.furniture.seats", true));
    }
  }
  if (placement.wall && !(fixed && placement.hasLimited && placement.wall)) {
    context.diagnostics.warning("FURNITURE_WALL_YAW_DIFFERENCE", "CraftEngine wall furniture faces the clicked wall, while this Nexo configuration keeps the player-derived yaw", detail(context, "Mechanics.furniture.limited_placing.wall", true));
  }
  // Nexo's support-derived horizontal click is an alternate input path to the
  // same ground/ceiling state. CE reaches that state natively by clicking the
  // UP/DOWN support face, so no extra wall variant (and no lossy warning) is
  // emitted; adding one would create unsupported floating placements.
  if (placement.wall && fixed && !placement.hasLimited) {
    context.diagnostics.warning("FURNITURE_WALL_VERTICAL_OFFSET_DYNAMIC", "Nexo moves unrestricted FIXED wall furniture down by half a block when the target has solid support below; CraftEngine cannot make this vertical position support-dependent", detail(context, "Mechanics.furniture.properties.display_transform", true));
  }
  const settings: JsonObject = { item: context.targetId };
  const sounds = mapFurnitureSounds(getObject(furniture, "block_sounds"), context);
  if (sounds) settings.sounds = sounds;
  const definition: JsonObject = { settings, variants };
  const rightClickFunctions: JsonObject[] = [];
  let toggleableLight = false;
  if (lightMapping) {
    const litVariants: JsonObject = {};
    const originalNames = Object.keys(variants);
    for (const name of originalNames) {
      litVariants[name] = shiftedFurnitureLights(
        lightMapping.lights,
        variantOffsets.get(name) ?? [0, 0, 0],
      );
    }
    definition.behavior = [{ type: "glowing_furniture", variants: litVariants }];
    if (lightMapping.toggleable) {
      toggleableLight = true;
      const cases: JsonObject[] = [];
      for (const name of originalNames) {
        const unlit = name + "_unlit";
        variants[unlit] = deepClone(variants[name]!);
        cases.push(
          { when: name, functions: [{ type: "set_furniture_variant", variant: unlit }] },
          { when: unlit, functions: [{ type: "set_furniture_variant", variant: name }] },
        );
      }
      // Nexo toggles light before deciding whether rotation or sitting wins.
      rightClickFunctions.push({ type: "when", source: "<arg:furniture.variant>", cases });
      if (seats.length === 0) {
        rightClickFunctions.push({ type: "update_interaction_tick" });
      } else {
        // Sneaking never enters Nexo's seat branch; consume that interaction
        // after toggling so CE does not forward the held item to the hitbox.
        rightClickFunctions.push({
          type: "update_interaction_tick",
          conditions: [{ type: "expression", expression: "<arg:player.is_sneaking>" }],
        });
      }
    }
  }
  if (rotatable.enabled) {
    // Nexo treats an allowed rotation as the winning interaction even when the
    // new orientation collides. Mark it handled synchronously before CE starts
    // its collision-aware asynchronous move, and never retry another angle.
    if (!(toggleableLight && seats.length === 0)) {
      rightClickFunctions.push({ type: "update_interaction_tick", conditions: deepClone(rotatable.conditions) });
    }
    rightClickFunctions.push({ type: "rotate_furniture", degree: rotatable.degree, conditions: deepClone(rotatable.conditions) });
  }
  const events: JsonObject[] = [];
  if (rightClickFunctions.length > 0) events.push({ on: "right_click", functions: rightClickFunctions });
  if (events.length > 0) definition.events = events;
  const loot = mapFurnitureLoot(getObject(furniture, "drop"), context);
  if (loot) definition.loot = loot;
  const unsupported = ["storage", "jukebox", "farmland_required", "evolution", "modelengine_id", "clickActions", "blocklocker", "waterloggable", "beds", "door", "states", "connectable", "placements", "light", "text_entities", "text_display"];
  for (const key of unsupported) {
    if (getValue(furniture, key) !== undefined) context.diagnostics.warning("FURNITURE_MECHANIC_UNSUPPORTED", "Nexo furniture mechanic " + key + " requires manual or API migration", detail(context, "Mechanics.furniture." + key, true));
  }
  return {
    definition,
    behavior: { type: "furniture_item", furniture: context.targetId, rules: placement.rules },
    semantics: {
      placements: placement.variants,
      collision_types: hitboxes.map((entry) => isObject(entry) ? entry.type ?? "interaction" : "unknown"),
      ...(lightMapping ? { lights: lightMapping.lights.length, toggleable_light: lightMapping.toggleable } : {}),
      rotatable: rotatable.enabled,
      ...(rotatable.enabled ? { rotation_on_sneak: rotatable.onSneak, rotation_degree: rotatable.degree } : {}),
    },
  };
}

function mapBlockSounds(section: JsonObject | undefined, context: Context): JsonObject | undefined {
  if (!section) return undefined;
  const defaults: Record<string, [number, number]> = { place: [1, 0.8], break: [1, 0.8], hit: [0.25, 0.5], step: [0.15, 1], fall: [0.5, 0.75] };
  const result: JsonObject = {};
  for (const event of Object.keys(defaults)) {
    const sound = nestedSound(section, event);
    if (!sound) continue;
    const fallback = defaults[event] ?? [1, 1];
    const id = normalizeSoundLocation(sound, context.diagnostics, detail(context, "Mechanics.block.block_sounds." + event)) ?? sound;
    result[event] = { id, volume: nestedNumber(section, event, "volume", fallback[0]), pitch: nestedNumber(section, event, "pitch", fallback[1]) };
  }
  return Object.keys(result).length > 0 ? result : undefined;
}

function convertBlock(type: "noteblock" | "stringblock" | "chorusblock", mechanic: JsonObject, baseModel: string | undefined, context: Context): { definition: JsonObject; behavior: JsonObject; semantics: JsonObject } | undefined {
  if (!baseModel) {
    context.diagnostics.error("BLOCK_MODEL_MISSING", "Custom block has no resolvable Pack model; its block definition and block_item behavior were suppressed", detail(context, "Pack.model"));
    return undefined;
  }
  let autoState = type === "noteblock" ? "note_block" : type === "chorusblock" ? "chorus" : "tripwire";
  if (type === "stringblock" && getBoolean(mechanic, "is_tall", false)) {
    autoState = "tripwire";
    context.diagnostics.warning("STRINGBLOCK_TALL_MANUAL", "Nexo tall stringblock placement spans states/blocks and cannot be recreated by copying custom_variation", detail(context, "Mechanics.stringblock.is_tall", true));
  }
  const definition: JsonObject = { state: { auto_state: autoState, model: baseModel } };
  const settings: JsonObject = {};
  const hardness = getNumber(mechanic, "hardness");
  if (hardness !== undefined) settings.hardness = hardness;
  const resistance = getNumber(mechanic, "resistance");
  if (resistance !== undefined) settings.resistance = resistance;
  const sounds = mapBlockSounds(getObject(mechanic, "block_sounds"), context);
  if (sounds) settings.sounds = sounds;
  if (Object.keys(settings).length > 0) definition.settings = settings;
  const drop = getObject(mechanic, "drop");
  if (isSimpleSelfLoot(drop, context.item)) {
    definition.loot = {
      pools: [{
        rolls: 1,
        conditions: [{ type: "survives_explosion" }],
        entries: [{ type: "item", item: context.targetId }],
      }],
    };
  } else {
    context.diagnostics.warning("CUSTOM_BLOCK_DROP_MANUAL", "A non-self Nexo block drop cannot be represented by CraftEngine's default self template; loot was omitted instead of producing an incorrect extra self drop", detail(context, "Mechanics." + type + ".drop", true));
  }
  if (getValue(mechanic, "custom_variation") !== undefined) {
    context.diagnostics.info("BLOCK_VARIATION_REALLOCATED", "Nexo custom_variation was intentionally not copied; CraftEngine allocates carrier states independently", detail(context, "Mechanics." + type + ".custom_variation"));
  }
  for (const key of ["directional", "farmblock", "light", "tall", "breaking", "clickActions"]) {
    if (getValue(mechanic, key) !== undefined) context.diagnostics.warning("CUSTOM_BLOCK_FEATURE_MANUAL", "Nexo custom-block feature " + key + " needs explicit CraftEngine reconstruction", detail(context, "Mechanics." + type + "." + key, true));
  }
  return {
    definition,
    behavior: { type: "block_item", block: context.targetId },
    semantics: { carrier: autoState, nexo_variation_copied: false },
  };
}

export function convertMechanics(
  config: JsonObject,
  targetId: string,
  baseModel: string | undefined,
  diagnostics: DiagnosticBag,
  source: string,
  item: string,
  furnitureDefaultProperties?: JsonObject,
  furnitureRuntime?: FurnitureRuntimeSettings,
): MechanicsConversion {
  const mechanics = getObject(config, "Mechanics");
  const result: MechanicsConversion = { behavior: [], semantics: {} };
  if (!mechanics) return result;
  const context: Context = { source, item, targetId, diagnostics };
  const furniture = getObject(mechanics, "furniture");
  if (furniture) {
    const converted = convertFurniture(furniture, context, furnitureDefaultProperties, furnitureRuntime);
    result.furniture = converted.definition;
    result.behavior.push(converted.behavior);
    result.semantics.furniture = converted.semantics;
  }
  const blockTypes: Array<"noteblock" | "stringblock" | "chorusblock"> = ["noteblock", "stringblock", "chorusblock"];
  const present = blockTypes.filter((type) => getObject(mechanics, type) !== undefined);
  if (present.length > 1) diagnostics.error("MULTIPLE_CUSTOM_BLOCK_TYPES", "An item has more than one Nexo custom block carrier mechanic", detail(context, "Mechanics"));
  const type = present[0];
  if (type) {
    const converted = convertBlock(type, getObject(mechanics, type)!, baseModel, context);
    if (converted) {
      result.block = converted.definition;
      result.behavior.push(converted.behavior);
      result.semantics.block = converted.semantics;
    }
  }
  const known = new Set(["furniture", "noteblock", "stringblock", "chorusblock"]);
  for (const key of Object.keys(mechanics)) {
    if (!known.has(key.toLowerCase())) diagnostics.warning("ITEM_MECHANIC_UNSUPPORTED", "Nexo item mechanic " + key + " was not converted", detail(context, "Mechanics." + key, true));
  }
  return result;
}
