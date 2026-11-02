// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The session pump: what one live session does with each frame, in both directions, sans I/O.

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
    /// Whether the session already said, once, that it refused a caller's audio it cannot meter.
    told_unmeterable: bool,
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
            told_unmeterable: false,
        }
    }

    /// The error frame a session past its `ceiling_secs` ceiling is told before it closes, in its
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
    pub fn locked_update(&mut self) -> Option<WireEvent> {
        let frame = self.codec.locked_frame(self.locked_config.as_ref()?)?;
        Some(WireEvent(frame.into()))
    }

    /// A frame from the far end. It reaches the caller as the far end sent it: a tool call is part
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
    pub fn on_client_frame(&mut self, frame: WireEvent) -> Outbound {
        let mut out = Outbound::default();

        let mut switch = None;

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

        let mut spliced = None;
        if !plain && !self.rules.is_empty() {
            let mut bytes = Cow::Borrowed(&frame.0[..]);
            for rule in &self.rules {
                match splice(&bytes, rule) {
                    Some(edited) => bytes = Cow::Owned(edited),
                    None => return out,
                }
            }

            let bytes = bytes.into_owned();
            if bytes[..] != frame.0[..] {
                spliced = Some(bytes);
            }
        }

        if let Some(top) = &read {
            if self.codec.uplink_audio(top, &mut |_, _| {}).is_err() {
                if !std::mem::replace(&mut self.told_unmeterable, true) {
                    tracing::warn!(
                        "a caller frame carries audio the streaming plane cannot meter (a format \
                         other than the PCM rates it counts, or data that does not decode); the \
                         frame is not relayed, and nor is any like it on this session"
                    );
                }
                return out;
            }
        }

        if let Some(named) = switch {
            self.audio_in = named;
        }
        if let Some(top) = &read {
            let (turn, audio_in) = (&mut self.turn, self.audio_in);

            let _ = self.codec.uplink_audio(top, &mut |format, len| {
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
