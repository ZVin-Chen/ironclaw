//! NDJSON CLI channel — subprocess-friendly stdin/stdout protocol.
//!
//! See the design spec at `docs/superpowers/specs/2026-04-11-ndjson-cli-mode-design.md`.

mod approval;
mod compat;
mod session;
mod stdin_reader;
mod types;

#[allow(unused_imports)]
pub(crate) use approval::ApprovalState;
#[allow(unused_imports)]
pub(crate) use compat::to_claude_code;
#[allow(unused_imports)]
pub(crate) use session::{resolve_session_id, SessionResolveArgs};
pub use types::{CompatMode, EventFilter};

#[cfg(test)]
mod tests {
    #[test]
    fn module_compiles() {}
}
