//! YAML/JSON loading and writing with legacy-compatible diagnostics.
//!
//! Port of `legacy/src/io.ts`. The legacy loader parses YAML 1.1 with
//! merge-key support; `serde_yaml` resolves anchors/aliases but leaves
//! `<<` merge keys as literal entries, so `yaml_to_json` applies the
//! merge semantics afterwards (merged keys have lower precedence than the
//! explicit keys of the mapping).

use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::diagnostics::{Details, DiagnosticBag};
use crate::json::JsonObject;

fn yaml_scalar_to_json(key: serde_yaml::Value) -> Option<String> {
    match key {
        serde_yaml::Value::String(text) => Some(text),
        serde_yaml::Value::Number(number) => Some(number.to_string()),
        serde_yaml::Value::Bool(flag) => Some(flag.to_string()),
        serde_yaml::Value::Null => Some("null".to_string()),
        _ => None,
    }
}

fn merge_into(target: &mut JsonObject, merged: &serde_yaml::Mapping, diagnostics: &mut DiagnosticBag, source: &str) {
    for (key, value) in merged {
        let Some(name) = yaml_scalar_to_json(key.clone()) else {
            diagnostics.warning(
                "YAML_KEY_SKIPPED",
                "Non-scalar YAML mapping key was skipped",
                Details::new().source(source),
            );
            continue;
        };
        if !target.contains_key(&name) {
            target.insert(name, yaml_to_json(value.clone(), diagnostics, source));
        }
    }
}

/// Convert a parsed YAML document to JSON, applying YAML merge-key (`<<`)
/// semantics recursively.
pub fn yaml_to_json(value: serde_yaml::Value, diagnostics: &mut DiagnosticBag, source: &str) -> Value {
    match value {
        serde_yaml::Value::Null => Value::Null,
        serde_yaml::Value::Bool(flag) => Value::Bool(flag),
        serde_yaml::Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                Value::from(int)
            } else if let Some(uint) = number.as_u64() {
                Value::from(uint)
            } else {
                match number.as_f64() {
                    Some(float) if float.is_finite() => serde_json::Number::from_f64(float)
                        .map(Value::Number)
                        .unwrap_or(Value::Null),
                    _ => Value::Null,
                }
            }
        }
        serde_yaml::Value::String(text) => Value::String(text),
        serde_yaml::Value::Sequence(entries) => Value::Array(
            entries
                .into_iter()
                .map(|entry| yaml_to_json(entry, diagnostics, source))
                .collect(),
        ),
        serde_yaml::Value::Mapping(mapping) => {
            let mut result = JsonObject::new();
            // Merge keys first: they provide defaults, explicit keys win.
            for (key, value) in &mapping {
                if let serde_yaml::Value::String(name) = key {
                    if name == "<<" {
                        match value {
                            serde_yaml::Value::Mapping(merged) => {
                                merge_into(&mut result, merged, diagnostics, source)
                            }
                            serde_yaml::Value::Sequence(entries) => {
                                for entry in entries {
                                    if let serde_yaml::Value::Mapping(merged) = entry {
                                        merge_into(&mut result, merged, diagnostics, source);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            for (key, value) in mapping {
                if let serde_yaml::Value::String(name) = &key {
                    if name == "<<" {
                        continue;
                    }
                }
                let Some(name) = yaml_scalar_to_json(key) else {
                    diagnostics.warning(
                        "YAML_KEY_SKIPPED",
                        "Non-scalar YAML mapping key was skipped",
                        Details::new().source(source),
                    );
                    continue;
                };
                result.insert(name, yaml_to_json(value, diagnostics, source));
            }
            Value::Object(result)
        }
        serde_yaml::Value::Tagged(tagged) => yaml_to_json(tagged.value, diagnostics, source),
    }
}

pub fn load_yaml(file: &Path, diagnostics: &mut DiagnosticBag) -> Option<Value> {
    let source = file.display().to_string();
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) => {
            diagnostics.error("YAML_READ_FAILED", &error.to_string(), Details::new().source(source));
            return None;
        }
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let document: serde_yaml::Value = match serde_yaml::from_str(text) {
        Ok(document) => document,
        Err(error) => {
            diagnostics.error("YAML_INVALID", &error.to_string(), Details::new().source(source));
            return None;
        }
    };
    Some(yaml_to_json(document, diagnostics, &source))
}

pub fn write_yaml(file: &Path, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_yaml::to_string(value).map_err(|error| std::io::Error::other(error.to_string()))?;
    fs::write(file, text)
}

pub fn load_json(file: &Path, diagnostics: &mut DiagnosticBag) -> Option<Value> {
    let source = file.display().to_string();
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) => {
            diagnostics.error("JSON_INVALID", &error.to_string(), Details::new().source(source));
            return None;
        }
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    match serde_json::from_str(text) {
        Ok(value) => Some(value),
        Err(error) => {
            diagnostics.error("JSON_INVALID", &error.to_string(), Details::new().source(source));
            None
        }
    }
}

pub fn write_json(file: &Path, value: &impl Serialize) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(value).map_err(|error| std::io::Error::other(error.to_string()))?;
    text.push('\n');
    fs::write(file, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn yaml_merge_keys_apply_with_lower_precedence() {
        let yaml = r#"
base: &base
  a: 1
  b: 2
child:
  <<: *base
  b: 3
"#;
        let mut diags = DiagnosticBag::new();
        let document: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let value = yaml_to_json(document, &mut diags, "test");
        assert_eq!(value["child"], json!({ "a": 1, "b": 3 }));
        assert!(!diags.has_errors());
    }

    #[test]
    fn yaml_sequence_merge_keys() {
        let yaml = r#"
one: &one
  a: 1
two: &two
  b: 2
merged:
  <<: [*one, *two]
  c: 3
"#;
        let mut diags = DiagnosticBag::new();
        let document: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let value = yaml_to_json(document, &mut diags, "test");
        assert_eq!(value["merged"], json!({ "a": 1, "b": 2, "c": 3 }));
    }

    #[test]
    fn yaml_bom_is_stripped() {
        let mut diags = DiagnosticBag::new();
        let dir = std::env::temp_dir().join("nexo2ce-io-test");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("bom.yml");
        fs::write(&file, "\u{feff}key: value").unwrap();
        let value = load_yaml(&file, &mut diags).unwrap();
        assert_eq!(value, json!({ "key": "value" }));
    }
}
