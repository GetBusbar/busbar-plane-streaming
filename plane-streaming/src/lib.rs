//! The streaming plane: what bytes mean, for duplex/live sessions. Voice is one dialect inside this

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod claims;

#[allow(missing_docs)]
mod codec;
mod config;
mod driven;
mod meta;
/// What an `on_piece` answer owes the reply buffer across re-calls, and the one settle of its buffers.
mod piece;
mod plane_door;
mod provider;
mod request_unit;
mod session;
mod session_door;
mod session_pump;
mod session_unit;
mod splice;
mod ticket;

#[cfg(test)]
mod tests;

/// The door: this plugin's one public item.
pub use plane_door::door;
