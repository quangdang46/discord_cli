//! Command modules. One file per verb/group (plan §15).
//!
//! Registered from `main.rs`; each `Cmd` variant maps to a function that
//! takes a shared context (token flag + output format) and returns `ExitCode`.

pub mod dc;
pub mod download;
pub mod fetchlinks;
pub mod local;
pub mod sync;
pub mod tail;
