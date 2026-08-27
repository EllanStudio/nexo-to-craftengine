//! Nexo glyph conversion and glyph-tag rewriting.
//!
//! Port of `legacy/src/glyphs.ts`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

use crate::diagnostics::{Details, DiagnosticBag};
use crate::io::load_yaml;
use crate::json::{as_string_list, get_boolean, get_number, get_string, get_value, JsonObject};
use crate::resource_location::{normalize_location, normalize_texture_location};

/// Default glyph font, mirrored from the TS optional parameter.
pub const DEFAULT_GLYPH_FONT: &str = "nexo:default";
/// Default glyph permission template, mirrored from the TS optional parameter.
pub const DEFAULT_GLYPH_PERMISSION: &str = "nexo.glyphs.<glyphid>";

/// Thrown by the TS implementation when glyph char allocation runs out of
/// Unicode code points; surfaced as an error result in Rust.
#[derive(Debug, thiserror::Error)]
pub enum GlyphError {
    #[error("Unicode code-point space exhausted during glyph allocation")]
    CodePointSpaceExhausted,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlyphEntry {
    pub source_id: String,
    pub target_id: String,
    pub font: String,
    pub texture: Option<String>,
    /// Logical Nexo rows. Reference glyphs deliberately have one logical row.
    pub chars: Vec<String>,
    /// Column count of the underlying bitmap image.
    pub columns: usize,
    /// Zero-based offset into the underlying bitmap image.
    pub start_index: usize,
    pub permission: Option<String>,
}

#[derive(Debug)]
pub struct GlyphConversion {
    pub images: JsonObject,
    pub entries: HashMap<String, GlyphEntry>,
    pub source_files: Vec<String>,
}

#[derive(Debug)]
struct RawGlyph {
    id: String,
    section: JsonObject,
    source: String,
}

/// Node `path.extname` for a bare file name: the suffix from the last dot,
/// or empty when the dot is absent or leads the name (hidden files).
fn ext_name(name: &str) -> &str {
    match name.rfind('.') {
        Some(index) if index > 0 => &name[index..],
        _ => "",
    }
}

/// Node `basename(file, extname(file))`: the file name without its extension.
fn basename_no_ext(path: &Path) -> String {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = ext_name(&name);
    if !ext.is_empty() && name.ends_with(ext) {
        name[..name.len() - ext.len()].to_string()
    } else {
        name
    }
}

/// Collect every YAML file below `directory`, recursively. Unreadable
/// directories are skipped silently, mirroring the TS `readdir` catch.
///
/// The TS sorts with `localeCompare` (ICU collation); Rust has no locale
/// collator, so this approximates it with case-folded lexicographic order on
/// the extension-less basename, then the full path.
fn yaml_files(directory: &Path) -> Vec<PathBuf> {
    fn visit(path: &Path, output: &mut Vec<PathBuf>) {
        let Ok(read_dir) = fs::read_dir(path) else { return; };
        for entry in read_dir.flatten() {
            let child = entry.path();
            let file_type = entry.file_type();
            if file_type.as_ref().map(|t| t.is_dir()).unwrap_or(false) {
                visit(&child, output);
            } else if file_type.as_ref().map(|t| t.is_file()).unwrap_or(false) {
                let name = entry.file_name().to_string_lossy().into_owned();
                let ext = ext_name(&name).to_lowercase();
                if ext == ".yml" || ext == ".yaml" {
                    output.push(child);
                }
            }
        }
    }
    let mut output = Vec::new();
    visit(directory, &mut output);
    output.sort_by(|a, b| {
        let base_a = basename_no_ext(a).to_lowercase();
        let base_b = basename_no_ext(b).to_lowercase();
        base_a
            .cmp(&base_b)
            .then_with(|| {
                a.to_string_lossy().to_lowercase().cmp(&b.to_string_lossy().to_lowercase())
            })
            .then_with(|| a.cmp(b))
    });
    output
}

fn string_code_points(value: &str) -> Vec<u32> {
    value.chars().map(|character| character as u32).collect()
}

fn code_point_count(value: &str) -> usize {
    value.chars().count()
}

fn allocate_chars(rows: f64, columns: f64, used: &mut HashSet<u32>) -> Result<Vec<String>, GlyphError> {
    let total = rows * columns;
    let mut candidate: u32 = 42000;
    let mut values: Vec<char> = Vec::new();
    let mut index = 0.0_f64;
    while index < total {
        while used.contains(&candidate) || (0xD800..=0xDFFF).contains(&candidate) {
            candidate += 1;
        }
        if candidate > 0x10FFFF {
            return Err(GlyphError::CodePointSpaceExhausted);
        }
        used.insert(candidate);
        values.push(char::from_u32(candidate).expect("validated code point"));
        candidate += 1;
        index += 1.0;
    }
    let rows = rows as usize;
    let columns = columns as usize;
    let mut result = Vec::new();
    for row in 0..rows {
        result.push(values[row * columns..(row + 1) * columns].iter().collect::<String>());
    }
    Ok(result)
}

fn grid_columns(chars: &[String]) -> usize {
    chars.first().map(|row| code_point_count(row)).unwrap_or(0)
}

fn coordinate(zero_based_index: i64, columns: usize) -> (i64, i64) {
    let safe = zero_based_index.max(0);
    let columns = (columns as i64).max(1);
    (safe / columns, safe % columns)
}

fn glyph_count(entry: &GlyphEntry) -> usize {
    entry.chars.iter().map(|row| code_point_count(row)).sum()
}

fn entry_aliases(entries: &mut HashMap<String, GlyphEntry>, id: &str, entry: GlyphEntry) {
    entries.insert(id.to_string(), entry.clone());
    entries.insert(id.to_lowercase(), entry);
}

fn configured_permission(section: &JsonObject, id: &str, fallback: Option<&str>) -> Option<String> {
    let value = match get_string(section, "permission") {
        Some(value) => Some(value.to_string()),
        None => fallback.map(|fallback| fallback.replace("<glyphid>", id)),
    };
    value.filter(|value| !value.is_empty())
}

fn glyph_details(glyph: &RawGlyph, field: impl Into<String>) -> Details {
    Details::new().source(glyph.source.clone()).item(glyph.id.clone()).field(field)
}

fn emit_auxiliary_diagnostics(glyph: &RawGlyph, permission: Option<&str>, diagnostics: &mut DiagnosticBag) {
    if permission.is_some() {
        diagnostics.warning(
            "GLYPH_PERMISSION_MANUAL",
            "CraftEngine image tags do not enforce Nexo glyph permission automatically",
            glyph_details(glyph, "permission").lossy(),
        );
    }
    if get_value(&glyph.section, "placeholder").is_some()
        || get_boolean(&glyph.section, "is_emoji", false)
        || get_boolean(&glyph.section, "tabcomplete", false)
    {
        diagnostics.warning(
            "GLYPH_PLACEHOLDER_MANUAL",
            "Nexo glyph placeholder, emoji, and tab-completion behavior needs a CraftEngine emoji/PAPI policy",
            glyph_details(glyph, "placeholder").lossy(),
        );
    }
    if get_value(&glyph.section, "default_shadow_color").is_some() {
        diagnostics.warning(
            "GLYPH_SHADOW_MANUAL",
            "Nexo glyph default shadow color has no image-level CraftEngine equivalent",
            glyph_details(glyph, "default_shadow_color").lossy(),
        );
    }
}

static RANGE_RE: OnceLock<Regex> = OnceLock::new();

fn range_re() -> &'static Regex {
    RANGE_RE.get_or_init(|| Regex::new(r"^-?[0-9]+(?:\.\.-?[0-9]+)?$").expect("static regex"))
}

fn parse_range_text(text: &str) -> Option<(f64, f64)> {
    if !range_re().is_match(text) {
        return None;
    }
    let (first_raw, last_raw) = match text.split_once("..") {
        Some((first, last)) => (first, last),
        None => (text, text),
    };
    let first: f64 = first_raw.parse().ok()?;
    let last: f64 = last_raw.parse().ok()?;
    Some((first, first.max(last)))
}

fn parse_reference_range(value: Option<&Value>) -> Option<(f64, f64)> {
    let text = match value {
        Some(Value::Number(number)) => {
            let truncated = number.as_f64()?.trunc();
            if truncated.abs() >= 1e21 {
                // JS prints such values in exponent notation ("1e+21"), which
                // never matches the range regex.
                return None;
            }
            if truncated == 0.0 {
                // JS String(-0) is "0".
                "0".to_string()
            } else {
                format!("{:.0}", truncated)
            }
        }
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    };
    parse_range_text(&text)
}

/// Serialize a truncated number the way JSON.stringify prints JS integers.
fn truncated_json(value: f64) -> Value {
    let truncated = value.trunc();
    if truncated >= i64::MIN as f64 && truncated < 9223372036854775808.0 {
        json!(truncated as i64)
    } else {
        json!(truncated)
    }
}

fn is_reference(section: &JsonObject) -> bool {
    get_string(section, "reference").map_or(false, |reference| !reference.is_empty())
}

pub fn convert_glyphs(
    source_root: &Path,
    namespace: &str,
    diagnostics: &mut DiagnosticBag,
    default_font: Option<&str>,
    default_permission: Option<&str>,
) -> Result<GlyphConversion, GlyphError> {
    let default_font = default_font.unwrap_or(DEFAULT_GLYPH_FONT);
    let default_permission = default_permission.unwrap_or(DEFAULT_GLYPH_PERMISSION);
    let source_paths = yaml_files(&source_root.join("glyphs"));
    let mut raw: Vec<RawGlyph> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut seen_folded: HashSet<String> = HashSet::new();
    for file in &source_paths {
        let source = file.display().to_string();
        let Some(loaded) = load_yaml(file, diagnostics) else { continue; };
        let Some(loaded) = loaded.as_object() else { continue; };
        for (id, value) in loaded {
            let Some(section) = value.as_object() else { continue; };
            if seen.contains(id) {
                diagnostics.error(
                    "DUPLICATE_GLYPH_ID",
                    &format!("Duplicate Nexo glyph id: {}", id),
                    Details::new().source(source.clone()).item(id.clone()),
                );
            } else if seen_folded.contains(&id.to_lowercase()) {
                diagnostics.error(
                    "DUPLICATE_GLYPH_ID_CASE",
                    &format!("Nexo glyph ids differ only by case and collide in CraftEngine: {}", id),
                    Details::new().source(source.clone()).item(id.clone()),
                );
            }
            seen.insert(id.clone());
            seen_folded.insert(id.to_lowercase());
            raw.push(RawGlyph { id: id.clone(), section: section.clone(), source: source.clone() });
        }
    }

    // CE, Minecraft, and Nexo allocate glyph code points independently inside
    // each font. Resolve fonts before reservation so equal chars in different
    // fonts do not conflict and each font's automatic sequence starts at 42000.
    let mut font_by_glyph: Vec<Option<String>> = vec![None; raw.len()];
    let mut used_by_font: HashMap<String, HashSet<u32>> = HashMap::new();
    let mut owners_by_font: HashMap<String, HashMap<u32, String>> = HashMap::new();
    for (index, glyph) in raw.iter().enumerate() {
        if is_reference(&glyph.section) {
            continue;
        }
        let font_raw = get_string(&glyph.section, "font").unwrap_or(default_font);
        let font = normalize_location(
            font_raw,
            diagnostics,
            &glyph_details(glyph, "font"),
            &[],
            "minecraft",
        )
        .unwrap_or_else(|| default_font.to_string());
        font_by_glyph[index] = Some(font.clone());
        for row in as_string_list(get_value(&glyph.section, "char")) {
            for code in string_code_points(&row) {
                let owners = owners_by_font.entry(font.clone()).or_default();
                if let Some(owner) = owners.get(&code) {
                    diagnostics.error(
                        "GLYPH_CHAR_CONFLICT",
                        &format!("Glyph char is assigned more than once in font {} (also used by {})", font, owner),
                        glyph_details(glyph, "char").lossy(),
                    );
                } else {
                    owners.insert(code, glyph.id.clone());
                }
                used_by_font.entry(font.clone()).or_default().insert(code);
            }
        }
    }

    let mut images: JsonObject = JsonObject::new();
    let mut entries: HashMap<String, GlyphEntry> = HashMap::new();
    let mut references: Vec<usize> = Vec::new();
    for (index, glyph) in raw.iter().enumerate() {
        if is_reference(&glyph.section) {
            references.push(index);
            continue;
        }
        if get_value(&glyph.section, "gif").is_some() {
            diagnostics.warning(
                "ANIMATED_GLYPH_UNSUPPORTED",
                "Nexo animated glyphs use sprite/shader runtime behavior and were not converted",
                glyph_details(glyph, "gif").lossy(),
            );
            continue;
        }

        let font = font_by_glyph[index].clone().unwrap_or_else(|| default_font.to_string());
        let used = used_by_font.entry(font.clone()).or_default();
        let raw_rows = get_number(&glyph.section, "rows").unwrap_or(1.0).trunc();
        let raw_columns = get_number(&glyph.section, "columns").unwrap_or(1.0).trunc();
        if raw_rows <= 0.0 || raw_columns <= 0.0 {
            diagnostics.error(
                "GLYPH_GRID_SIZE_INVALID",
                "Nexo glyph rows and columns must both be positive",
                glyph_details(glyph, "rows").lossy(),
            );
            continue;
        }
        let mut chars = as_string_list(get_value(&glyph.section, "char"));
        if chars.is_empty() {
            chars = allocate_chars(raw_rows, raw_columns, used)?;
        }
        let columns_from_chars = grid_columns(&chars);
        if columns_from_chars == 0 || chars.iter().any(|row| code_point_count(row) != columns_from_chars) {
            diagnostics.error(
                "GLYPH_CHAR_GRID_INVALID",
                "Every Nexo glyph char row must have the same non-zero Unicode code-point width",
                glyph_details(glyph, "char").lossy(),
            );
            continue;
        }

        let texture_raw = get_string(&glyph.section, "texture").unwrap_or("minecraft:required/exit_icon");
        let texture = normalize_texture_location(texture_raw, diagnostics, &glyph_details(glyph, "texture"));
        let target_input = format!("{}:{}", namespace, glyph.id.to_lowercase());
        let target_id = normalize_location(&target_input, diagnostics, &glyph_details(glyph, "id"), &[], "minecraft");
        let (Some(texture), Some(target_id)) = (texture, target_id) else {
            continue;
        };
        let height = get_number(&glyph.section, "height").unwrap_or(8.0).trunc();
        if height <= 0.0 {
            diagnostics.error(
                "GLYPH_HEIGHT_INVALID",
                "Nexo glyph height must be positive for CraftEngine and Minecraft",
                glyph_details(glyph, "height").lossy(),
            );
            continue;
        }
        // Nexo creates the bitmap provider with min(ascent, height).
        let ascent = get_number(&glyph.section, "ascent").unwrap_or(8.0).trunc().min(height);
        let mut image = JsonObject::new();
        image.insert("file".to_string(), Value::String(texture.clone()));
        image.insert("font".to_string(), Value::String(font.clone()));
        image.insert("height".to_string(), truncated_json(height));
        image.insert("ascent".to_string(), truncated_json(ascent));
        image.insert("chars".to_string(), json!(chars));
        images.insert(target_id.clone(), Value::Object(image));
        let permission = configured_permission(&glyph.section, &glyph.id, Some(default_permission));
        let entry = GlyphEntry {
            source_id: glyph.id.clone(),
            target_id,
            font,
            texture: Some(texture),
            chars,
            columns: columns_from_chars,
            start_index: 0,
            permission: permission.clone(),
        };
        entry_aliases(&mut entries, &glyph.id, entry);
        emit_auxiliary_diagnostics(glyph, permission.as_deref(), diagnostics);
    }

    // Resolve references only after ordinary glyphs. Iteration supports reference
    // chains without making YAML file order observable; cycles remain invalid.
    let mut pending = references;
    while !pending.is_empty() {
        let mut next: Vec<usize> = Vec::new();
        let mut progress = false;
        for &index in &pending {
            let glyph = &raw[index];
            let reference = get_string(&glyph.section, "reference").expect("reference glyphs have a non-empty reference");
            let source_entry = entries.get(reference).or_else(|| entries.get(&reference.to_lowercase())).cloned();
            let Some(source_entry) = source_entry else {
                next.push(index);
                continue;
            };
            let range = parse_reference_range(get_value(&glyph.section, "index"));
            let total = glyph_count(&source_entry);
            let Some((first, last)) = range else {
                diagnostics.warning(
                    "GLYPH_REFERENCE_INVALID",
                    "Nexo reference glyph target or index range is invalid",
                    glyph_details(glyph, "index").lossy(),
                );
                progress = true;
                continue;
            };
            if first <= 0.0 || last > total as f64 {
                diagnostics.warning(
                    "GLYPH_REFERENCE_INVALID",
                    "Nexo reference glyph target or index range is invalid",
                    glyph_details(glyph, "index").lossy(),
                );
                progress = true;
                continue;
            }
            let flattened: Vec<char> = source_entry.chars.iter().flat_map(|row| row.chars()).collect();
            let chars = vec![flattened[first as usize - 1..last as usize].iter().collect::<String>()];
            let permission = configured_permission(&glyph.section, &glyph.id, source_entry.permission.as_deref());
            let entry = GlyphEntry {
                source_id: glyph.id.clone(),
                target_id: source_entry.target_id.clone(),
                font: source_entry.font.clone(),
                texture: source_entry.texture.clone(),
                chars,
                columns: source_entry.columns,
                start_index: source_entry.start_index + first as usize - 1,
                permission: permission.clone(),
            };
            entry_aliases(&mut entries, &glyph.id, entry);
            emit_auxiliary_diagnostics(glyph, permission.as_deref(), diagnostics);
            progress = true;
        }
        if next.is_empty() {
            break;
        }
        if !progress {
            for &index in &next {
                let glyph = &raw[index];
                diagnostics.warning(
                    "GLYPH_REFERENCE_INVALID",
                    "Nexo reference glyph target does not exist or forms a reference cycle",
                    glyph_details(glyph, "reference").lossy(),
                );
            }
            break;
        }
        pending = next;
    }

    let source_files = source_paths.iter().map(|path| path.display().to_string()).collect();
    Ok(GlyphConversion { images, entries, source_files })
}

fn image_tag(entry: &GlyphEntry, logical_index: f64, colorable: bool) -> String {
    let total = glyph_count(entry) as f64;
    let local_index = if logical_index >= 1.0 && logical_index <= total { logical_index - 1.0 } else { 0.0 };
    let (row, column) = coordinate(entry.start_index as i64 + local_index as i64, entry.columns);
    let tag = format!("<image:{}:{}:{}>", entry.target_id, row, column);
    if colorable {
        tag
    } else {
        format!("<white>{}</white>", tag)
    }
}

fn full_image_tags(entry: &GlyphEntry, colorable: bool) -> String {
    let mut logical_index = 1.0;
    let mut rows: Vec<String> = Vec::new();
    for row_text in &entry.chars {
        let mut row: Vec<String> = Vec::new();
        for _character in row_text.chars() {
            row.push(image_tag(entry, logical_index, colorable));
            logical_index += 1.0;
        }
        rows.push(row.join("<shift:-1>"));
    }
    rows.join("\n")
}

fn indexed_image_tags(entry: &GlyphEntry, start: f64, end: f64, colorable: bool) -> String {
    // Nexo coerces a descending range to one value and falls back to the first
    // bitmap char for every out-of-range index.
    let final_index = start.max(end);
    let count = final_index - start + 1.0;
    if count > 10_000.0 {
        return image_tag(entry, start, colorable);
    }
    let mut tags: Vec<String> = Vec::new();
    let mut index = start;
    while index <= final_index {
        let mut tag = image_tag(entry, index, colorable);
        if count > 1.0 {
            tag.push_str("<shift:-1>");
        }
        tags.push(tag);
        index += 1.0;
    }
    tags.join("")
}

static GLYPH_TAG_RE: OnceLock<Regex> = OnceLock::new();

fn glyph_tag_re() -> &'static Regex {
    // The TS pattern is `(?<!\\)<(?:glyph|g):([^:>]+)((?::[^>]+)*)>` with /gi; the regex
    // crate cannot express the lookbehind, so the rewrite loop checks the
    // preceding character itself.
    GLYPH_TAG_RE.get_or_init(|| Regex::new(r"(?i)<(?:glyph|g):([^:>]+)((?::[^>]+)*)>").expect("static regex"))
}

fn rewrite_glyph_string(
    text: &str,
    glyphs: &HashMap<String, GlyphEntry>,
    diagnostics: &mut DiagnosticBag,
    source: &str,
    item: &str,
) -> String {
    let re = glyph_tag_re();
    let mut output = String::new();
    let mut last_end = 0usize;
    let mut search_from = 0usize;
    while let Some(captures) = re.captures_at(text, search_from) {
        let whole_match = captures.get(0).expect("whole match");
        // Emulate the `(?<!\\)` lookbehind: a backslash before the tag disables it.
        if text[..whole_match.start()].chars().next_back() == Some('\\') {
            search_from = whole_match.start() + 1;
            continue;
        }
        let whole = whole_match.as_str();
        let raw_id = captures.get(1).expect("group 1").as_str();
        let raw_arguments = captures.get(2).map(|group| group.as_str()).unwrap_or("");
        search_from = whole_match.end();

        let entry = glyphs.get(raw_id).or_else(|| glyphs.get(&raw_id.to_lowercase()));
        let Some(entry) = entry else {
            diagnostics.warning(
                "GLYPH_TAG_UNKNOWN",
                &format!("Nexo glyph tag references an unknown or unsupported glyph: {}", raw_id),
                Details::new().source(source).item(item).field(whole).lossy(),
            );
            continue;
        };
        let arguments_list: Vec<&str> = raw_arguments.split(':').filter(|argument| !argument.is_empty()).collect();
        let colorable = arguments_list.iter().any(|argument| *argument == "c" || *argument == "colorable");
        if arguments_list.iter().any(|argument| *argument == "s" || *argument == "shadow") {
            diagnostics.warning(
                "GLYPH_TAG_SHADOW_MANUAL",
                "Per-use Nexo glyph shadow arguments were omitted",
                Details::new().source(source).item(item).field(whole).lossy(),
            );
        }
        let replacement = match arguments_list.iter().find(|argument| range_re().is_match(argument)) {
            None => full_image_tags(entry, colorable),
            Some(range_text) => {
                let (start, end) = parse_range_text(range_text).expect("regex-validated range");
                if end - start + 1.0 > 10_000.0 {
                    diagnostics.warning(
                        "GLYPH_TAG_RANGE_TOO_LARGE",
                        "Nexo glyph index range is too large to expand safely; only its first index was converted",
                        Details::new().source(source).item(item).field(whole).lossy(),
                    );
                }
                indexed_image_tags(entry, start, end, colorable)
            }
        };
        output.push_str(&text[last_end..whole_match.start()]);
        output.push_str(&replacement);
        last_end = whole_match.end();
    }
    output.push_str(&text[last_end..]);
    output
}

pub fn rewrite_glyph_tags(
    value: &Value,
    glyphs: &HashMap<String, GlyphEntry>,
    diagnostics: &mut DiagnosticBag,
    source: &str,
    item: &str,
) -> Value {
    match value {
        Value::String(text) => Value::String(rewrite_glyph_string(text, glyphs, diagnostics, source, item)),
        Value::Array(entries) => Value::Array(
            entries.iter().map(|entry| rewrite_glyph_tags(entry, glyphs, diagnostics, source, item)).collect(),
        ),
        Value::Object(map) => {
            let mut result = JsonObject::new();
            for (key, entry) in map {
                result.insert(key.clone(), rewrite_glyph_tags(entry, glyphs, diagnostics, source, item));
            }
            Value::Object(result)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn test_root(name: &str) -> PathBuf {
        let count = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("nexo2ce-glyphs-{}-{}-{}", name, std::process::id(), count));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("glyphs")).unwrap();
        dir
    }

    fn write_glyph_file(root: &Path, name: &str, yaml: &str) {
        fs::write(root.join("glyphs").join(name), yaml).unwrap();
    }

    fn entry(target_id: &str, chars: &[&str], columns: usize, start_index: usize) -> GlyphEntry {
        GlyphEntry {
            source_id: target_id.to_string(),
            target_id: target_id.to_string(),
            font: "nexo:default".to_string(),
            texture: None,
            chars: chars.iter().map(|row| row.to_string()).collect(),
            columns,
            start_index,
            permission: None,
        }
    }

    fn codes(bag: &DiagnosticBag) -> Vec<String> {
        bag.items.iter().map(|item| item.code.clone()).collect()
    }

    #[test]
    fn allocate_chars_skips_used_and_surrogates() {
        let mut used: HashSet<u32> = HashSet::new();
        used.insert(42000);
        let chars = allocate_chars(1.0, 2.0, &mut used).unwrap();
        assert_eq!(chars, vec!["\u{A411}\u{A412}"]);

        // Fill everything up to the surrogate block; the next char must jump
        // past U+D800..U+DFFF to U+E000.
        let mut used: HashSet<u32> = (42000..0xD800).collect();
        let chars = allocate_chars(1.0, 1.0, &mut used).unwrap();
        assert_eq!(chars, vec!["\u{E000}"]);
    }

    #[test]
    fn parse_reference_range_matches_ts_semantics() {
        assert_eq!(parse_reference_range(Some(&json!(3.9))), Some((3.0, 3.0)));
        assert_eq!(parse_reference_range(Some(&json!("2..5"))), Some((2.0, 5.0)));
        // Descending ranges coerce to the first value.
        assert_eq!(parse_reference_range(Some(&json!("5..2"))), Some((5.0, 5.0)));
        assert_eq!(parse_reference_range(Some(&json!("-3"))), Some((-3.0, -3.0)));
        assert_eq!(parse_reference_range(Some(&json!("abc"))), None);
        assert_eq!(parse_reference_range(Some(&json!(null))), None);
        assert_eq!(parse_reference_range(None), None);
        // JS prints |n| >= 1e21 in exponent notation, which fails the regex.
        assert_eq!(parse_reference_range(Some(&json!(1e21))), None);
    }

    #[test]
    fn convert_glyphs_builds_images_entries_and_diagnostics() {
        let root = test_root("basic");
        write_glyph_file(&root, "one.yml", "Hello:\n  texture: custom:hello\n  char: x\nWorld:\n  texture: custom:world\n");
        write_glyph_file(
            &root,
            "two.yml",
            "RefHello:\n  reference: Hello\n  index: 1\nAnimated:\n  gif: anim.gif\n  texture: custom:anim\nBadGrid:\n  rows: 0\n  texture: custom:bad\nLowHeight:\n  height: 0\n  texture: custom:low\nShadowed:\n  texture: custom:shadowed\n  char: s\n  default_shadow_color: BLACK\n  placeholder: \":shadow:\"\n",
        );
        let mut diags = DiagnosticBag::new();
        let conversion = convert_glyphs(&root, "ns", &mut diags, None, None).unwrap();

        assert_eq!(conversion.source_files.len(), 2);
        let hello = &conversion.images["ns:hello"];
        assert_eq!(hello["file"], "custom:hello");
        assert_eq!(hello["font"], "nexo:default");
        assert_eq!(hello["height"], 8);
        assert_eq!(hello["ascent"], 8);
        assert_eq!(hello["chars"], json!(["x"]));
        // Auto-allocation starts at code point 42000 (U+A410).
        assert_eq!(conversion.images["ns:world"]["chars"], json!(["\u{A410}"]));
        assert_eq!(conversion.images.len(), 3);

        for alias in ["Hello", "hello", "World", "world", "RefHello", "refhello", "Shadowed", "shadowed"] {
            assert!(conversion.entries.contains_key(alias), "missing alias {}", alias);
        }
        let reference = &conversion.entries["RefHello"];
        assert_eq!(reference.target_id, "ns:hello");
        assert_eq!(reference.chars, vec!["x".to_string()]);
        assert_eq!(reference.columns, 1);
        assert_eq!(reference.start_index, 0);
        assert_eq!(reference.texture.as_deref(), Some("custom:hello"));
        assert_eq!(conversion.entries["Hello"].permission.as_deref(), Some("nexo.glyphs.Hello"));

        let codes = codes(&diags);
        for expected in [
            "ANIMATED_GLYPH_UNSUPPORTED",
            "GLYPH_GRID_SIZE_INVALID",
            "GLYPH_HEIGHT_INVALID",
            "GLYPH_SHADOW_MANUAL",
            "GLYPH_PLACEHOLDER_MANUAL",
            "GLYPH_PERMISSION_MANUAL",
        ] {
            assert!(codes.iter().any(|code| code == expected), "missing diagnostic {}", expected);
        }
        // The fixture's BadGrid/LowHeight glyphs are the only errors expected.
        let errors: Vec<&str> = diags
            .items
            .iter()
            .filter(|item| item.severity == crate::diagnostics::Severity::Error)
            .map(|item| item.code.as_str())
            .collect();
        assert_eq!(errors, vec!["GLYPH_GRID_SIZE_INVALID", "GLYPH_HEIGHT_INVALID"]);
    }

    #[test]
    fn convert_glyphs_reports_duplicates_and_conflicts() {
        let root = test_root("dupes");
        write_glyph_file(&root, "one.yml", "Alpha:\n  texture: t:a\n  char: q\nBeta:\n  texture: t:b\n  char: q\n");
        write_glyph_file(&root, "two.yml", "Alpha:\n  texture: t:a2\n  char: z\nalpha:\n  texture: t:a3\n  char: w\n");
        let mut diags = DiagnosticBag::new();
        let conversion = convert_glyphs(&root, "ns", &mut diags, None, None).unwrap();

        let codes = codes(&diags);
        assert!(codes.iter().any(|code| code == "GLYPH_CHAR_CONFLICT"));
        assert!(codes.iter().any(|code| code == "DUPLICATE_GLYPH_ID"));
        assert!(codes.iter().any(|code| code == "DUPLICATE_GLYPH_ID_CASE"));
        // Later duplicates win the entries map and the image slot.
        assert_eq!(conversion.entries["Alpha"].chars, vec!["z".to_string()]);
        assert_eq!(conversion.entries["alpha"].chars, vec!["w".to_string()]);
        assert_eq!(conversion.images["ns:alpha"]["chars"], json!(["w"]));
    }

    #[test]
    fn convert_glyphs_resolves_reference_ranges_and_cycles() {
        let root = test_root("refs");
        write_glyph_file(
            &root,
            "one.yml",
            "Base:\n  texture: t:base\n  char: ab\nGrid:\n  texture: t:grid\n  char:\n    - ab\n    - cd\nRefRange:\n  reference: Base\n  index: 1..2\nRefGrid:\n  reference: Grid\n  index: 3..4\nRefBadIndex:\n  reference: Base\n  index: 5\nRefZero:\n  reference: Base\n  index: 0\nRefMissing:\n  reference: Nope\nRefCycleA:\n  reference: RefCycleB\nRefCycleB:\n  reference: RefCycleA\n",
        );
        let mut diags = DiagnosticBag::new();
        let conversion = convert_glyphs(&root, "ns", &mut diags, None, None).unwrap();

        let range = &conversion.entries["RefRange"];
        assert_eq!(range.chars, vec!["ab".to_string()]);
        assert_eq!(range.start_index, 0);
        let grid_ref = &conversion.entries["RefGrid"];
        // Reference glyphs deliberately collapse to one logical row.
        assert_eq!(grid_ref.chars, vec!["cd".to_string()]);
        assert_eq!(grid_ref.columns, 2);
        assert_eq!(grid_ref.start_index, 2);
        assert_eq!(grid_ref.target_id, "ns:grid");

        let index_warnings = diags
            .items
            .iter()
            .filter(|item| item.code == "GLYPH_REFERENCE_INVALID" && item.field.as_deref() == Some("index"))
            .count();
        assert_eq!(index_warnings, 2);
        let cycle_warnings = diags
            .items
            .iter()
            .filter(|item| item.code == "GLYPH_REFERENCE_INVALID" && item.field.as_deref() == Some("reference"))
            .count();
        assert_eq!(cycle_warnings, 3);
    }

    #[test]
    fn rewrite_glyph_tags_rewrites_simple_and_colorable_tags() {
        let mut glyphs = HashMap::new();
        entry_aliases(&mut glyphs, "Hello", entry("ns:hello", &["x"], 1, 0));
        let mut diags = DiagnosticBag::new();

        assert_eq!(
            rewrite_glyph_tags(&json!("hi <glyph:Hello>"), &glyphs, &mut diags, "s", "i"),
            json!("hi <white><image:ns:hello:0:0></white>")
        );
        assert_eq!(
            rewrite_glyph_tags(&json!("<g:hello:c>"), &glyphs, &mut diags, "s", "i"),
            json!("<image:ns:hello:0:0>")
        );
        // Escaped tags stay untouched and emit no diagnostics.
        let mut diags2 = DiagnosticBag::new();
        assert_eq!(
            rewrite_glyph_tags(&json!(r"\<glyph:Hello>"), &glyphs, &mut diags2, "s", "i"),
            json!(r"\<glyph:Hello>")
        );
        assert!(diags2.items.is_empty());
    }

    #[test]
    fn rewrite_glyph_tags_unknown_and_shadow_arguments() {
        let mut glyphs = HashMap::new();
        entry_aliases(&mut glyphs, "Hello", entry("ns:hello", &["x"], 1, 0));
        let mut diags = DiagnosticBag::new();
        assert_eq!(
            rewrite_glyph_tags(&json!("<glyph:Nope>"), &glyphs, &mut diags, "s", "i"),
            json!("<glyph:Nope>")
        );
        assert_eq!(diags.items[0].code, "GLYPH_TAG_UNKNOWN");

        let mut diags = DiagnosticBag::new();
        assert_eq!(
            rewrite_glyph_tags(&json!("<glyph:Hello:s>"), &glyphs, &mut diags, "s", "i"),
            json!("<white><image:ns:hello:0:0></white>")
        );
        assert_eq!(diags.items[0].code, "GLYPH_TAG_SHADOW_MANUAL");
    }

    #[test]
    fn rewrite_glyph_tags_expands_ranges_with_bitmap_semantics() {
        let mut glyphs = HashMap::new();
        entry_aliases(&mut glyphs, "Grid", entry("ns:grid", &["ab", "cd"], 2, 0));
        let mut diags = DiagnosticBag::new();

        // Index 3 lands on row 1, column 0 of the bitmap.
        assert_eq!(
            rewrite_glyph_tags(&json!("<glyph:Grid:3>"), &glyphs, &mut diags, "s", "i"),
            json!("<white><image:ns:grid:1:0></white>")
        );
        // Multi-index expansion appends a shift after every tag, even the last.
        assert_eq!(
            rewrite_glyph_tags(&json!("<glyph:Grid:1..2:c>"), &glyphs, &mut diags, "s", "i"),
            json!("<image:ns:grid:0:0><shift:-1><image:ns:grid:0:1><shift:-1>")
        );
        // Out-of-range indexes fall back to the first bitmap char.
        assert_eq!(
            rewrite_glyph_tags(&json!("<glyph:Grid:9>"), &glyphs, &mut diags, "s", "i"),
            json!("<white><image:ns:grid:0:0></white>")
        );
        // Descending ranges coerce to the first value.
        assert_eq!(
            rewrite_glyph_tags(&json!("<glyph:Grid:3..1>"), &glyphs, &mut diags, "s", "i"),
            json!("<white><image:ns:grid:1:0></white>")
        );
    }

    #[test]
    fn rewrite_glyph_tags_full_expansion_and_range_cap() {
        let mut glyphs = HashMap::new();
        entry_aliases(&mut glyphs, "Grid", entry("ns:grid", &["ab", "cd"], 2, 0));
        let mut diags = DiagnosticBag::new();
        assert_eq!(
            rewrite_glyph_tags(&json!("<glyph:Grid:c>"), &glyphs, &mut diags, "s", "i"),
            json!("<image:ns:grid:0:0><shift:-1><image:ns:grid:0:1>\n<image:ns:grid:1:0><shift:-1><image:ns:grid:1:1>")
        );

        let mut diags = DiagnosticBag::new();
        assert_eq!(
            rewrite_glyph_tags(&json!("<glyph:Grid:1..10002>"), &glyphs, &mut diags, "s", "i"),
            json!("<white><image:ns:grid:0:0></white>")
        );
        assert_eq!(diags.items[0].code, "GLYPH_TAG_RANGE_TOO_LARGE");
    }

    #[test]
    fn rewrite_glyph_tags_recurses_through_arrays_and_objects() {
        let mut glyphs = HashMap::new();
        entry_aliases(&mut glyphs, "Hello", entry("ns:hello", &["x"], 1, 0));
        let mut diags = DiagnosticBag::new();
        let value = json!({ "title": "<glyph:Hello>", "list": ["<g:Hello:c>", 5], "nested": { "deep": "<glyph:Hello>" } });
        assert_eq!(
            rewrite_glyph_tags(&value, &glyphs, &mut diags, "s", "i"),
            json!({
                "title": "<white><image:ns:hello:0:0></white>",
                "list": ["<image:ns:hello:0:0>", 5],
                "nested": { "deep": "<white><image:ns:hello:0:0></white>" }
            })
        );
    }
}
