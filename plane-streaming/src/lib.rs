//! The STREAMING plane: what bytes mean, for duplex/live sessions. Voice is ONE dialect inside this
//! plane (DECISIONS #18) — never the plane's name.
//!
//! ## What this crate is
//!
//! The plane's door ([`door`]) and everything it answers with: the Statement tail and the
//! per-generation snapshot, the driver answers for the one-request doors ([`driven`]), and one live
//! session as the driver serves it ([`session_unit`] over the frame pump [`session_pump`]). The
//! meter's reads of each dialect (OpenAI Realtime, Gemini Live) and the session config they lock
//! are this crate's own `codec` module (`docs/design/BUSBAR-1.6.0.md` #18/#45); the
//! Twilio Media Streams envelope is [`codec::topology::twilio`]. No wire format is written twice.
//!
//! ## What this crate is not
//!
//! It holds no governance, no breaker, no hook seat and no arithmetic over a metered quantity. Those
//! are units, on the far side of the kernel from a plane. The metering method here returns
//! LOCATORS — the class and the quantity the codec (or this crate's own bookkeeping) already
//! computed — never a price. The routing method returns a plan, never a connection. Nothing here
//! opens a socket, reads a file, or reads a clock other than the one the context hands it.
//!
//! ## The dependency seam, stated honestly
//!
//! Measured with `cargo tree -p busbar-plane-streaming --edges normal --depth 1`, this crate's whole
//! direct closure is `busbar-contract` + `serde` + `serde_json` + `bytes` + `url` — the codecs are
//! this crate's own `codec` module, not a named dependency, and no async runtime (`tokio`,
//! `futures`, `hyper`) is reachable from here.
//!
//! What this crate DOES use is [`codec::ir`] — the plane-4 duplex/session intermediate
//! representation and both dialect codecs — which is pure, sync, and free of any async surface.
//!
//! ## What it holds across calls
//!
//! A door instance holds its live generations; a live session's state (the codec's decode state, the
//! open turn's counters and the session's cumulative units) is a [`session_unit::SessionUnit`].

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod broker;
mod claims;
// `#![deny(missing_docs)]` above is a rule this crate holds itself to; the `codec` module was not
// written under it and is exempt, scoped to exactly this module.
#[allow(missing_docs)]
mod codec;
mod config;
mod door;
mod driven;
mod meta;
/// What an `on_piece` answer owes the reply buffer across re-calls, and the one settle of its buffers.
mod piece;
mod provider;
mod request_unit;
mod session;
mod session_door;
mod session_pump;
mod session_unit;
mod splice;

#[cfg(test)]
mod tests;

/// THE DOOR: this plugin's one public item (BUSBAR-1.6.0.md l.3567, Crate shape).
pub use door::door;
