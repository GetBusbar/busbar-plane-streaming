// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! LIVE SESSIONS THROUGH THE DOOR (`BUSBAR-1.6.0.md` Part 3, section 12, "Duplex sessions" (K6);
//! THE DESIGN, section 7, "A session is one unit with one line").
//!
//! A piece that names a stream is a live session's. The door keeps one [`Live`] per stream, built
//! at the stream's first piece over the newest live generation's `streams:` section, and answers
//! each piece in the driver's duplex vocabulary:
//!
//! - A CALLER piece goes through the session ([`SessionUnit::from_caller`]); a frame it owes the
//!   far end is that answer's `EMIT_TO_FAR_END` (one turn). The caller's last piece ends the
//!   session: its open turn settles once and the answer is `EMIT_DONE`.
//! - A FAR-END piece goes through the session ([`SessionUnit::from_far_end`]); a frame it owes the
//!   caller is that answer's emission.
//! - ONE FRAME PER ANSWER. Every frame keeps its own message boundary, so a piece that leaves more
//!   than one frame owed (the telephony carrier's start and the locked session it is translated
//!   into, a ceiling's error and the end) answers the first and queues the rest. A queued frame is
//!   the session's unsolicited output: the door wakes the instance's driver ticket, `drive` names
//!   the session, and each collection (`FROM_KERNEL`, no bytes, no attempt) answers the next queued
//!   frame, toward the side it is bound for.
//! - EVERY ANSWER REPORTS THE SESSION'S CUMULATIVE UNITS, the eight classes the plane counts over its
//!   closed turns and its open turn's running counts (so a cancel or a cut, which reports nothing
//!   of its own, bills what streamed from the last answer), and, once the far end has first
//!   answered the session, its fee unit ([`PER_SESSION_CLASS`], `1`): a session whose far end never
//!   answered (its open failed) reports none, and its fee is refunded. The kernel checkpoints them
//!   and writes the session's one line at its end; the plane holds no reservation and never cuts on
//!   money.
//! - THE WALL-CLOCK CEILING (`streams.session_max_secs`, Q21a) is measured on the kernel's tick
//!   clock: while a live generation configures one the door asks to be ticked every
//!   [`CEILING_TICK_NS`], a session's clock starts at the first tick after its first piece, and a
//!   session past its ceiling is told why in its dialect and ends once its queued frames are
//!   collected.
//! - A CANCEL on either side's ticket ends the session: it is forgotten, and the kernel's cleanup
//!   writes its line.

use std::collections::{BTreeMap, HashMap, VecDeque};

use busbar_contract::abi::mechanism::ticket::Ticket;

use crate::codec::ir::codec::OpenAiRealtimeCodec;
use crate::codec::ir::config::{MaxOutputTokens, SessionConfig};
use crate::codec::ir::media::AudioFormat;
use crate::codec::ir::GeminiLiveCodec;
use crate::config::StreamsCfg;
use crate::driven::Door;
use crate::session_unit::{CumulativeUnits, Plan, SessionUnit};

/// Nanoseconds per second, on the kernel's tick clock.
const NS_PER_SEC: u64 = 1_000_000_000;

/// How often the door asks to be ticked while a live generation configures a session ceiling: a
/// session's clock starts, and its ceiling is enforced, at most this late.
///
/// The clock starts at a tick rather than at the first piece because a piece carries no clock
/// reading on the tick clock, and a ceiling compared across two clocks is no ceiling.
pub const CEILING_TICK_NS: u64 = NS_PER_SEC;

/// The tail's class index of the session's fee unit (`per_session`), after the eight counted
/// classes ([`crate::session_unit::CLASSES`]).
pub const PER_SESSION_CLASS: u32 = crate::session_unit::CLASSES.len() as u32;

/// The most live sessions the door keeps at once; past it, a new stream is refused.
pub const MAX_SESSIONS: usize = 4096;

/// The side a session's frame is bound for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The far end: one turn.
    FarEnd,
    /// The caller.
    Caller,
}

/// The session over its door's codec.
#[derive(Debug)]
enum Unit {
    /// The OpenAI Realtime far end (the sideband, telephony and plain realtime sockets, and the
    /// browser's call busbar terminates, its events on the call's data channel).
    Realtime(SessionUnit<OpenAiRealtimeCodec>),
    /// The Gemini Live far end.
    Gemini(SessionUnit<GeminiLiveCodec>),
}

macro_rules! each {
    ($unit:expr, |$u:ident| $body:expr) => {
        match $unit {
            Unit::Realtime($u) => $body,
            Unit::Gemini($u) => $body,
        }
    };
}

/// What one session answer carries.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Emit {
    /// The frame this answer writes, and the side it is bound for; `None` = nothing.
    pub frame: Option<(Side, Vec<u8>)>,
    /// The session is over: this answer is its last.
    pub done: bool,
}

/// ONE LIVE SESSION on the door.
#[derive(Debug)]
pub struct Live {
    unit: Unit,
    /// Frames owed a side, oldest first, past the one each answer carries.
    queued: VecDeque<(Side, Vec<u8>)>,
    /// The session ended; it is over once its queued frames are collected.
    ending: bool,
    /// The wall-clock ceiling of the generation it opened under, in seconds.
    ceiling_secs: Option<u64>,
    /// When its clock started, on the tick clock; `None` until the first tick after its open.
    started_ns: Option<u64>,
    /// The far end has answered the session: its fee unit was incurred.
    answered: bool,
    /// The far end accepted the session's upgrade (its status a switch or a success).
    accepted: bool,
    /// The provider's call a sideband session attaches to, as its arrival named it. The call's
    /// fee unit was reported by the SDP offer that brokered it, so an attached session reports
    /// none of its own: one fee per call.
    call_id: Option<String>,
    /// The model a session attached to no call names on its dial (`?model=`): the provider's wire
    /// name for the session model its route was named for (the catalog entry's `upstream_model`,
    /// else its name; Part 2 #51), on the plain realtime and telephony doors; `None` on the sideband
    /// (it names its call) and on the Gemini Live socket (its model rides the setup frame).
    dial_model: Option<String>,
    /// Busbar's own call an SDP door's session placed (row 132): the call a sideband names, freed
    /// when the session ends.
    placed: Option<String>,
    /// What the frame a caller-side answer is writing still owes the reply buffer.
    pub(crate) near: crate::piece::Owed,
    /// What the frame a far-side answer is writing still owes the reply buffer.
    pub(crate) far: crate::piece::Owed,
    /// A caller-side answer that did not fit its buffers: the re-call carries the same piece,
    /// which is answered from here rather than read twice.
    pub(crate) short_near: Option<Emit>,
    /// The same, for a far-side answer.
    pub(crate) short_far: Option<Emit>,
}

impl Live {
    /// The session a stream's first piece on `door` opens, under `cfg`; `None` for a door that
    /// answers one request. The telephony door locks the carrier's µ-law on both legs; the other
    /// doors lock the section's session.
    #[cfg(test)]
    #[must_use]
    pub fn open(door: Door, cfg: &StreamsCfg) -> Option<Live> {
        Self::open_locked(door, cfg, None)
    }

    /// The session params `door` locks under `cfg`, as the open's hooks are shown them: the
    /// section's session, with the carrier's 8 kHz µ-law both ways on the telephony door, so it
    /// passes straight through with no resample.
    #[must_use]
    pub fn locked(door: Door, cfg: &StreamsCfg) -> SessionConfig {
        match door {
            Door::Twilio => SessionConfig {
                input_audio_format: Some(AudioFormat::G711Ulaw),
                output_audio_format: Some(AudioFormat::G711Ulaw),
                ..cfg.session.clone()
            },
            _ => cfg.session.clone(),
        }
    }

    /// [`Self::open`], locked to `rewritten` where a session-open hook rewrote the params
    /// ([`Self::locked`]) this session opens under, with the section's two token ceilings applied
    /// ([`with_ceilings`]).
    #[must_use]
    pub fn open_locked(
        door: Door,
        cfg: &StreamsCfg,
        rewritten: Option<SessionConfig>,
    ) -> Option<Live> {
        let ceiling = cfg.session_max_secs.map(|s| u64::from(s.get()));
        // The locked session as the provider is told it: under the section's token ceilings, the
        // session model by its wire name.
        let locked = with_ceilings(
            cfg.wire_session(rewritten.unwrap_or_else(|| Self::locked(door, cfg))),
            cfg,
        );
        let unit = match door {
            Door::Sideband | Door::Realtime | Door::Sdp => {
                Unit::Realtime(SessionUnit::open(OpenAiRealtimeCodec, locked, false))
            }
            Door::Twilio => Unit::Realtime(SessionUnit::open(OpenAiRealtimeCodec, locked, true)),
            Door::Gemini => Unit::Gemini(SessionUnit::open(GeminiLiveCodec, locked, false)),
            Door::Mint => return None,
        };
        Some(Live {
            unit,
            queued: VecDeque::new(),
            ending: false,
            ceiling_secs: ceiling,
            started_ns: None,
            answered: false,
            accepted: false,
            call_id: None,
            dial_model: match door {
                Door::Realtime | Door::Twilio => cfg
                    .upstream_model
                    .clone()
                    .or_else(|| cfg.session.model.clone()),
                _ => None,
            },
            placed: None,
            near: crate::piece::Owed::default(),
            far: crate::piece::Owed::default(),
            short_near: None,
            short_far: None,
        })
    }

    /// The provider's call this session attaches to (the sideband door's `{call_id}`).
    pub fn attach(&mut self, call_id: String) {
        self.call_id = Some(call_id);
    }

    /// The far end accepted the session's upgrade.
    pub fn accept(&mut self) {
        self.accepted = true;
    }

    /// Whether the far end accepted the session's upgrade.
    #[must_use]
    pub fn accepted(&self) -> bool {
        self.accepted
    }

    /// The provider's call this session is attached to, if any.
    #[must_use]
    pub fn call_id(&self) -> Option<&str> {
        self.call_id.as_deref()
    }

    /// The target the session dials under the member's base URL: the dialect's socket `path`; for
    /// a session attached to a provider's call, that call only (`?call_id=`, the provider's
    /// sideband connection to the call's own session); for a plain or telephony session, the
    /// session's model (`?model=`, form-escaped, as OpenAI's own WS surface takes it; ARCHITECT
    /// ruling 2026-10-23 on Part 2 #45).
    #[must_use]
    pub fn dial_target(&self, path: &str) -> String {
        match (&self.call_id, &self.dial_model) {
            (Some(id), _) => format!("{path}?call_id={id}"),
            (None, Some(model)) => format!(
                "{path}?model={}",
                url::form_urlencoded::byte_serialize(model.as_bytes()).collect::<String>()
            ),
            (None, None) => path.to_string(),
        }
    }

    /// Whether the far end sent a usage report the reader could not read and the kernel has not
    /// been told; telling clears it.
    pub fn take_unreadable(&mut self) -> bool {
        each!(&mut self.unit, |u| u.take_unreadable())
    }

    /// Whether a turn of the session billed the plane's floor (its output tokens).
    #[must_use]
    pub fn floored(&self) -> bool {
        each!(&self.unit, |u| u.floored())
    }

    /// THE SESSION'S OPENING CALL: `frame`, owed the far end before anything else (the SDP door's
    /// request, which places busbar's own call and so dials the session's far end at its open).
    pub fn open_far(&mut self, frame: Vec<u8>) {
        self.queued.push_front((Side::FarEnd, frame));
    }

    /// The session ends now: whatever it still owes is collected, then its end.
    pub fn end(&mut self) {
        self.ending = true;
    }

    /// The session placed busbar's own call `id` (the SDP door's open).
    pub fn place(&mut self, id: String) {
        self.placed = Some(id);
    }

    /// Busbar's own call the session placed, if any.
    #[must_use]
    pub fn placed_call(&self) -> Option<&str> {
        self.placed.as_deref()
    }

    /// The provider answered busbar's call with a success: the session's fee unit is incurred.
    pub fn answered_call(&mut self) {
        self.answered = true;
    }

    /// Queue what `plan` owes either side, the far end's frames first.
    fn take(&mut self, plan: Plan) {
        self.queued
            .extend(plan.to_far_end.into_iter().map(|f| (Side::FarEnd, f)));
        self.queued
            .extend(plan.to_caller.into_iter().map(|f| (Side::Caller, f)));
        self.ending |= plan.end;
    }

    /// A caller piece; `last` = the caller's side ended.
    #[allow(clippy::wrong_self_convention)]
    pub fn from_caller(&mut self, bytes: &[u8], last: bool) {
        let plan = each!(&mut self.unit, |u| {
            let mut plan = if bytes.is_empty() {
                Plan::default()
            } else {
                u.from_caller(bytes)
            };
            if last {
                let ended = u.end();
                plan.end = ended.end;
            }
            plan
        });
        self.take(plan);
    }

    /// A far-end piece. The first one incurs the session's fee unit.
    #[allow(clippy::wrong_self_convention)]
    pub fn from_far_end(&mut self, bytes: &[u8]) {
        self.answered = true;
        let plan = each!(&mut self.unit, |u| u.from_far_end(bytes));
        self.take(plan);
    }

    /// The tick clock reads `now_ns`: the session's clock starts on its first tick, and a session
    /// past its ceiling is told why and ends. Answers whether it now owes a frame or its end.
    pub fn tick(&mut self, now_ns: u64) -> bool {
        let started = *self.started_ns.get_or_insert(now_ns);
        let Some(ceiling) = self.ceiling_secs else {
            return false;
        };
        if self.ending || now_ns.saturating_sub(started) < ceiling.saturating_mul(NS_PER_SEC) {
            return false;
        }
        let plan = each!(&mut self.unit, |u| u.close_at_ceiling(ceiling));
        self.take(plan);
        true
    }

    /// The next answer: one queued frame, or, with none queued, the session's end when it ended.
    /// A caller-side answer (`near`) carries a frame toward either side, oldest first, and the
    /// session's end; a far-side answer reaches only the caller, so it carries the oldest frame
    /// bound for the caller and never the end (the caller side answers that, once collected).
    pub fn next(&mut self, near: bool) -> Emit {
        let at = if near {
            (!self.queued.is_empty()).then_some(0)
        } else {
            self.queued
                .iter()
                .position(|(side, _)| *side == Side::Caller)
        };
        match at.and_then(|i| self.queued.remove(i)) {
            Some(frame) => Emit {
                frame: Some(frame),
                done: false,
            },
            None => Emit {
                frame: None,
                done: near && self.ending,
            },
        }
    }

    /// Whether the session owes an answer of its own: a queued frame, or its end.
    #[must_use]
    pub fn ready(&self) -> bool {
        !self.queued.is_empty() || self.ending
    }

    /// The session's cumulative units, as the door reports them: (class index, amount), the zero
    /// classes left out; the fee unit is `1` once the far end has answered, unless the session is
    /// attached to a call whose SDP offer already reported the call's fee.
    #[must_use]
    pub fn units(&self) -> Vec<(u32, u64)> {
        let units: CumulativeUnits = each!(&self.unit, |u| u.units());
        let mut out = units.reported();
        if self.answered && self.call_id.is_none() {
            out.push((PER_SESSION_CLASS, 1));
        }
        out
    }
}

/// THE SECTION'S TOKEN CEILINGS over the session params `locked`: the per-response output cap is
/// `streams.max_output_tokens`, or the session's own where that is lower, and the context window is
/// `streams.context_window_tokens`. Each dialect states them in its own keys when the session is
/// locked (`max_output_tokens` and `truncation` on OpenAI Realtime, `generationConfig.maxOutputTokens`
/// and `contextWindowCompression` on Gemini Live).
#[must_use]
pub fn with_ceilings(mut locked: SessionConfig, cfg: &StreamsCfg) -> SessionConfig {
    let cap = match locked.max_output_tokens {
        Some(MaxOutputTokens::Limit(n)) => n.min(cfg.max_output_tokens),
        _ => cfg.max_output_tokens,
    };
    locked.max_output_tokens = Some(MaxOutputTokens::Limit(cap));
    locked.context_window_tokens = Some(cfg.context_window_tokens);
    locked
}

/// THE SESSION-OPEN PARAMS A COMMITTED HOOK REWRITE PRODUCED (the session-open hook projection the
/// retired streams crate applied, verbatim: ARCHITECT Q-L5B-PROJECT), or why they cannot be used.
/// The rewrite is a JSON object PATCHED over the `locked` params as the hook was shown them: a key
/// the hook named wins, a key it did not name keeps the locked value (naming a key with `null`
/// clears it). The merge comes before the decode, so a patch that names one bad field refuses the
/// open; so does output that is not JSON, or JSON that is not an object. A rewrite busbar cannot
/// read back is never opened as if it had not been made.
///
/// # Errors
///
/// The output is not JSON, not an object, or not a session config once merged.
pub fn committed_session_config(
    locked: &SessionConfig,
    rewrite: &[u8],
) -> Result<SessionConfig, String> {
    let patch: serde_json::Value =
        serde_json::from_slice(rewrite).map_err(|e| format!("the output is not JSON: {e}"))?;
    let serde_json::Value::Object(patch) = patch else {
        return Err("the output is JSON but not a session-params object".to_string());
    };
    let Ok(serde_json::Value::Object(mut merged)) = serde_json::to_value(locked) else {
        return Err(
            "the plane's own locked session params did not project to an object".to_string(),
        );
    };
    merged.extend(patch);
    serde_json::from_value::<SessionConfig>(serde_json::Value::Object(merged))
        .map_err(|e| format!("the output is not a session config: {e}"))
}

/// The door's live sessions, by the kernel's stream.
#[derive(Debug, Default)]
pub struct Sessions {
    live: BTreeMap<u64, Live>,
    /// The session-open params a hook rewrote, by the unit (= the stream) the session opens on,
    /// until its first piece opens it.
    rewritten: BTreeMap<u64, SessionConfig>,
    /// The tickets each session's pieces crossed on (one per side), so a cancel on either ends it.
    tickets: HashMap<Ticket, u64>,
    /// The streams whose sessions ended most recently, oldest first (at most [`MAX_SESSIONS`]): a
    /// cut ends a session before the kernel asks for the cut's frame.
    ended: VecDeque<u64>,
    /// Busbar's calls placed by sessions that ended, until the door frees them
    /// ([`Sessions::take_ended_calls`]).
    ended_calls: Vec<String>,
}

impl Sessions {
    /// Stream `stream`'s session, opened on `door` under `cfg` at its first piece. `None` for a
    /// door that answers one request, a stream past [`MAX_SESSIONS`], or a stream whose session
    /// ended: a caller's piece after its call ended (a second Twilio `stop`) is refused, as 1.5.5
    /// refused it, never opened as a fresh session that ends and settles a second time.
    pub fn get_or_open(&mut self, stream: u64, door: Door, cfg: &StreamsCfg) -> Option<&mut Live> {
        if !self.live.contains_key(&stream) {
            if self.live.len() >= MAX_SESSIONS || self.ended.contains(&stream) {
                return None;
            }
            let rewritten = self.rewritten.remove(&stream);
            self.live
                .insert(stream, Live::open_locked(door, cfg, rewritten)?);
        }
        self.live.get_mut(&stream)
    }

    /// The session-open params a hook rewrote for the session unit `unit` opens (kept until its
    /// first piece; the oldest dropped past [`MAX_SESSIONS`] kept).
    pub fn rewrite(&mut self, unit: u64, params: SessionConfig) {
        while self.rewritten.len() >= MAX_SESSIONS {
            if self.rewritten.pop_first().is_none() {
                break;
            }
        }
        self.rewritten.insert(unit, params);
    }

    /// The session-open params stated for the session unit `unit` opens (a hook's rewrite, or a
    /// ticket's record), until its first piece opens it.
    #[must_use]
    pub fn rewritten(&self, unit: u64) -> Option<&SessionConfig> {
        self.rewritten.get(&unit)
    }

    /// Stream `stream`'s session, when one is open.
    pub fn get(&mut self, stream: u64) -> Option<&mut Live> {
        self.live.get_mut(&stream)
    }

    /// A piece of stream `stream` crossed on `ticket`.
    pub fn crossed(&mut self, stream: u64, ticket: Ticket) {
        if ticket != Ticket::NONE {
            self.tickets.insert(ticket, stream);
        }
    }

    /// Stream `stream`'s session is over: it and its tickets are forgotten.
    pub fn close(&mut self, stream: u64) {
        if let Some(live) = self.live.remove(&stream) {
            while self.ended.len() >= MAX_SESSIONS {
                self.ended.pop_front();
            }
            self.ended.push_back(stream);
            self.ended_calls.extend(live.placed);
        }
        self.rewritten.remove(&stream);
        self.tickets.retain(|_, s| *s != stream);
    }

    /// Whether the session on `ticket`, or stream `stream` (`0` = none named), had its far end
    /// answer.
    #[must_use]
    pub fn far_answered(&self, ticket: Ticket, stream: u64) -> bool {
        self.tickets
            .get(&ticket)
            .copied()
            .into_iter()
            .chain((stream != 0).then_some(stream))
            .any(|s| self.live.get(&s).is_some_and(|live| live.answered))
    }

    /// Busbar's calls placed by the sessions that ended since this was last asked.
    pub fn take_ended_calls(&mut self) -> Vec<String> {
        std::mem::take(&mut self.ended_calls)
    }

    /// The op on `ticket` is cancelled: the session it serves, if any, is over. Answers whether
    /// one was.
    pub fn cancel(&mut self, ticket: Ticket) -> bool {
        match self.tickets.get(&ticket).copied() {
            Some(stream) => {
                self.close(stream);
                true
            }
            None => false,
        }
    }

    /// The tick clock reads `now_ns`: every session's clock and ceiling. Answers whether any
    /// session now owes an answer of its own.
    pub fn tick(&mut self, now_ns: u64) -> bool {
        let mut owed = false;
        for live in self.live.values_mut() {
            owed |= live.tick(now_ns);
        }
        owed
    }

    /// The streams whose sessions owe an answer of their own, up to `cap`.
    #[must_use]
    pub fn ready(&self, cap: usize) -> Vec<u64> {
        self.live
            .iter()
            .filter(|(_, l)| l.ready())
            .map(|(s, _)| *s)
            .take(cap)
            .collect()
    }

    /// How many streams' sessions owe an answer of their own.
    #[cfg(test)]
    #[must_use]
    pub fn ready_count(&self) -> usize {
        self.live.values().filter(|l| l.ready()).count()
    }

    /// Whether `stream` is, or was until recently, a live session's: a refusal of it is told in
    /// the session's stream, never as the answer to an upgrade.
    #[must_use]
    pub fn served(&self, stream: u64) -> bool {
        self.live.contains_key(&stream) || self.ended.contains(&stream)
    }

    /// How many sessions are live.
    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.live.len()
    }
}

#[cfg(test)]
#[path = "tests/session_door_tests.rs"]
mod tests;
