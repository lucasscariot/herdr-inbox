//! Everything that talks to a Herdr server.

pub mod api;
pub mod events;
pub mod socket;
pub mod terminal;
pub mod types;

pub use api::{Api, ApiError};
pub use socket::Endpoint;
