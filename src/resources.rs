//! Resource-pack discovery, copying and language file generation.
//!
//! Port of `legacy/src/resources.ts`.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::diagnostics::{Details, DiagnosticBag};
use crate::json::JsonObject;

pub fn find_resource_pack_root(input: &Path) -> Option<PathBuf> {
    for candidate in [input.join("pack"), input.join("resourcepack"), input.to_path_buf()] {
        if candidate.join("assets").exists() {
            return Some(candidate);
        }
    }
    None
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::read(left), fs::read(right)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Copy a Nexo resource pack into the CraftEngine output tree.
///
/// `.bbmodel` files are relocated under `blueprint_root/<namespace>/...`
/// when a blueprint root is given. Returns the number of copied files.
pub fn copy_resource_pack(
    source_root: &Path,
    output_root: &Path,
    diagnostics: &mut DiagnosticBag,
    blueprint_root: Option<&Path>,
) -> std::io::Result<usize> {
    let mut copied = 0usize;

    fn visit(
        directory: &Path,
        source_root: &Path,
        output_root: &Path,
        blueprint_root: Option<&Path>,
        diagnostics: &mut DiagnosticBag,
        copied: &mut usize,
    ) -> std::io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let source = entry.path();
            let file_type = entry.file_type()?;
            let relative_path = source.strip_prefix(source_root).unwrap_or(&source);
            let relative_slash = relative_path.to_string_lossy().replace('\\', "/");

            if relative_slash.eq_ignore_ascii_case("pack.mcmeta") {
                diagnostics.info(
                    "PACK_MCMETA_SKIPPED",
                    "CraftEngine generates versioned pack.mcmeta; the Nexo file was not copied",
                    Details::new().source(source.display().to_string()),
                );
                continue;
            }
            let target = output_root.join(relative_path);

            if file_type.is_symlink() {
                diagnostics.warning(
                    "RESOURCE_SYMLINK_SKIPPED",
                    "Resource-pack symbolic link was skipped",
                    Details::new().source(source.display().to_string()).lossy(),
                );
                continue;
            }
            if file_type.is_dir() {
                visit(&source, source_root, output_root, blueprint_root, diagnostics, copied)?;
                continue;
            }
            if !file_type.is_file() {
                continue;
            }

            if relative_slash.chars().any(|c| c.is_ascii_uppercase()) {
                diagnostics.warning(
                    "RESOURCE_PATH_UPPERCASE",
                    &format!("Minecraft resource paths should be lowercase: {}", relative_slash),
                    Details::new().source(source.display().to_string()).lossy(),
                );
            }

            let mut destination = target;
            if let Some(blueprint_root) = blueprint_root {
                if entry.file_name().to_string_lossy().to_lowercase().ends_with(".bbmodel") {
                    let parts: Vec<&str> = relative_slash.split('/').collect();
                    let assets = parts.iter().rposition(|part| *part == "assets");
                    let Some(assets) = assets.filter(|index| parts.len() >= index + 4) else {
                        diagnostics.error(
                            "BBMODEL_ASSET_PATH_INVALID",
                            "Nexo bbmodel must be below assets/<namespace>/<category>/",
                            Details::new().source(source.display().to_string()).lossy(),
                        );
                        continue;
                    };
                    let namespace = parts[assets + 1];
                    destination = blueprint_root.join(namespace).join(parts[assets + 3..].join("/"));
                }
            }

            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            if destination.exists() {
                if !same_file(&source, &destination) {
                    diagnostics.error(
                        "RESOURCE_COPY_CONFLICT",
                        "Different resources map to the same output path",
                        Details::new()
                            .source(source.display().to_string())
                            .field(destination.display().to_string()),
                    );
                }
                continue;
            }
            fs::copy(&source, &destination)?;
            *copied += 1;
        }
        Ok(())
    }

    visit(source_root, source_root, output_root, blueprint_root, diagnostics, &mut copied)?;
    Ok(copied)
}

/// Write Nexo language sets as vanilla lang JSON files under assets/nexo.
pub fn write_language_resources(
    root: &JsonObject,
    output_resource_pack: &Path,
    diagnostics: &mut DiagnosticBag,
    source: &str,
) -> std::io::Result<usize> {
    let global: JsonObject = root
        .get("global")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    let mut count = 0usize;
    for (locale, value) in root {
        if locale.to_lowercase() == "global" {
            continue;
        }
        let Some(Value::Object(entries)) = value else { continue };
        let mut merged = global.clone();
        for (key, entry) in entries {
            merged.insert(key.clone(), entry.clone());
        }
        let file = output_resource_pack
            .join("assets")
            .join("nexo")
            .join("lang")
            .join(format!("{}.json", locale.to_lowercase().replace('-', "_")));
        crate::io::write_json(&file, &merged)?;
        count += 1;
    }
    if !global.is_empty() && count == 0 {
        diagnostics.warning(
            "GLOBAL_LANGUAGE_SCOPE",
            "Nexo global translations need an explicit locale set; no locale file could be generated",
            Details::new().source(source).field("global").lossy(),
        );
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_files_merge_global_scope() {
        let root = json::object(&[
            ("global", &serde_json::json!({ "key.shared": "S" })),
            ("en-US", &serde_json::json!({ "key.en": "E" })),
        ]);
        let dir = std::env::temp_dir().join("nexo2ce-lang-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut diags = DiagnosticBag::new();
        let count = write_language_resources(&root, &dir, &mut diags, "languages.yml").unwrap();
        assert_eq!(count, 1);
        let written = fs::read_to_string(dir.join("assets/nexo/lang/en_us.json")).unwrap();
        assert!(written.contains("key.shared"));
        assert!(written.contains("key.en"));
    }
}

mod json {
    pub fn object(entries: &[(&str, &serde_json::Value)]) -> crate::json::JsonObject {
        entries
            .iter()
            .map(|(key, value)| (key.to_string(), (*value).clone()))
            .collect()
    }
}
