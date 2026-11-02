// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! One live session as the driver serves it, sans I/O: the session pump, the telephony carrier's

use bytes::Bytes;

use crate::codec::ir::codec::{DuplexReader, WireEvent};
use crate::codec::ir::config::SessionConfig;
use crate::codec::ir::usage::IrDuplexUsage;
use crate::codec::topology::twilio::{CallerStep, TwilioBridge};
use crate::meta;
use crate::session::{class_counts, TurnCounters};
use crate::session_pump::{SessionPump, TurnSink};
use crate::splice::LockedKeys;

/// The billable classes a session reports, in the tail's order (their index is the class a unit
pub const CLASSES: [busbar_contract::ids::MeterClassId; 8] = [
    meta::CLASS_AUDIO_TOKENS_IN,
    meta::CLASS_AUDIO_TOKENS_OUT,
    meta::CLASS_TEXT_TOKENS_IN,
    meta::CLASS_TEXT_TOKENS_OUT,
    meta::CLASS_AUDIO_SECONDS_IN,
    meta::CLASS_TOOL_CALLS,
    meta::CLASS_IMAGE_TOKENS_IN,
    meta::CLASS_VIDEO_TOKENS_IN,
];

/// The index of the output text tokens in [`CLASSES`]: the class an unreadable usage report's floor
pub const TEXT_TOKENS_OUT: u32 = 3;

/// The session's units so far, per class: every closed turn adds to them, and the session always
/// continues (the kernel owns the cut).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CumulativeUnits(pub [u64; CLASSES.len()]);

impl CumulativeUnits {
    /// The units as the door reports them: (class index, amount), the zero classes left out.
    #[must_use]
    pub fn reported(&self) -> Vec<(u32, u64)> {
        self.0
            .iter()
            .enumerate()
            .filter(|(_, n)| **n != 0)
            .map(|(i, n)| (i as u32, *n))
            .collect()
    }
}

impl TurnSink for CumulativeUnits {
    fn turn_closed(&mut self, usage: Option<&IrDuplexUsage>, counters: TurnCounters) -> bool {
        for (class, n) in class_counts(usage, counters) {
            if let Some(i) = CLASSES.iter().position(|c| *c == class) {
                self.0[i] = self.0[i].saturating_add(n);
            }
        }
        true
    }
}

/// What one piece asks the driver to carry. Every frame a session emits, to either side, is one
/// TEXT message (the door sets the text bit on each): the realtime dialects and the telephony envelope are JSON
/// events. What arrives may be either: Gemini Live's server sends its JSON in binary frames, and a
/// frame is read by its bytes, not its opcode.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Frames bound for the far end, in order.
    pub to_far_end: Vec<Vec<u8>>,
    /// Frames bound for the caller, in order.
    pub to_caller: Vec<Vec<u8>>,
    /// The session ends after these frames.
    pub end: bool,
}

/// ONE LIVE SESSION over the far end's codec `C`.
#[derive(Debug)]
pub struct SessionUnit<C> {
    pump: SessionPump<C>,
    units: CumulativeUnits,
    twilio: Option<TwilioBridge>,
    /// The far end has been sent the locked session the telephony carrier's start called for.
    locked_told: bool,
    /// The far end reported usage the reader could not read; not yet reported to the kernel.
    unreadable: bool,
    /// A turn of the session billed the plane's floor (its output tokens): the far end's usage
    /// report could not be read.
    floored: bool,
    ended: bool,
}

impl<C> SessionUnit<C>
where
    C: DuplexReader + LockedKeys,
{
    /// A session over `codec`, locked to `locked`. `telephony` puts the telephony carrier's
    pub fn open(codec: C, locked: SessionConfig, telephony: bool) -> Self {
        SessionUnit {
            pump: SessionPump::new(codec, Some(locked)),
            units: CumulativeUnits::default(),
            twilio: telephony.then(TwilioBridge::default),
            locked_told: false,
            unreadable: false,
            floored: false,
            ended: false,
        }
    }

    /// The session's cumulative units, per class in [`CLASSES`] order: every closed turn's, and the
    #[must_use]
    pub fn units(&self) -> CumulativeUnits {
        let mut units = self.units;
        let open = self.pump.open_turn();
        if !open.is_empty() {
            units.turn_closed(None, open);
        }
        units
    }

    /// The locked session, once, for a native socket's initial upstream request. A carrier that
    pub fn initial_lock(&mut self) -> Option<Vec<u8>> {
        if std::mem::replace(&mut self.locked_told, true) {
            return None;
        }
        self.pump.locked_update().map(|w| w.0.to_vec())
    }

    /// `true` once the session has ended.
    #[cfg(test)]
    #[must_use]
    pub fn ended(&self) -> bool {
        self.ended
    }

    fn caller_frames(&self, frames: Vec<WireEvent>) -> Vec<Vec<u8>> {
        frames
            .into_iter()
            .filter_map(|w| match &self.twilio {
                Some(bridge) => bridge.to_caller(&w.0),
                None => Some(w.0.to_vec()),
            })
            .collect()
    }

    /// A piece from the caller. Behind the telephony carrier's envelope, the carrier's start is
    #[allow(clippy::wrong_self_convention)]
    pub fn from_caller(&mut self, bytes: &[u8]) -> Plan {
        if self.ended {
            return Plan::default();
        }
        let (frame, mut to_far_end) = match self.twilio.as_mut() {
            None => (Some(WireEvent(Bytes::copy_from_slice(bytes))), Vec::new()),
            Some(bridge) => {
                let step = bridge.from_caller(bytes);
                let started = bridge.started();
                let frame = match step {
                    CallerStep::Audio(w) => Some(w),
                    CallerStep::Stop | CallerStep::Refused => return self.end(),
                    CallerStep::Nothing => None,
                };
                let mut told = Vec::new();
                if started && !self.locked_told {
                    self.locked_told = true;
                    told.extend(self.pump.locked_update().map(|w| w.0.to_vec()));
                }
                (frame, told)
            }
        };
        if let Some(frame) = frame {
            let out = self.pump.on_client_frame(frame);
            to_far_end.extend(out.upstream.into_iter().map(|w| w.0.to_vec()));
        }
        Plan {
            to_far_end,
            to_caller: Vec::new(),
            end: false,
        }
    }

    /// A piece from the far end. A tool call in it is relayed to the caller as-is: the
    #[allow(clippy::wrong_self_convention)]
    pub fn from_far_end(&mut self, bytes: &[u8]) -> Plan {
        if self.ended {
            return Plan::default();
        }
        let out = self
            .pump
            .on_server_frame(WireEvent(Bytes::copy_from_slice(bytes)), &mut self.units);
        let to_caller = self.caller_frames(out.downlink);
        if out.usage_unreadable {
            self.unreadable = true;
            self.floored = true;
        }
        Plan {
            to_far_end: out.upstream.into_iter().map(|w| w.0.to_vec()).collect(),
            to_caller,
            end: false,
        }
    }

    /// Whether the far end sent a usage report the reader could not read and the kernel has not been
    pub fn take_unreadable(&mut self) -> bool {
        std::mem::take(&mut self.unreadable)
    }

    /// Whether a turn of the session billed the plane's floor: its output tokens are the floor's.
    #[must_use]
    pub fn floored(&self) -> bool {
        self.floored
    }

    /// The session reached the wall-clock ceiling of `ceiling_secs` its host measured: it is told
    pub fn close_at_ceiling(&mut self, ceiling_secs: u64) -> Plan {
        if self.ended {
            return Plan::default();
        }
        let told = self.pump.ceiling_error(ceiling_secs);
        let mut plan = self.end();
        plan.to_caller = self.caller_frames(told.into_iter().collect());
        plan
    }

    /// The session ends: its open turn settles once.
    pub fn end(&mut self) -> Plan {
        if !self.ended {
            self.ended = true;
            self.pump.settle_open_turn(&mut self.units);
        }
        Plan {
            end: true,
            ..Plan::default()
        }
    }
}

#[cfg(test)]
#[path = "tests/session_unit_tests.rs"]
mod tests;
