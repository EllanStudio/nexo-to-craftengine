//! Machine-readable migration diagnostics.
//!
//! Port of `legacy/src/diagnostics.ts`. Fields that cannot be represented
//! equivalently in CraftEngine are never guessed: the converter omits the
//! erroneous output and records a `lossy` diagnostic instead.

use std::collections::HashSet;

use serde::Serialize;

use crate::json::JsonObject;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default)]
    pub lossy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<JsonObject>,
}

/// Optional diagnostic details, mirroring the TS `Omit<Diagnostic, ...>` bag.
#[derive(Debug, Clone, Default)]
pub struct Details {
    pub source: Option<String>,
    pub item: Option<String>,
    pub field: Option<String>,
    pub lossy: bool,
    pub context: Option<JsonObject>,
}

impl Details {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn item(mut self, item: impl Into<String>) -> Self {
        self.item = Some(item.into());
        self
    }

    pub fn field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    pub fn lossy(mut self) -> Self {
        self.lossy = true;
        self
    }

    pub fn context(mut self, context: JsonObject) -> Self {
        self.context = Some(context);
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub info: usize,
    pub warning: usize,
    pub error: usize,
    pub lossy: usize,
}

#[derive(Debug, Default)]
pub struct DiagnosticBag {
    pub items: Vec<Diagnostic>,
    seen: HashSet<String>,
}

impl DiagnosticBag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, diagnostic: Diagnostic) {
        let signature = serde_json::to_string(&diagnostic).unwrap_or_default();
        if !self.seen.insert(signature) {
            return;
        }
        self.items.push(diagnostic);
    }

    fn push(&mut self, severity: Severity, code: &str, message: &str, details: Details) {
        self.add(Diagnostic {
            severity,
            code: code.to_string(),
            message: message.to_string(),
            source: details.source,
            item: details.item,
            field: details.field,
            lossy: details.lossy,
            context: details.context,
        });
    }

    pub fn info(&mut self, code: &str, message: &str, details: Details) {
        self.push(Severity::Info, code, message, details);
    }

    pub fn warning(&mut self, code: &str, message: &str, details: Details) {
        self.push(Severity::Warning, code, message, details);
    }

    pub fn error(&mut self, code: &str, message: &str, details: Details) {
        self.push(Severity::Error, code, message, details);
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|item| item.severity == Severity::Error)
    }

    pub fn has_lossy(&self) -> bool {
        self.items.iter().any(|item| item.lossy)
    }

    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for item in &self.items {
            match item.severity {
                Severity::Info => counts.info += 1,
                Severity::Warning => counts.warning += 1,
                Severity::Error => counts.error += 1,
            }
            if item.lossy {
                counts.lossy += 1;
            }
        }
        counts
    }

    pub fn format_lines(&self) -> Vec<String> {
        self.items
            .iter()
            .map(|item| {
                let where_parts: Vec<&str> = [&item.source, &item.item, &item.field]
                    .iter()
                    .filter_map(|part| part.as_deref())
                    .collect();
                let suffix = if where_parts.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", where_parts.join("/"))
                };
                let severity = match item.severity {
                    Severity::Info => "INFO",
                    Severity::Warning => "WARNING",
                    Severity::Error => "ERROR",
                };
                format!(
                    "[{}{}] {}: {}{}",
                    severity,
                    if item.lossy { " LOSSY" } else { "" },
                    item.code,
                    item.message,
                    suffix
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupes_identical_diagnostics() {
        let mut bag = DiagnosticBag::new();
        bag.warning("X", "same", Details::new().item("a"));
        bag.warning("X", "same", Details::new().item("a"));
        bag.warning("X", "other", Details::new().item("a"));
        assert_eq!(bag.items.len(), 2);
    }

    #[test]
    fn counts_and_flags() {
        let mut bag = DiagnosticBag::new();
        bag.info("I", "i", Details::new());
        bag.warning("W", "w", Details::new().lossy());
        bag.error("E", "e", Details::new());
        let counts = bag.counts();
        assert_eq!((counts.info, counts.warning, counts.error, counts.lossy), (1, 1, 1, 1));
        assert!(bag.has_errors());
        assert!(bag.has_lossy());
    }

    #[test]
    fn format_lines_matches_legacy_shape() {
        let mut bag = DiagnosticBag::new();
        bag.warning(
            "ITEM_ID_NORMALIZED",
            "Item id normalized from A B to a_b",
            Details::new().source("items.yml").item("a_b").lossy(),
        );
        assert_eq!(
            bag.format_lines(),
            vec!["[WARNING LOSSY] ITEM_ID_NORMALIZED: Item id normalized from A B to a_b (items.yml/a_b)".to_string()]
        );
    }
}
