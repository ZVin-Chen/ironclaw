//! NDJSON CLI channel — subprocess-friendly stdin/stdout protocol.
//!
//! See the design spec at `docs/superpowers/specs/2026-04-11-ndjson-cli-mode-design.md`.

mod approval;
#[allow(unused_imports)]
pub(crate) use approval::ApprovalState;
mod compat;
mod session;
mod types;

pub use types::{CompatMode, EventFilter};

#[cfg(test)]
mod tests {
    #[test]
    fn module_compiles() {}
}
