// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Live sessions through the door: a session is one unit with one line.

use std::borrow::Cow;
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
    /// The session's opening call ([`Live::open_far`]) is still owed: the first far-bound frame is
    /// the body of the open that dials the far end, not a message on it ([`Live::take_opening`]).
    opening: bool,
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
    /// else its name), on the plain realtime and telephony doors; `None` on the sideband
    /// (it names its call) and on the Gemini Live socket (its model rides the setup frame).
    dial_model: Option<String>,
    dial_path: &'static str,
    cached_target: String,
    /// The call placed by an SDP session, released when that session ends.
    placed: Option<String>,
    /// A sideband's payload-free initial dial is still owed on its empty opening crossing.
    attach_opening: bool,
    /// What the frame a caller-side answer is writing still owes the reply buffer.
    pub(crate) near: crate::piece::Owed,
    /// What the frame a far-side answer is writing still owes the reply buffer.
    pub(crate) far: crate::piece::Owed,
    /// A caller-side answer that did not fit its buffers: the re-call carries the same piece,
    /// which is answered from here rather than read twice.
    pub(crate) short_near: Option<Emit>,
    /// The same, for a far-side answer.
    pub(crate) short_far: Option<Emit>,
    /// The open's head (its status and fields) a short caller-side answer did not write: the
    /// re-call answers it with the parked emit (the open happened once, and is answered whole).
    pub(crate) short_head: Option<(u16, Vec<(&'static str, String)>)>,
    /// The tickets the session's pieces crossed on, so its close forgets only its own.
    tickets: Vec<Ticket>,
}

impl Live {
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

    /// Open a session locked to `rewritten` when a hook supplied session parameters.
    /// ([`Self::locked`]) this session opens under, with the section's two token ceilings
    /// applied ([`with_ceilings`]).
    #[must_use]
    pub fn open_locked(
        door: Door,
        cfg: &StreamsCfg,
        rewritten: Option<SessionConfig>,
    ) -> Option<Live> {
        let ceiling = cfg.session_max_secs.map(|s| u64::from(s.get()));

        let locked = with_ceilings(
            cfg.wire_session(rewritten.unwrap_or_else(|| Self::locked(door, cfg))),
            cfg,
        );
        if door == Door::Gemini && locked.validate_gemini().is_err() {
            return None;
        }
        let mut unit = match door {
            Door::Sideband | Door::Realtime | Door::Sdp => {
                Unit::Realtime(SessionUnit::open(OpenAiRealtimeCodec, locked, false))
            }
            Door::Twilio => Unit::Realtime(SessionUnit::open(OpenAiRealtimeCodec, locked, true)),
            Door::Gemini => Unit::Gemini(SessionUnit::open(GeminiLiveCodec, locked, false)),
            Door::Mint => return None,
        };
        let mut queued = VecDeque::new();
        if door == Door::Realtime {
            queued.extend(each!(&mut unit, |u| u.initial_lock()).map(|f| (Side::FarEnd, f)));
        }
        let dial_model = match door {
            Door::Realtime | Door::Twilio => cfg
                .upstream_model
                .clone()
                .or_else(|| cfg.session.model.clone()),
            _ => None,
        };
        let dial_path = door.route(None)?.target;
        let cached_target = dial_model.as_deref().map_or_else(
            || dial_path.to_string(),
            |model| {
                format!(
                    "{dial_path}?model={}",
                    url::form_urlencoded::byte_serialize(model.as_bytes()).collect::<String>()
                )
            },
        );
        Some(Live {
            unit,
            queued,
            opening: false,
            ending: false,
            ceiling_secs: ceiling,
            started_ns: None,
            answered: false,
            accepted: false,
            call_id: None,
            dial_model,
            dial_path,
            cached_target,
            placed: None,
            attach_opening: door == Door::Sideband,
            near: crate::piece::Owed::default(),
            far: crate::piece::Owed::default(),
            short_near: None,
            short_far: None,
            short_head: None,
            tickets: Vec::new(),
        })
    }

    /// The provider's call this session attaches to (the sideband door's `{call_id}`).
    pub fn attach(&mut self, call_id: String) {
        self.cached_target = format!("{}?call_id={}", self.dial_path, call_id);
        self.call_id = Some(call_id);
    }

    /// The far end accepted the session's upgrade: it answered the session, and its fee unit is
    /// incurred.
    pub fn accept(&mut self) {
        self.accepted = true;
        self.answered = true;
    }

    /// Whether the far end accepted the session's upgrade.
    #[must_use]
    pub fn accepted(&self) -> bool {
        self.accepted
    }

    /// Whether the session keeps a wall-clock ceiling of its own.
    #[must_use]
    pub fn has_ceiling(&self) -> bool {
        self.ceiling_secs.is_some()
    }

    /// The provider's call this session is attached to, if any.
    #[must_use]
    pub fn call_id(&self) -> Option<&str> {
        self.call_id.as_deref()
    }

    /// The target the session dials under the member's base URL: the dialect's socket `path`;
    /// for a session attached to a provider's call, that call only (`?call_id=`, the provider's
    /// sideband connection to the call's own session); for a plain or telephony session, the
    /// session's model (`?model=`, form-escaped, as OpenAI's own WS surface takes it).
    #[must_use]
    pub fn dial_target<'a>(&'a self, path: &'a str) -> Cow<'a, str> {
        if path == self.dial_path {
            return Cow::Borrowed(&self.cached_target);
        }
        Cow::Owned(match (&self.call_id, &self.dial_model) {
            (Some(id), _) => format!("{path}?call_id={id}"),
            (None, Some(model)) => format!(
                "{path}?model={}",
                url::form_urlencoded::byte_serialize(model.as_bytes()).collect::<String>()
            ),
            (None, None) => path.to_string(),
        })
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

    /// THE SESSION'S OPENING CALL: `frame`, owed the far end before anything else (the SDP
    /// door's request, which places busbar's own call and so dials the session's far end at its
    /// open).
    pub fn open_far(&mut self, frame: Vec<u8>) {
        self.queued.push_front((Side::FarEnd, frame));
        self.opening = true;
    }

    /// Whether the far-bound frame being written is the session's opening call (once): the body
    /// of the open that dials the far end, never a text message on it.
    pub fn take_opening(&mut self) -> bool {
        std::mem::take(&mut self.opening)
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
        if std::mem::take(&mut self.attach_opening) && bytes.is_empty() && !last {
            self.open_far(Vec::new());
        }
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

    /// A far-end piece.
    #[allow(clippy::wrong_self_convention)]
    pub fn from_far_end(&mut self, bytes: &[u8]) {
        let plan = each!(&mut self.unit, |u| u.from_far_end(bytes));
        self.take(plan);
    }

    /// The tick clock reads `now_ns`: the session's clock starts on its first tick, and a
    /// session past its ceiling is told why and ends.
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

    /// The session's cumulative units, as the door reports them: (class index, amount), the
    /// zero classes left out; the fee unit is `1` once the far end has answered, unless the
    /// session is attached to a call whose SDP offer already reported the call's fee.
    #[must_use]
    pub fn units(&self) -> Vec<(u32, u64)> {
        let units: CumulativeUnits = each!(&self.unit, |u| u.units());
        let mut out = units.reported();
        if self.call_id.is_some() {
            // Provider events mirror the SDP data channel, but caller audio injected on this
            // sideband was admitted only here and still contributes its own ingress seconds.
            out.retain(|(class, _)| {
                crate::session_unit::CLASSES[*class as usize] == crate::meta::CLASS_AUDIO_SECONDS_IN
            });
            return out;
        }
        if self.answered && self.call_id.is_none() {
            out.push((PER_SESSION_CLASS, 1));
        }
        out
    }
}

/// The section's token ceilings over the session params `locked`: the per-response output cap
/// is `streams.max_output_tokens`, or the session's own where that is lower, and the context
/// window is `streams.context_window_tokens`.
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

/// The session-open params a committed hook rewrite produced, or why they cannot be used.
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

/// Whether `rewrite` is the kernel's rewrite crossing (`{"messages":[..],"tools":[..]}`, the one
/// shape core's request-stage hooks hand a plane), and if so whether it names any message. A live
/// session frames no conversation a rewrite's messages could replace, so such a rewrite is never
/// merged into the session's params: one naming no message is no rewrite, and one naming messages
/// does not apply to the session (`PROJECT_REWRITE_NOT_APPLICABLE`). `None`: any other shape, read
/// as a session-params patch ([`committed_session_config`]).
#[must_use]
pub fn hook_crossing(rewrite: &[u8]) -> Option<bool> {
    match serde_json::from_slice::<serde_json::Value>(rewrite).ok()? {
        serde_json::Value::Object(crossing) => crossing
            .get("messages")
            .map(|m| m.as_array().is_none_or(|m| !m.is_empty())),
        _ => None,
    }
}

/// Name a rejected session-open rewrite after its projection has completed.
pub fn report_unapplied_rewrite(why: &str) {
    tracing::warn!(
        "the session-open hook's rewrite was not applied ({why}); the session opens with \
         the params the hook was shown"
    );
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
    // Only a completed reply permits a later payload-free far terminal. Cancellation and
    // refusal retain the stream identity but never grant that permission.
    ended: VecDeque<(u64, Option<Door>)>,
    /// Busbar's calls placed by sessions that ended, until the door frees them
    /// ([`Sessions::take_ended_calls`]).
    ended_calls: Vec<String>,
}

impl Sessions {
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.live.len()
    }

    #[cfg(test)]
    pub fn ready_count(&self) -> usize {
        self.ready(usize::MAX).len()
    }

    /// Stream `stream`'s session, opened on `door` under `cfg` at its first piece. `None` for a
    /// door that answers one request, a stream past [`MAX_SESSIONS`], or a stream whose session
    /// ended: a caller's piece after its call ended (a second Twilio `stop`) is refused,
    /// never opened as a fresh session that ends and settles a second time.
    pub fn get_or_open(&mut self, stream: u64, door: Door, cfg: &StreamsCfg) -> Option<&mut Live> {
        if !self.live.contains_key(&stream) {
            if self.live.len() >= MAX_SESSIONS || self.ended(stream) {
                return None;
            }
            let rewritten = self.rewritten.remove(&stream);
            self.live
                .insert(stream, Live::open_locked(door, cfg, rewritten)?);
        }
        self.live.get_mut(&stream)
    }

    /// Keep the session-open params a hook rewrote (or a ticket locks) for the session unit
    /// `unit` opens, until its first piece.
    #[must_use]
    pub fn rewrite(&mut self, unit: u64, params: SessionConfig) -> bool {
        if self.rewritten.len() >= MAX_SESSIONS && !self.rewritten.contains_key(&unit) {
            return false;
        }
        self.rewritten.insert(unit, params);
        true
    }

    /// The session-open params stated for the session unit `unit` opens (a hook's rewrite, or a
    /// ticket's record), until its first piece opens it.
    #[must_use]
    pub fn rewritten(&self, unit: u64) -> Option<&SessionConfig> {
        self.rewritten.get(&unit)
    }

    /// Whether as many sessions are live as the door keeps ([`MAX_SESSIONS`]): a new one is
    /// refused.
    #[must_use]
    pub fn full(&self) -> bool {
        self.live.len() >= MAX_SESSIONS
    }

    /// Stream `stream`'s session, when one is open.
    pub fn get(&mut self, stream: u64) -> Option<&mut Live> {
        self.live.get_mut(&stream)
    }

    /// A piece of stream `stream` crossed on `ticket`.
    pub fn crossed(&mut self, stream: u64, ticket: Ticket) {
        if ticket == Ticket::NONE {
            return;
        }
        let Some(live) = self.live.get_mut(&stream) else {
            return;
        };
        if self.tickets.insert(ticket, stream) != Some(stream) {
            live.tickets.push(ticket);
        }
    }

    /// Stream `stream`'s session is over: it and its tickets are forgotten.
    pub fn close(&mut self, stream: u64) {
        self.end(stream, None);
    }

    /// The reply completed on `door`; the far side may still report its transport terminal.
    pub fn complete(&mut self, stream: u64, door: Door) {
        self.end(stream, Some(door));
    }

    fn end(&mut self, stream: u64, completed: Option<Door>) {
        if let Some(live) = self.live.remove(&stream) {
            while self.ended.len() >= MAX_SESSIONS {
                self.ended.pop_front();
            }
            self.ended.push_back((stream, completed));
            for ticket in &live.tickets {
                if self.tickets.get(ticket) == Some(&stream) {
                    self.tickets.remove(ticket);
                }
            }
            self.ended_calls.extend(live.placed);
        } else if completed.is_none() {
            // Cancellation/refusal ends even the completed reply's cleanup custody.
            if let Some((_, door)) = self.ended.iter_mut().find(|(id, _)| *id == stream) {
                *door = None;
            }
        }
        self.rewritten.remove(&stream);
    }

    /// Whether `stream`'s session already ended: a late piece of it opens nothing.
    #[must_use]
    pub fn ended(&self, stream: u64) -> bool {
        self.ended.iter().any(|(id, _)| *id == stream)
    }

    /// Whether this exact stream and door completed their reply. The existing bounded
    /// recently-ended table retains no live session, tickets or metering state.
    #[must_use]
    pub fn completed(&self, stream: u64, door: Door) -> bool {
        self.ended.contains(&(stream, Some(door)))
    }

    /// Whether any live session keeps a wall-clock ceiling: the door's tick must keep running.
    #[must_use]
    pub fn any_ceiling(&self) -> bool {
        self.live.values().any(Live::has_ceiling)
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

    /// The op on `ticket` is cancelled: the session it serves, if any, is over.
    pub fn cancel(&mut self, ticket: Ticket) -> bool {
        match self.tickets.get(&ticket).copied() {
            Some(stream) => {
                self.close(stream);
                true
            }
            None => false,
        }
    }

    /// The tick clock reads `now_ns`: every session's clock and ceiling.
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

    /// Whether `stream` is, or was until recently, a live session's: a refusal of it is told in
    /// the session's stream, never as the answer to an upgrade.
    #[must_use]
    pub fn served(&self, stream: u64) -> bool {
        self.live.contains_key(&stream) || self.ended(stream)
    }
}

#[cfg(test)]
#[path = "tests/session_door_tests.rs"]
mod tests;
