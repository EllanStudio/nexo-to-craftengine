//! Author namespace inference.
//!
//! Port of `legacy/src/source-namespace.ts`. The converter trusts, in
//! descending order: ItemsAdder contents/<namespace>, MythicMobs
//! packs/<namespace>, shared tokens in Nexo item configuration filenames,
//! the single Nexo author item directory, and finally a conservative
//! fallback chosen by the caller.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use crate::resource_location::validate_namespace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceInference {
    pub namespace: String,
    pub evidence: String,
    pub candidates: Vec<String>,
}

static YAML_EXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.ya?ml$").unwrap());
static TEMPLATE_DIR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(^|/)templates?(/|$)").unwrap());
static ITEM_DIR_SEGMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:^|/)(?:items?|item)/([^/]+)/").unwrap());
static ITEMSADDER_NS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|/)itemsadder/contents/([^/]+)/(?:configs|resourcepack)(?:/|$)").unwrap());
static MYTHIC_NS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:^|/)mythicmobs/packs/([^/]+)/").unwrap());
static NEXO_ITEM_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:^|/)(?:items?|item)/.+\.ya?ml$").unwrap());
static TRAILING_ROOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\./$|/$").unwrap());

const GENERIC_BASENAMES: &[&str] = &[
    "item",
    "items",
    "config",
    "configs",
    "categories",
    "template",
    "templates",
];

fn normalize_namespace(raw: &str) -> Option<String> {
    let nfkc: String = raw.nfkc().collect();
    let trimmed = nfkc.trim().to_lowercase();
    let without_ext = Regex::new(r"(?i)\.ya?ml$").unwrap().replace(&trimmed, "").to_string();
    let replaced = Regex::new(r"[^a-z0-9_.-]+").unwrap().replace_all(&without_ext, "_").to_string();
    let collapsed = Regex::new(r"_+").unwrap().replace_all(&replaced, "_").to_string();
    let normalized = collapsed.trim_matches(|c| matches!(c, '_' | '-' | '.')).to_string();
    if !normalized.is_empty() && validate_namespace(&normalized) {
        Some(normalized)
    } else {
        None
    }
}

fn token_subsequence(needle: &[String], haystack: &[String]) -> bool {
    let mut index = 0;
    for token in haystack {
        if index < needle.len() && token == &needle[index] {
            index += 1;
        }
    }
    index == needle.len()
}

fn infer_from_nexo_item_paths(paths: &[String]) -> Option<NamespaceInference> {
    let normalized_paths: Vec<String> = paths.iter().map(|path| path.replace('\\', "/")).collect();

    let mut basenames: Vec<String> = Vec::new();
    for path in &normalized_paths {
        if !YAML_EXT.is_match(path) || TEMPLATE_DIR.is_match(path) {
            continue;
        }
        let basename = path.rsplit('/').next().unwrap_or(path);
        let Some(namespace) = normalize_namespace(basename) else { continue };
        if GENERIC_BASENAMES.contains(&namespace.as_str()) || basenames.contains(&namespace) {
            continue;
        }
        basenames.push(namespace);
    }

    if basenames.len() == 1 {
        return Some(NamespaceInference {
            namespace: basenames[0].clone(),
            evidence: "Nexo item configuration filename".to_string(),
            candidates: basenames,
        });
    }
    if basenames.len() > 1 {
        let tokenized: HashMap<&str, Vec<String>> = basenames
            .iter()
            .map(|value| (value.as_str(), value.split('_').filter(|token| !token.is_empty()).map(String::from).collect()))
            .collect();
        let mut universal: Vec<&String> = basenames
            .iter()
            .filter(|candidate| {
                let tokens = &tokenized[candidate.as_str()];
                tokens.len() >= 2
                    && basenames
                        .iter()
                        .all(|other| token_subsequence(tokens, &tokenized[other.as_str()]))
            })
            .collect();
        universal.sort_by(|left, right| {
            let left_len = tokenized[left.as_str()].len();
            let right_len = tokenized[right.as_str()].len();
            right_len.cmp(&left_len).then_with(|| left.cmp(right))
        });
        if universal.len() == 1 {
            return Some(NamespaceInference {
                namespace: universal[0].clone(),
                evidence: "shared author name in Nexo item configuration filenames".to_string(),
                candidates: basenames,
            });
        }
    }

    let mut item_directories: Vec<String> = Vec::new();
    for path in &normalized_paths {
        if let Some(captures) = ITEM_DIR_SEGMENT.captures(path) {
            if let Some(candidate) = normalize_namespace(&captures[1]) {
                if !matches!(candidate.as_str(), "template" | "templates") && !item_directories.contains(&candidate) {
                    item_directories.push(candidate);
                }
            }
        }
    }
    if item_directories.len() == 1 {
        return Some(NamespaceInference {
            namespace: item_directories.into_iter().next().unwrap(),
            evidence: "Nexo author item directory".to_string(),
            candidates: basenames,
        });
    }
    None
}

pub fn infer_author_namespace_from_bundle_paths(paths: &[String], nexo_root: &str) -> Option<NamespaceInference> {
    let normalized_root = TRAILING_ROOT
        .replace(&nexo_root.replace('\\', "/"), "")
        .to_string();
    let normalized_paths: Vec<String> = paths
        .iter()
        .map(|path| path.replace('\\', "/").strip_prefix("./").map(str::to_string).unwrap_or_else(|| path.replace('\\', "/")))
        .collect();

    let mut explicit: Vec<(String, Vec<String>)> = Vec::new();
    let mut add = |raw: &str, evidence_source: &str| {
        if let Some(namespace) = normalize_namespace(raw) {
            match explicit.iter_mut().find(|(name, _)| name == &namespace) {
                Some((_, sources)) => {
                    if !sources.contains(&evidence_source.to_string()) {
                        sources.push(evidence_source.to_string());
                    }
                }
                None => explicit.push((namespace, vec![evidence_source.to_string()])),
            }
        }
    };
    for path in &normalized_paths {
        if let Some(captures) = ITEMSADDER_NS.captures(path) {
            add(&captures[1], "ItemsAdder contents namespace");
        }
        if let Some(captures) = MYTHIC_NS.captures(path) {
            add(&captures[1], "MythicMobs pack namespace");
        }
    }

    let root_prefix = if !normalized_root.is_empty() && normalized_root != "." {
        format!("{}/", normalized_root)
    } else {
        String::new()
    };
    let nexo_item_paths: Vec<String> = normalized_paths
        .iter()
        .filter(|path| path.starts_with(&root_prefix) && NEXO_ITEM_FILE.is_match(path))
        .cloned()
        .collect();
    let nexo_inference = infer_from_nexo_item_paths(&nexo_item_paths);

    if explicit.len() == 1 {
        let (namespace, mut sources) = explicit.into_iter().next().unwrap();
        sources.sort();
        return Some(NamespaceInference {
            candidates: vec![namespace.clone()],
            namespace,
            evidence: sources.join(" + "),
        });
    }
    if explicit.len() > 1 {
        if let Some(inference) = &nexo_inference {
            if let Some((_, sources)) = explicit.iter().find(|(name, _)| name == &inference.namespace) {
                let mut sorted = sources.clone();
                sorted.sort();
                let mut candidates: Vec<String> = explicit.iter().map(|(name, _)| name.clone()).collect();
                candidates.sort();
                return Some(NamespaceInference {
                    namespace: inference.namespace.clone(),
                    evidence: format!("{} + {}", sorted.join(" + "), inference.evidence),
                    candidates,
                });
            }
        }
    }
    nexo_inference
}

pub fn infer_author_namespace_from_nexo_files(nexo_root: &Path, files: &[std::path::PathBuf]) -> Option<NamespaceInference> {
    let relative: Vec<String> = files
        .iter()
        .map(|file| {
            pathdiff::diff_paths(file, nexo_root)
                .unwrap_or_else(|| file.clone())
                .to_string_lossy()
                .to_string()
        })
        .collect();
    infer_from_nexo_item_paths(&relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_namespace_cleans_raw_names() {
        assert_eq!(normalize_namespace("My Pack.yml").as_deref(), Some("my_pack"));
        assert_eq!(normalize_namespace("__author__").as_deref(), Some("author"));
        // TS replaces invalid runs (including "/") with "_" before validating.
        assert_eq!(normalize_namespace("Bad/Name").as_deref(), Some("bad_name"));
        assert_eq!(normalize_namespace("..."), None);
    }

    #[test]
    fn single_item_filename_wins() {
        let inference = infer_from_nexo_item_paths(&["items/authorpack.yml".to_string()]).unwrap();
        assert_eq!(inference.namespace, "authorpack");
        assert_eq!(inference.evidence, "Nexo item configuration filename");
    }

    #[test]
    fn shared_token_subsequence_wins_for_multiple_files() {
        // TS requires one filename's FULL token list to be a subsequence of
        // every other filename; sibling tokens alone (author_swords vs
        // author_tools) infer nothing.
        let inference = infer_from_nexo_item_paths(&[
            "items/author_pack.yml".to_string(),
            "items/author_pack_deluxe.yml".to_string(),
        ])
        .unwrap();
        assert_eq!(inference.namespace, "author_pack");
        assert_eq!(inference.evidence, "shared author name in Nexo item configuration filenames");
        assert!(infer_from_nexo_item_paths(&[
            "items/author_swords.yml".to_string(),
            "items/author_tools.yml".to_string(),
        ])
        .is_none());
    }

    #[test]
    fn itemsadder_contents_namespace_beats_filenames() {
        let inference = infer_author_namespace_from_bundle_paths(
            &[
                "pack/itemsadder/contents/myns/configs/x.yml".to_string(),
                "pack/items/other.yml".to_string(),
            ],
            "pack",
        )
        .unwrap();
        assert_eq!(inference.namespace, "myns");
        assert!(inference.evidence.contains("ItemsAdder"));
    }

    #[test]
    fn author_item_directory_fallback() {
        let inference = infer_from_nexo_item_paths(&["items/author/items.yml".to_string()]).unwrap();
        assert_eq!(inference.namespace, "author");
        assert_eq!(inference.evidence, "Nexo author item directory");
    }
}
