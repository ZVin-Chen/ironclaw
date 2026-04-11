//! NDJSON wire protocol types.

use serde::{Deserialize, Serialize};

/// Output wire-format selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatMode {
    /// IronClaw native NDJSON format.
    Ironclaw,
    /// Claude Code compatible format.
    ClaudeCode,
}

/// Optional event categories included in the output stream.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub reasoning: bool,
    pub hooks: bool,
    pub cost: bool,
    pub partial_messages: bool,
}

impl EventFilter {
    pub fn from_names(names: &[String]) -> Self {
        let mut filter = Self::default();
        for name in names {
            match name.trim().to_ascii_lowercase().as_str() {
                "reasoning" => filter.reasoning = true,
                "hooks" => filter.hooks = true,
                "cost" => filter.cost = true,
                "partial_messages" | "partial" => filter.partial_messages = true,
                _ => {}
            }
        }
        filter
    }
}

/// Placeholder — populated in Task 2.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum NdjsonInput {}

/// Placeholder — populated in Task 2.
#[allow(dead_code)]
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum NdjsonOutput {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_filter_parses_names_case_insensitive() {
        let filter = EventFilter::from_names(&[
            "reasoning".into(),
            "Hooks".into(),
            "UNKNOWN".into(),
        ]);
        assert!(filter.reasoning);
        assert!(filter.hooks);
        assert!(!filter.cost);
    }

    #[test]
    fn compat_mode_equality() {
        assert_eq!(CompatMode::Ironclaw, CompatMode::Ironclaw);
        assert_ne!(CompatMode::Ironclaw, CompatMode::ClaudeCode);
    }
}
