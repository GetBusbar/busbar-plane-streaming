// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! ONE REQUEST UNIT AS THE KERNEL'S ROUTE PUMP DRIVES IT (`BUSBAR-1.6.0.md` Part 3, section 12):
//! the pieces of a one-request door (the ephemeral-secret mint, the SDP offer), in the order the
//! pump pushes them, and what the plane answers each with.
//!
//! Per attempt the pump pushes an ATTEMPT piece first, then the caller's body (re-pushed on every
//! attempt), then the far end's answer. The plane answers:
//!
//! | piece | mint | SDP offer |
//! |---|---|---|
//! | ATTEMPT | the mint request, whole | the request head; its body follows |
//! | the caller's body | nothing (the mint reads no caller body) | relayed to the far end as it arrived |
//! | the far end's answer | gathered; on its last piece, the caller's answer | the same |
//!
//! Every answer is plain data in the plane's own vocabulary; writing it into the host's buffers is
//! the door's job. A new ATTEMPT forgets what the previous attempt's far end said.

use zeroize::{Zeroize, Zeroizing};

use crate::codec::ir::config::SessionConfig;
use crate::driven::{
    mint_failed, mint_reply, sdp_reply, Attempt, Door, Reply, FIELD_CONTENT_TYPE, FIELD_LOCATION,
};

/// Who pushed a piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum From {
    /// The caller: the request body.
    Caller,
    /// The far end: its answer.
    FarEnd,
    /// The kernel: an ATTEMPT, numbered from `1`.
    Kernel(u32),
}

/// One piece, as the door read it off the crossing.
#[derive(Debug, Clone, Copy)]
pub struct Piece<'a> {
    /// Who pushed it.
    pub from: From,
    /// Its bytes.
    pub bytes: &'a [u8],
    /// On the far end's first piece: the status it answered.
    pub status: Option<u16>,
    /// No piece of this `from` follows.
    pub last: bool,
    /// On the far end's first piece: the response head fields the plane's need keeps, names
    /// lower-case.
    pub head: &'a [(&'a [u8], &'a [u8])],
}

/// The plane's answer to one piece. (No equality: a mint's answer to the caller is sealed.)
#[derive(Debug)]
pub enum Answer {
    /// Nothing to emit yet.
    Nothing,
    /// The request this attempt sends the far end; a body that follows arrives as [`Answer::ToFarEnd`].
    Attempt(Attempt),
    /// More of the request's body, bound for the far end.
    ToFarEnd(Vec<u8>),
    /// The caller's whole answer: the unit is done.
    ToCaller(Reply),
    /// The piece is one this door never takes.
    Refused,
}

/// One unit on a one-request door. Its `Debug` gives the far end's gathered answer as a length:
/// a mint's answer carries the live `ek_` secret.
#[derive(Clone)]
pub struct RequestUnit {
    door: Door,
    locked: SessionConfig,
    caller_ref: Option<String>,
    status: Option<u16>,
    location: Option<String>,
    content_type: Option<String>,
    /// The far end's answer, gathered: wiped when it is cleared or dropped.
    far: Zeroizing<Vec<u8>>,
    answered: bool,
}

impl std::fmt::Debug for RequestUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestUnit")
            .field("door", &self.door)
            .field("locked", &self.locked)
            .field("caller_ref", &self.caller_ref)
            .field("status", &self.status)
            .field("location", &self.location)
            .field("content_type", &self.content_type)
            .field("far", &format_args!("{} bytes", self.far.len()))
            .field("answered", &self.answered)
            .finish()
    }
}

impl RequestUnit {
    /// Open the plane's state for one kernel unit on `door` (the unit's identity is the kernel's;
    /// the door keys this state by it): minting over the `locked` session params, naming the caller
    /// by `caller_ref` (the kernel's reference, never the principal). `None` for a door that opens
    /// a live session.
    #[must_use]
    pub fn open(door: Door, locked: SessionConfig, caller_ref: Option<String>) -> Option<Self> {
        if door.is_session() {
            return None;
        }
        Some(Self {
            door,
            locked,
            caller_ref,
            status: None,
            location: None,
            content_type: None,
            far: Zeroizing::new(Vec::new()),
            answered: false,
        })
    }

    /// `true` once the caller's answer has been given.
    #[cfg(test)]
    #[must_use]
    pub const fn answered(&self) -> bool {
        self.answered
    }

    fn answer(&mut self, reply: Reply) -> Answer {
        self.answered = true;
        Answer::ToCaller(reply)
    }

    /// The plane's answer to `piece`.
    pub fn on_piece(&mut self, piece: Piece<'_>) -> Answer {
        if self.answered {
            return Answer::Nothing;
        }
        match (self.door, piece.from) {
            (_, From::Kernel(0)) => Answer::Refused,
            (_, From::Kernel(_)) => {
                self.status = None;
                self.location = None;
                self.content_type = None;
                self.far.zeroize();
                match self
                    .door
                    .route(&self.locked, self.caller_ref.as_deref(), &[])
                {
                    Ok(attempt) => Answer::Attempt(attempt),
                    Err(reason) => {
                        let reply = mint_failed(&reason);
                        self.answer(reply)
                    }
                }
            }
            (Door::Sdp, From::Caller) => {
                if piece.bytes.is_empty() {
                    Answer::Nothing
                } else {
                    Answer::ToFarEnd(piece.bytes.to_vec())
                }
            }
            (_, From::Caller) => Answer::Nothing,
            (_, From::FarEnd) => self.far_end_piece(piece),
        }
    }

    fn far_end_piece(&mut self, piece: Piece<'_>) -> Answer {
        if let Some(status) = piece.status {
            self.status = Some(status);
            let field = |wanted: &str| {
                piece
                    .head
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(wanted.as_bytes()))
                    .and_then(|(_, value)| std::str::from_utf8(value).ok())
                    .map(str::to_owned)
            };
            self.location = field(FIELD_LOCATION);
            self.content_type = field(FIELD_CONTENT_TYPE);
        }
        gather(&mut self.far, piece.bytes);
        if !piece.last {
            return Answer::Nothing;
        }
        let Some(status) = self.status else {
            return Answer::Refused;
        };
        let reply = match self.door {
            Door::Mint => mint_reply(status, &self.far),
            _ => sdp_reply(
                status,
                self.location.as_deref(),
                self.content_type.as_deref(),
                &self.far,
            ),
        };
        self.far.zeroize();
        self.answer(reply)
    }
}

/// Append `bytes` to the gathered answer `far`. Outgrowing its buffer moves it into a larger one
/// and wipes the old (a `Vec` that grew in place would leave each outgrown buffer unwiped).
fn gather(far: &mut Zeroizing<Vec<u8>>, bytes: &[u8]) {
    if far.capacity() - far.len() < bytes.len() {
        let wanted = (far.len() + bytes.len()).max(far.capacity().saturating_mul(2));
        let mut grown = Zeroizing::new(Vec::with_capacity(wanted));
        grown.extend_from_slice(far);
        *far = grown;
    }
    far.extend_from_slice(bytes);
}

#[cfg(test)]
#[path = "tests/request_unit_tests.rs"]
mod tests;
