// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! One request unit as the kernel's route pump drives it: the mint door's one request, and what
//! the plane answers it with.
//!
//! The mint door is answered by the plane itself (`ROUTE_LOCAL`): it dials nothing, so the pump
//! pushes the caller's body alone, as one last piece. The plane answers it with a ticket busbar
//! mints (`ticket.mint`, row 132), its record the session config the ticket locks. The door makes the
//! host call; this unit says what to ask for and how the caller is answered.
//!
//! Every answer is plain data in the plane's own vocabulary; writing it into the host's buffers is
//! the door's job.

use std::sync::Arc;

use busbar_contract::abi::sdk::Minted;

use crate::codec::ir::config::SessionConfig;
use crate::driven::{mint_failed, mint_not_granted, minted, Door, Reply};
use crate::ticket::{ticket_record, TTL_SECS};

/// Who pushed a piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum From {
    /// The caller: the request body.
    Caller,
    /// The far end: its answer.
    FarEnd,
    /// The kernel: an ATTEMPT or a collection (a unit that dials nothing reads neither).
    Kernel,
}

/// One piece, as the door read it off the crossing: the mint reads no body and no far end, only who
/// pushed it and whether it is that side's last.
#[derive(Debug, Clone, Copy)]
pub struct Piece {
    /// Who pushed it.
    pub from: From,
    /// No piece of this `from` follows.
    pub last: bool,
}

/// The plane's answer to one piece. (No equality: a mint's answer to the caller is sealed.)
#[derive(Debug)]
pub enum Answer {
    /// Nothing to emit yet.
    Nothing,
    /// Mint a ticket living `ttl_secs` with `record` stored beside it, then answer the caller
    /// through [`RequestUnit::minted`].
    Mint {
        /// The lifetime asked, in seconds.
        ttl_secs: u64,
        /// The ticket's record: the session config it locks.
        record: Vec<u8>,
    },
    /// The caller's whole answer: the unit is done.
    ToCaller(Reply),
    /// The piece is one this door never takes.
    Refused,
}

/// One unit on a one-request door.
#[derive(Debug, Clone)]
pub struct RequestUnit {
    locked: Arc<SessionConfig>,
    asked: bool,
    answered: bool,
}

impl RequestUnit {
    /// Open the plane's state for one kernel unit on `door` (the unit's identity is the kernel's;
    /// the door keys this state by it), minting over the `locked` session params. `None` for a door
    /// that opens a live session.
    #[must_use]
    pub fn open(door: Door, locked: impl Into<Arc<SessionConfig>>) -> Option<Self> {
        if door.is_session() {
            return None;
        }
        Some(Self {
            locked: locked.into(),
            asked: false,
            answered: false,
        })
    }

    /// `true` once the far end has begun its answer: never, for a unit that dials nothing, so a
    /// cancel of one is ABORTED (owner billing rule R21).
    #[must_use]
    pub const fn far_answered(&self) -> bool {
        false
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

    /// The plane's answer to `piece`. The caller's last piece asks for the ticket, once; a far
    /// end never answers a unit that dials nothing.
    pub fn on_piece(&mut self, piece: Piece) -> Answer {
        if self.answered || self.asked {
            return Answer::Nothing;
        }
        match piece.from {
            From::Caller if piece.last => match ticket_record(&self.locked) {
                Ok(record) => {
                    self.asked = true;
                    Answer::Mint {
                        ttl_secs: TTL_SECS,
                        record,
                    }
                }
                Err(reason) => self.answer(mint_failed(&reason)),
            },
            From::Caller | From::Kernel => Answer::Nothing,
            From::FarEnd => Answer::Refused,
        }
    }

    /// The key's grant does not reach `entry`, the model the ticket's call would be routed to: the
    /// caller is refused, explained, and no ticket is asked for (R10 grant ruling).
    pub fn not_granted(&mut self, entry: &str) -> Answer {
        self.answer(mint_not_granted(entry))
    }

    /// The host answered the mint: the caller's answer, the ticket (sealed: R7) and its expiry, or
    /// the served plane's 502 naming why.
    pub fn minted(&mut self, result: Result<Minted, String>) -> Answer {
        let reply = match result {
            Ok(m) => minted(&m.ticket, m.expires_at_unix),
            Err(reason) => mint_failed(&reason),
        };
        self.answer(reply)
    }
}

#[cfg(test)]
#[path = "tests/request_unit_tests.rs"]
mod tests;
