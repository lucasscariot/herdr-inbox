//! Everything that talks to a Herdr server.

pub mod api;
pub mod events;
pub mod socket;
pub mod ssh;
pub mod terminal;
pub mod transport;
pub mod types;

pub use api::{Api, ApiError};
pub use socket::Endpoint;
