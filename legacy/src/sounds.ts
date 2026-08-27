import type { DiagnosticBag } from "./diagnostics.js";
import { normalizeSoundLocation } from "./resource-location.js";
import { asStringList, getBoolean, getNumber, getObject, getString, getValue, isObject, type JsonObject, type JsonValue } from "./types.js";

export function convertSounds(root: JsonObject, diagnostics: DiagnosticBag, source: string): JsonObject {
  const result: JsonObject = {};
  const raw = getValue(root, "sounds");
  if (!Array.isArray(raw)) {
    if (raw !== undefined) diagnostics.error("SOUNDS_NOT_LIST", "Nexo 1.26 sounds must be a list", { source, field: "sounds" });
    return result;
  }
  for (const entry of raw) {
    if (!isObject(entry)) {
      diagnostics.error("SOUND_ENTRY_INVALID", "Each Nexo sound entry must be a map", { source, field: "sounds" });
      continue;
    }
    const rawId = getString(entry, "id");
    if (!rawId) {
      diagnostics.error("SOUND_ID_MISSING", "Sound entry has no id", { source, field: "sounds.id" });
      continue;
    }
    const id = normalizeSoundLocation(rawId, diagnostics, { source, field: "sounds.id" });
    if (!id) continue;
    const files = [...asStringList(getValue(entry, "sounds"))];
    const single = getString(entry, "sound");
    if (files.length === 0 && single) files.push(single);
    if (files.length === 0) {
      diagnostics.error("SOUND_FILES_MISSING", "Sound event has neither sound nor sounds", { source, field: rawId });
      continue;
    }
    const converted: JsonValue[] = [];
    for (const file of files) {
      const name = normalizeSoundLocation(file, diagnostics, { source, field: rawId + ".sound" });
      if (!name) continue;
      converted.push({
        name,
        stream: getBoolean(entry, "stream", false),
        preload: getBoolean(entry, "preload", false),
        volume: getNumber(entry, "volume") ?? 1,
        pitch: getNumber(entry, "pitch") ?? 1,
        weight: getNumber(entry, "weight") ?? 1,
        attenuation_distance: getNumber(entry, "attenuation_distance") ?? 16,
      });
    }
    result[id] = { sounds: converted };
    if (getObject(entry, "jukebox_playable")) {
      diagnostics.warning("JUKEBOX_SONG_MANUAL", "Nexo jukebox_playable registration needs a separate CraftEngine jukebox song/item migration", { source, field: rawId + ".jukebox_playable", lossy: true });
    }
  }
  return result;
}
