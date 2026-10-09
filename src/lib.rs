//! Herdr Inbox: one keyboard-first inbox for every coding agent on every
//! machine, on top of a Herdr server.

pub mod app;
pub mod cli;
pub mod clipboard;
pub mod config;
pub mod discovery;
pub mod editor;
pub mod fuzzy;
pub mod git;
pub mod herdr;
pub mod hold;
pub mod images;
pub mod keys;
pub mod launch;
pub mod link;
pub mod machines;
pub mod orbit;
pub mod presets;
pub mod runtime;
pub mod screen;
pub mod speech;
pub mod state;
pub mod theme;
pub mod threads;
pub mod ui;
pub mod update;

#[cfg(test)]
pub(crate) mod testing;
