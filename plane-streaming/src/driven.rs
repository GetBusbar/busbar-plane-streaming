// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The plane's answers to the kernel plane driver: which door a claim is, and the request each
//! session door sends its far end. Each answer is plain data, in the plane's own vocabulary;
//! writing it into the host's buffers is the door's job.
//!
//! | driver crossing | here |
//! |---|---|
//! | `arrive` | [`arrive`]: which door the claim is, its dialect and its principal need |
//! | the mint door's caller body | [`minted`], [`mint_failed`]: the ticket busbar minted, or why not |
//! | a session's far-end turn | [`Door::route`]: the request bound for the far end |
//!
//! Busbar terminates the browser's media (row 132): the mint door answers a ticket busbar mints
//! itself and dials nothing; the SDP door is a datagram session whose far end is busbar's own call.

use busbar_contract::abi::sdk::cred::{Cred, SealedWrite};

use crate::ticket::{minted_answer, CALLS_PATH, CALL_FORM_TYPE, SAFETY_IDENTIFIER_HEADER};

/// The media type of a JSON body.
pub const JSON_CONTENT_TYPE: &str = "application/json";
/// The head field naming a body's media type.
pub const FIELD_CONTENT_TYPE: &str = "content-type";
/// The head field naming where a created call lives.
pub const FIELD_LOCATION: &str = "location";
/// The head field a whole answer states its length in.
pub const FIELD_CONTENT_LENGTH: &str = "content-length";

/// The door a claim is, by its index in the snapshot's claims (`crate::door::ROUTES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    /// `POST /v1/realtime/client_secrets`: one ticket for a browser session, minted by busbar.
    Mint,
    /// `POST /v1/realtime/calls`: one SDP offer, answered by busbar, which terminates the media.
    Sdp,
    /// The browser's sideband socket.
    Sideband,
    /// The Gemini Live socket.
    Gemini,
    /// The telephony socket.
    Twilio,
    /// `GET /v1/realtime`: plain server-to-server OpenAI Realtime over WS, attached to no call
    /// (the sideband is call-attached only).
    Realtime,
}

impl Door {
    /// The door of claim `claim`; `None` for a claim the plane never published.
    #[must_use]
    pub const fn of(claim: u32) -> Option<Door> {
        match claim {
            0 => Some(Door::Mint),
            1 => Some(Door::Sdp),
            2 => Some(Door::Sideband),
            3 => Some(Door::Gemini),
            4 => Some(Door::Twilio),
            5 => Some(Door::Realtime),
            _ => None,
        }
    }

    /// The door's line on the guest list: its claim (`crate::door::ROUTES`), in claim order.
    #[must_use]
    pub const fn line(self) -> crate::door::Route {
        crate::door::ROUTES[self as usize]
    }

    /// The door's dialect, by its index in the tail's dialects: its line's.
    #[must_use]
    pub const fn dialect(self) -> u32 {
        self.line().dialect
    }

    /// `true` for a door that opens a live session rather than answering one request.
    #[must_use]
    pub const fn is_session(self) -> bool {
        matches!(
            self,
            Door::Sdp | Door::Sideband | Door::Gemini | Door::Twilio | Door::Realtime
        )
    }

    /// The outbound need a far request on this door rides (`OnPieceOut::need`): the socket of the
    /// provider the door's session dials (Gemini Live's for the Gemini door, OpenAI Realtime's for
    /// the sideband, telephony and plain realtime doors), and busbar's own call to OpenAI Realtime
    /// for the SDP door (row 132: busbar terminates the browser's media). The mint door dials
    /// nothing (`ROUTE_LOCAL`).
    #[must_use]
    pub const fn rides(self) -> u32 {
        match self {
            Door::Gemini => crate::door::RIDES_LIVE_SOCKET,
            Door::Sdp => crate::door::RIDES_REALTIME_CALL,
            Door::Mint | Door::Sideband | Door::Twilio | Door::Realtime => {
                crate::door::RIDES_REALTIME_SOCKET
            }
        }
    }
}

/// What `arrive` answers for a claimed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arrival {
    /// The door.
    pub door: Door,
    /// Index into the tail's op classes: the session open, every door's one operation.
    pub op_class: u32,
    /// Index into the tail's dialects.
    pub dialect: u32,
}

/// `arrive` for claim `claim`: every keyed door opens a session (or mints the ticket one opens
/// with) for a known principal.
#[must_use]
pub const fn arrive(claim: u32) -> Option<Arrival> {
    match Door::of(claim) {
        Some(door) => Some(Arrival {
            door,
            op_class: 0,
            dialect: door.dialect(),
        }),
        None => None,
    }
}

impl Door {
    /// The request a session's far-end turn on this door sends: the socket doors dial the
    /// dialect's realtime socket (the credential is the kernel's, never in the target); the SDP
    /// door places busbar's own call ([`call_attempt`]), its caller named by `caller_ref` for
    /// attribution; the mint door dials nothing (`None`).
    #[must_use]
    pub fn route(self, caller_ref: Option<&str>) -> Option<Attempt> {
        let socket = |target| Attempt {
            verb: "GET",
            target,
            fields: Vec::new(),
        };
        Some(match self {
            Door::Mint => return None,
            Door::Sdp => call_attempt(caller_ref),
            Door::Sideband | Door::Twilio | Door::Realtime => socket(REALTIME_SOCKET_PATH),
            Door::Gemini => socket(GEMINI_SOCKET_PATH),
        })
    }
}

/// The action a session open is audited under.
#[cfg(test)]
pub const SESSION_OPEN_ACTION: &str = "streaming.session.open";

/// The realtime socket an OpenAI Realtime session dials, under the member's base URL. A sideband
/// session attaches to its provider call there (`?call_id=`, `crate::session_door::Live::dial_target`).
pub const REALTIME_SOCKET_PATH: &str = "/v1/realtime";

/// The sideband door's path, before its `{call_id}` segment.
pub const SIDEBAND_PREFIX: &str = "/v1/realtime/sideband/";

/// Busbar's call a sideband arrival's `target` names: its `{call_id}` segment, less any query or
/// fragment. `None` for a segment that is empty or holds anything but ASCII letters, digits, `_`
/// and `-` (busbar's call ids and the provider's, `rtc_…`): nothing that could carry another
/// parameter into a dialled query is taken.
#[must_use]
pub fn sideband_call_id(target: &str) -> Option<&str> {
    let id = target.strip_prefix(SIDEBAND_PREFIX)?;
    let id = id.split(['?', '#']).next().unwrap_or(id);
    let well_formed = !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    well_formed.then_some(id)
}

/// The socket a Gemini Live session dials, under the member's base URL.
pub const GEMINI_SOCKET_PATH: &str =
    "/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

/// A request head bound for the far end: verb and target explicit, then its head fields. The
/// kernel adds the credential and sends it; the target is under the member's base URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    /// The verb.
    pub verb: &'static str,
    /// The target, under the member's base URL.
    pub target: &'static str,
    /// The head fields, in order.
    pub fields: Vec<(&'static str, String)>,
}

/// The SDP door's far-end request head: busbar's own call to the provider, one form
/// ([`crate::ticket::call_form`]) carrying the locked session and busbar's description, and the
/// caller named for the provider's attribution of the session (`OpenAI-Safety-Identifier`, the
/// kernel's caller reference, never the principal).
#[must_use]
pub fn call_attempt(caller: Option<&str>) -> Attempt {
    let mut fields = vec![(FIELD_CONTENT_TYPE, CALL_FORM_TYPE.to_string())];
    if let Some(r) = caller.filter(|r| !r.is_empty()) {
        fields.push((SAFETY_IDENTIFIER_HEADER, r.to_string()));
    }
    Attempt {
        verb: "POST",
        target: CALLS_PATH,
        fields,
    }
}

/// A caller's answer body: public bytes, or a mint's answer sealed by the SDK (it carries busbar's
/// bearer ticket, which this plane never holds as text and cannot read back: R7).
#[allow(missing_debug_implementations)]
pub enum Body {
    /// Public bytes (a refusal's text).
    Public(Vec<u8>),
    /// The SDK's sealed write: the door moves it into the host's reply buffer
    /// (`SealedWrite::stream_into`).
    Sealed(SealedWrite),
}

impl Body {
    /// The length in bytes (public: the head states it).
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Public(b) => b.len(),
            Self::Sealed(w) => w.len(),
        }
    }

    /// The public bytes; `None` for a sealed answer, which this plane never reads. (Read by the
    /// voice battery, whose overlay lifts this `cfg(test)` fence.)
    #[cfg(test)]
    #[must_use]
    pub fn public(&self) -> Option<&[u8]> {
        match self {
            Self::Public(b) => Some(b),
            Self::Sealed(_) => None,
        }
    }
}

/// What the caller is answered. Its `Debug` prints the body's length, never its bytes: a mint
/// answer's body is the live `ek_` ticket.
pub struct Reply {
    /// The status.
    pub status: u16,
    /// The head fields, in order.
    pub fields: Vec<(&'static str, String)>,
    /// The body.
    pub body: Body,
}

impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            status,
            fields,
            body,
        } = self;
        f.debug_struct("Reply")
            .field("status", status)
            .field("fields", fields)
            .field("body", &format_args!("<redacted {} bytes>", body.len()))
            .finish()
    }
}

/// The status the served plane answers a failed mint with.
pub const MINT_FAILED_STATUS: u16 = 502;

/// The served plane's 502 for a mint that failed for `reason`.
#[must_use]
pub fn mint_failed(reason: &str) -> Reply {
    Reply {
        status: MINT_FAILED_STATUS,
        fields: Vec::new(),
        body: Body::Public(
            format!(
                "streaming ephemeral-secret mint failed: ephemeral token mint failed: {reason}"
            )
            .into_bytes(),
        ),
    }
}

/// Why a minted ticket was not answered: it is not text.
pub const MINTED_NOT_TEXT: &str = "the minted ticket is not text";

/// The caller's answer to a mint busbar answered: the ticket, sealed, and its expiry.
#[must_use]
pub fn minted(ticket: &Cred, expires_at_unix: u64) -> Reply {
    match minted_answer(ticket, expires_at_unix) {
        Ok(answer) => Reply {
            status: 200,
            fields: vec![(FIELD_CONTENT_TYPE, JSON_CONTENT_TYPE.to_string())],
            body: Body::Sealed(answer),
        },
        Err(_) => mint_failed(MINTED_NOT_TEXT),
    }
}

#[cfg(test)]
#[path = "tests/driven_tests.rs"]
mod tests;
