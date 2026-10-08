//! Herdr Inbox: one keyboard-first inbox for every coding agent on every
//! machine, on top of a Herdr server.

pub mod app;
pub mod cli;
pub mod git;
pub mod herdr;
pub mod keys;
pub mod link;
pub mod machines;
pub mod runtime;
pub mod screen;
pub mod theme;
pub mod threads;
pub mod ui;

#[cfg(test)]
pub(crate) mod testing;
