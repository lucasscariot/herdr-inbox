//! Herdr Inbox: one keyboard-first inbox for every coding agent on every
//! machine, on top of a Herdr server.

pub mod app;
pub mod git;
pub mod herdr;
pub mod keys;
pub mod screen;
pub mod theme;
pub mod threads;

#[cfg(test)]
pub(crate) mod testing;
