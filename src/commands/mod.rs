//! The commands: the loops that wire the layers together.
//!
//! A command gets its options from `cli`, opens a source and an output,
//! and passes what the source delivers through the business code in
//! `domain`. The full-screen terminal UI of the debug commands and the
//! handling of signals live here too.

pub mod all;
pub mod clear;
pub mod dump;
pub mod filter;
pub mod flipdot;
pub mod live;
pub mod pattern;
pub mod pi;
pub mod record;
pub mod shutdown;
pub mod tui;
