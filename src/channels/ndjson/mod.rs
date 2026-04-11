//! NDJSON CLI channel — subprocess-friendly stdin/stdout protocol.
//!
//! See the design spec at `docs/superpowers/specs/2026-04-11-ndjson-cli-mode-design.md`.

mod types;

pub use types::{CompatMode, EventFilter};

#[cfg(test)]
mod tests {
    #[test]
    fn module_compiles() {}
}
