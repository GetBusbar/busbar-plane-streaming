//! The STREAMING plane: what bytes mean, for duplex/live sessions. Voice is ONE dialect inside this
//! plane (DECISIONS #18) — never the plane's name.
//!
//! ## What this crate is
//!
//! The plane's door ([`door`]) and everything it answers with: the Statement tail and the
//! per-generation snapshot, the driver answers for the one-request doors ([`driven`]), and one live
//! session as the driver serves it ([`session_unit`] over the frame pump [`session_pump`]). The
//! dialect readers and writers (OpenAI Realtime, Gemini Live) and the four-layer duplex/session IR
//! they meet in are this crate's own `codec` module (`docs/design/BUSBAR-1.6.0.md` #18/#45); the
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
//! The plane value ([`StreamingPlane`]) is immutable and carries only its configured upstream list.
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
mod session_params;
mod session_pump;
/// The session record the crate's tests read (the door writes none: the kernel writes a session's
/// one line).
#[cfg(test)]
mod session_row;
mod session_unit;
/// The live sessions one open builds, by door (the crate's tests' driver; the door holds `Live`).
#[cfg(test)]
mod sessions;
mod splice;

#[cfg(test)]
mod tests;

// The plane value and its upstreams: the crate's tests' (purity, the plane's meta); a shipped build
// states the plane through its door alone.
#[cfg(test)]
use busbar_contract::ids::LaneId;
#[cfg(test)]
use busbar_contract::plugin::{AbiVersion, Kind, Plugin};

#[cfg(test)]
use claims::Dialect;

/// THE DOOR: this plugin's one public item (BUSBAR-1.6.0.md l.3567, Crate shape).
pub use door::door;

/// One configured upstream this plane may dial or pair a claim against.
///
/// Borrowed for the life of the program, the same way `busbar-plane-llm::Upstream` is: the
/// composition root interns every config-derived key once through
/// the host's word interner (its `Registration`) and hands over names that outlive it.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Upstream {
    /// The priced lane this upstream is reached on.
    pub lane: LaneId,
    /// The host to dial.
    pub host: &'static str,
    /// Which dialect the upstream speaks. Always one of the two duplex dialects this plane can
    /// DIAL — `openai-realtime` or `gemini-live`. Twilio and the one-shot dialects are ingress-only
    /// claims; a unit that arrives on them is routed to one of these two upstreams, never dialed as
    /// one itself.
    pub dialect: Dialect,
}

/// The streaming plane. Voice is one dialect inside it (DECISIONS #18), not its name.
///
/// The one field is a borrowed, immutable list: no cell, no lock, no atomic. The purity test in
/// `tests::purity` asserts that by walking the type rather than by trusting this sentence.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StreamingPlane {
    upstreams: &'static [Upstream],
}

#[cfg(test)]
impl StreamingPlane {
    /// A plane with a configured upstream set.
    #[must_use]
    pub const fn new(upstreams: &'static [Upstream]) -> Self {
        Self { upstreams }
    }

    /// A plane with nothing configured.
    ///
    /// It answers every question the loop asks; its answer to "where does this go" is a destination
    /// the trust unit refuses. That is the honest answer for a plane with no upstream configured —
    /// not a panic, and not a fabricated host.
    pub const EMPTY: Self = Self::new(&[]);

    /// The first configured upstream that speaks a given dialect.
    ///
    /// First match wins, the declaration order the operator wrote. Choosing among several when more
    /// than one qualifies is the trust unit's and the ranking hooks' business, not this plane's.
    #[must_use]
    pub fn upstream_for_dialect(&self, dialect: Dialect) -> Option<&'static Upstream> {
        self.upstreams.iter().find(|u| u.dialect == dialect)
    }
}

#[cfg(test)]
impl Default for StreamingPlane {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[cfg(test)]
impl Plugin for StreamingPlane {
    fn key(&self) -> &'static str {
        <Self as busbar_contract::plane::PlaneMeta>::KEY
    }

    fn kind(&self) -> Kind {
        Kind::Plane
    }

    fn abi(&self) -> AbiVersion {
        AbiVersion(1)
    }
}
