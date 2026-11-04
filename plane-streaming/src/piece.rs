// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THIS PLANE'S PIECE ANSWER: what an `on_piece` answer owes the host's reply buffer, and the
//! one settle of the answer's field, unit and arena buffers.

use busbar_contract::abi::plane::{
    OnPieceIn, OnPieceOut, OutField, RecordWrite, UnitCount, EMIT_DONE, EMIT_TO_FAR_END,
    PIECE_OUT_TEXT,
};
use busbar_contract::abi::sdk::cred::SealedWrite;
use busbar_contract::abi::sdk::{HostBuf, Lent, Out};

use crate::driven::Body;

/// Whether `input` is the kernel's re-call after a `more = 1` answer: no bytes, no flags and no
/// attempt.
#[must_use]
pub fn is_recall(input: Lent<'_, OnPieceIn>) -> bool {
    let given = input;
    given.flags() == 0 && given.attempt_no() == 0 && input.bytes().bytes().is_empty()
}

/// The bytes a unit's answer still owes the host's reply buffer, and the `EMIT_*` bits they carry.
/// A sealed answer (a mint's) is owed as the SDK's [`SealedWrite`], which moves its bytes into the
/// reply buffer itself; its `Debug` gives lengths only.
#[derive(Default)]
pub struct Owed {
    bytes: Vec<u8>,
    at: usize,
    sealed: Option<SealedWrite>,
    flags: u32,
}

impl std::fmt::Debug for Owed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Owed")
            .field("bytes", &self.bytes.len())
            .field("at", &self.at)
            .field("sealed", &self.sealed.as_ref().map(SealedWrite::remaining))
            .field("flags", &self.flags)
            .finish()
    }
}

impl Owed {
    /// Whether any byte is still owed.
    #[must_use]
    pub fn pending(&self) -> bool {
        self.at < self.bytes.len() || self.sealed.as_ref().is_some_and(|w| !w.finished())
    }

    /// Owe a caller's answer `body` under the `EMIT_*` `flags`, and pay what fits now: public
    /// bytes as [`Owed::owe`] owes them, a sealed answer through the SDK.
    pub fn owe_body(
        &mut self,
        body: Body,
        flags: u32,
        input: Lent<'_, OnPieceIn>,
        out: &mut Out<'_, OnPieceOut>,
    ) -> Result<bool, &'static str> {
        match body {
            Body::Public(bytes) => self.owe(bytes, flags, input, out),
            Body::Sealed(write) => {
                *self = Owed {
                    sealed: Some(write),
                    flags,
                    ..Owed::default()
                };
                self.pay(input, out)
            }
        }
    }

    /// Owe `bytes` under the `EMIT_*` `flags`, and pay what fits now.
    pub fn owe(
        &mut self,
        bytes: Vec<u8>,
        flags: u32,
        input: Lent<'_, OnPieceIn>,
        out: &mut Out<'_, OnPieceOut>,
    ) -> Result<bool, &'static str> {
        self.sealed = None;
        self.bytes = bytes;
        self.at = 0;
        self.flags = flags;
        self.pay(input, out)
    }

    /// Write what is owed into the reply buffer, as much as fits: `more = 1` while any is left,
    /// and the owed flags on every write, `EMIT_DONE` held back until the last byte, and so is
    /// a far-bound message's `PIECE_OUT_TEXT` (the kernel gathers a far-bound message whole, so
    /// its last piece names it text).
    pub fn pay(
        &mut self,
        input: Lent<'_, OnPieceIn>,
        out: &mut Out<'_, OnPieceOut>,
    ) -> Result<bool, &'static str> {
        let first = self.at == 0;
        let n = match &mut self.sealed {
            Some(write) => match write.stream_into(&mut input.reply_buf()) {
                Ok(n) => n,
                Err(_) => {
                    *self = Owed::default();
                    return Err("the sealed answer could not be written to the host reply buffer");
                }
            },
            None => {
                let n = input.reply_buf().stream(&self.bytes[self.at..]);
                self.at += n;
                n
            }
        };
        let more = self.pending();
        if more && first && self.flags & (PIECE_OUT_TEXT | EMIT_TO_FAR_END) == PIECE_OUT_TEXT {
            tracing::warn!(
                frame = self.bytes.len(),
                window = n,
                "a frame bound for the caller is wider than the host's reply window: it is \
                 written in pieces, and only its last piece is marked as text"
            );
        }
        out.set(|o| &o.emitted, n as u64);
        out.set(|o| &o.more, u32::from(more));
        // While more is owed, neither the end nor the text bit is stated: the contract's rule
        // (`on_piece.text`) refuses a text message that a later re-call continues, either way.
        out.set(
            |o| &o.flags,
            if more {
                self.flags & !(EMIT_DONE | PIECE_OUT_TEXT)
            } else {
                self.flags
            },
        );
        if more {
            return Ok(false);
        }
        let done = self.flags & EMIT_DONE != 0;
        *self = Owed::default();
        Ok(done)
    }
}

/// Settle an answer's field, unit and arena buffers: `(written, needed)` of each, short as a
/// whole when any one is short.
pub fn settle(
    out: &mut Out<'_, OnPieceOut>,
    fields: &HostBuf<'_, OutField>,
    units: &HostBuf<'_, UnitCount>,
    arena: &HostBuf<'_, u8>,
    records: Option<&HostBuf<'_, RecordWrite>>,
) -> bool {
    let short =
        !(fields.fits() && units.fits() && arena.fits() && records.is_none_or(HostBuf::fits));
    if let Some(records) = records {
        let (rw, rnd) = records.settle(short);
        out.set(|o| &o.records_written, rw as u32);
        out.set(|o| &o.records_needed, rnd as u32);
    }
    let (fw, fnd) = fields.settle(short);
    let (uw, und) = units.settle(short);
    let (aw, and) = arena.settle(short);
    out.set(|o| &o.fields_written, fw as u32);
    out.set(|o| &o.fields_needed, fnd as u32);
    out.set(|o| &o.units_written, uw as u32);
    out.set(|o| &o.units_needed, und as u32);
    out.set(|o| &o.arena_written, aw as u64);
    out.set(|o| &o.arena_needed, and as u64);
    short
}
