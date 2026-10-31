// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The session pump: what one live session does with each frame, in both directions, sans I/O.
//!
//! A session's caller and far end speak the same dialect, so the pump is a relay: it
//! runs the reader only as a read-only tap and sends the client's bytes with governed byte-level
//! splices (a writer runs only when translating). A frame from the far end
//! ([`SessionPump::on_server_frame`]) reaches the caller as the far end sent it; a frame from the
//! caller ([`SessionPump::on_client_frame`]) reaches the far end as the caller sent it, except
//! that a frame configuring the session, or asking for one response under its own settings, takes
//! the session's locked keys ([`crate::splice`]). The reader reads each frame only to meter it.
//!
//! Nothing here answers for the caller (Law 11): a tool call, a barge-in, an error reach the caller
//! as they came, and the caller acts on them. Nothing here sends, sleeps, reads a clock or holds a
//! key; the caller carries the plan and is told when a turn closes.
//!
//! What the pump holds for the session: the open turn's own counters (the
//! uplink audio admitted and the tool calls opened since the last turn closed, the two quantities no
//! usage report carries), and the locked session config.
//!
//! What it asks of its caller, through [`TurnSink`]: when a turn closes (a usage report, an error,
//! or the session ending with a turn open), the turn's usage and counters, and whether the session
//! stays open.

use std::borrow::Cow;

use crate::codec::ir::codec::peek::Lite;
use crate::codec::ir::codec::{DuplexReader, Metered, WireEvent};
use crate::codec::ir::config::SessionConfig;
use crate::codec::ir::media::AudioFormat;
use crate::codec::ir::usage::IrDuplexUsage;
use crate::session::TurnCounters;
use crate::splice::{splice, LockedKeys, Rule};

/// The reason code of the error frame a session past its configured ceiling is closed with.
pub const SESSION_CEILING_REASON: &str = "session_expired";

/// ONE FRAME'S PLAN: what the caller carries to each side, and whether the session ends.
#[derive(Debug, Default)]
pub struct Outbound {
    /// Frames bound for the far end.
    pub upstream: Vec<WireEvent>,
    /// Frames bound for the caller.
    pub downlink: Vec<WireEvent>,
    /// The session must close: its turn sink answered that it may not continue.
    pub close: bool,
    /// The far end reported usage the reader could not read: the turn bills the plane's floor.
    pub usage_unreadable: bool,
}

/// Where a closed turn goes: its usage and its counters. The answer is whether the session may
/// continue; `false` closes it.
pub trait TurnSink {
    /// One turn closed.
    fn turn_closed(&mut self, usage: Option<&IrDuplexUsage>, counters: TurnCounters) -> bool;
}

/// The session pump over the session's codec `C`.
#[derive(Debug)]
pub struct SessionPump<C> {
    codec: C,
    turn: TurnCounters,
    /// The locked `session` config: its keys are spliced over a caller's own.
    locked_config: Option<SessionConfig>,
    /// The format uplink audio is counted in when a frame's wire states none: the locked config's
    /// input format, else the one the caller's session update states, else PCM16.
    audio_in: AudioFormat,
    /// The governed splices every caller frame takes under the lock, built once at the lock.
    rules: Vec<Rule>,
}

impl<C> SessionPump<C>
where
    C: DuplexReader + LockedKeys,
{
    /// A pump over `codec`, locked to `locked_config` when one is given.
    pub fn new(codec: C, locked_config: Option<SessionConfig>) -> Self {
        let audio_in = locked_config
            .as_ref()
            .and_then(|c| c.input_audio_format)
            .unwrap_or(AudioFormat::Pcm16);
        let rules = match &locked_config {
            Some(locked) => std::iter::once(codec.configure_rule(locked))
                .chain(codec.response_rule(locked))
                .collect(),
            None => Vec::new(),
        };
        SessionPump {
            codec,
            rules,
            turn: TurnCounters::default(),
            locked_config,
            audio_in,
        }
    }

    /// The error frame a session past its `ceiling_secs` ceiling is told before it closes, in its
    /// dialect; `None` when the dialect has no place to say so.
    pub fn ceiling_error(&mut self, ceiling_secs: u64) -> Option<WireEvent> {
        self.codec
            .ceiling_frame(
                SESSION_CEILING_REASON,
                &format!(
                    "the session reached its configured ceiling of {ceiling_secs} s (streams.session_max_secs)"
                ),
            )
            .map(|frame| WireEvent(frame.into()))
    }

    /// The locked session as a frame that configures the far end, in the shape its socket reads
    /// ([`LockedKeys::locked_frame`]): what a caller-side envelope that configures nothing itself
    /// (the telephony carrier) is translated into. `None` with no lock.
    pub fn locked_update(&mut self) -> Option<WireEvent> {
        let frame = self.codec.locked_frame(self.locked_config.as_ref()?)?;
        Some(WireEvent(frame.into()))
    }

    /// A frame from the far end. It reaches the caller as the far end sent it: a tool call is part
    /// of the model's response and the caller runs it, a barge-in is the caller's to act on. The reader reads it only to meter: a usage report or an error closes the open
    /// turn, and a tool call the far end opens is counted ([`DuplexReader::meter_down`]: a frame
    /// whose head shows it carries none of these, the audio, is not read past its head).
    pub fn on_server_frame(&mut self, frame: WireEvent, sink: &mut dyn TurnSink) -> Outbound {
        let mut out = Outbound::default();
        if frame.0.is_empty() {
            return out;
        }
        for metered in self.codec.meter_down(&frame.0) {
            match metered {
                Metered::Usage(u) => {
                    out.usage_unreadable |= u.floor;
                    self.settle_turn(sink, Some(&u), &mut out);
                }
                Metered::Error => self.settle_turn(sink, None, &mut out),
                Metered::ToolOpen => self.turn.open_tool_call(),
            }
        }
        out.downlink.push(frame);
        out
    }

    /// A FRAME FROM THE CALLER. It reaches the far end as the caller sent it; uplink audio is
    /// counted. Under a lock it is governed by its BYTES, whatever its `type` says: every
    /// configuring object and per-response object in it takes the locked keys, and a frame that
    /// authors an instructing item goes nowhere. A frame that configures an uplink
    /// format the plane cannot meter, or one other than the session's locked format, goes nowhere:
    /// the far end would read the caller's audio at a rate the plane does not count it at. A frame
    /// that is not JSON goes nowhere: bytes the reader cannot read are never handed to a far end
    /// whose own reader might read them differently.
    pub fn on_client_frame(&mut self, frame: WireEvent) -> Outbound {
        let mut out = Outbound::default();
        // The uplink format the frame switches the session to, committed only once the frame is
        // relayed: a frame that goes nowhere configures nothing at the far end, so nothing here.
        let mut switch = None;
        // The frame is read once, as strictly as a whole-frame read, its audio left where it lies.
        // An object naming none of the dialect's governed members passes every check and every
        // splice as it came (`LockedKeys::GOVERNED`); any other frame takes the checks and the
        // splices. A frame that read refuses carries no audio the meter counts.
        let read = Lite::read(&frame.0);
        let plain = read
            .as_ref()
            .and_then(Lite::keys)
            .is_some_and(|mut keys| !keys.any(|k| C::GOVERNED.contains(&k)));
        if !plain {
            if read.is_none() && serde_json::from_slice::<serde::de::IgnoredAny>(&frame.0).is_err()
            {
                return out;
            }
            let locked_format = self
                .locked_config
                .as_ref()
                .and_then(|c| c.input_audio_format);
            let Ok(named) = self.codec.uplink_format(&frame.0) else {
                return out;
            };
            match (locked_format, named) {
                (Some(locked), Some(named)) if locked != named => return out,
                (None, Some(named)) => switch = Some(named),
                _ => {}
            }
            if self.locked_config.is_some() && self.codec.authors_instructions(&frame.0) {
                return out;
            }
        }
        // The governed splices; `None` = the frame goes as it came.
        let mut spliced = None;
        if !plain && !self.rules.is_empty() {
            let mut bytes = Cow::Borrowed(&frame.0[..]);
            for rule in &self.rules {
                match splice(&bytes, rule) {
                    Some(edited) => bytes = Cow::Owned(edited),
                    None => return out,
                }
            }
            // At least one rule ran, so the bytes are already the splice's own.
            let bytes = bytes.into_owned();
            if bytes[..] != frame.0[..] {
                spliced = Some(bytes);
            }
        }
        // The frame goes to the far end: the format it names is the one the far end now reads,
        // and its audio is counted at it.
        if let Some(named) = switch {
            self.audio_in = named;
        }
        if let Some(top) = &read {
            let (turn, audio_in) = (&mut self.turn, self.audio_in);
            self.codec.uplink_audio(top, &mut |format, len| {
                turn.admit_audio(format.unwrap_or(audio_in), len);
            });
        }
        out.upstream.push(match spliced {
            Some(bytes) => WireEvent(bytes.into()),
            None => frame,
        });
        out
    }

    /// Close the open turn: its usage and counters go to `sink`; a sink that refuses to continue
    /// closes the session.
    fn settle_turn(
        &mut self,
        sink: &mut dyn TurnSink,
        usage: Option<&IrDuplexUsage>,
        out: &mut Outbound,
    ) {
        let counters = self.turn.close();
        if !sink.turn_closed(usage, counters) {
            out.close = true;
        }
    }

    /// The open turn's counters as they stand: what [`Self::settle_open_turn`] would settle now.
    #[must_use]
    pub fn open_turn(&self) -> TurnCounters {
        self.turn
    }

    /// The session ends with a turn open: the turn's counters settle, once. A turn with nothing
    /// counted settles nothing.
    pub fn settle_open_turn(&mut self, sink: &mut dyn TurnSink) {
        if self.turn.is_empty() {
            return;
        }
        let mut out = Outbound::default();
        self.settle_turn(sink, None, &mut out);
    }
}

#[cfg(test)]
#[path = "tests/session_pump_tests.rs"]
mod tests;
