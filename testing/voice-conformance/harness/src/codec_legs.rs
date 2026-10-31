// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE FIXTURE LEGS — `spec-per-dialect`, `replay` — read by the harness's own decoder
//! ([`crate::decode`]) AND driven through the door's session pieces.
//!
//! Each fixture is judged twice. The DECODE half: a fixture must decode to IR or be a documented
//! drop (spec), a captured transcript must re-derive its concept skeleton (replay). The DOOR half
//! pushes the same wire bytes through a live session on the door that
//! speaks the dialect — the caller's frames as caller pieces, the far end's as far-end pieces — on
//! the linked and the dropped door, and holds what the door relays to the SESSION RULES the plane
//! states (`session_pump`, Law 5's relay): a caller frame reaches the far end as it arrived,
//! except that a frame configuring the session or asking for a response takes the session's
//! locked keys (the plane's governed splice, `busbar_plane_streaming::splice`); a far-end frame
//! reaches the caller as it arrived, every one of them (a usage report also becomes reported
//! units), and nothing goes to the far end on the caller's behalf (Law 11). Compared BYTE FOR BYTE,
//! frame by frame, per side and in order (Law 5, `BUSBAR-1.6.0.md` l.1903: a relay "sends the
//! client's bytes with governed byte-level splices"): a frame the reader decodes to nothing, or a
//! member it does not model, is held to the same rule as a modelled one. The reader serves only the
//! units. The expectation is computed from the wire by the splice, never by the door.

use std::fs;
use std::path::Path;

use busbar_contract::abi::mechanism::call::Outcome;
use busbar_contract::abi::plane::{FROM_CALLER, FROM_FAR_END, UNITS_REPORTED};
use busbar_plane_streaming::codec::ir::codec::OpenAiRealtimeCodec;
use busbar_plane_streaming::codec::ir::config::SessionConfig;
use busbar_plane_streaming::codec::ir::GeminiLiveCodec;
use busbar_plane_streaming::driven::{GEMINI_SOCKET_PATH, REALTIME_SOCKET_PATH};
use busbar_plane_streaming::plane_door::{read_settings, RIDES_LIVE_SOCKET, RIDES_REALTIME_SOCKET};
use busbar_plane_streaming::session_door::with_ceilings;
use busbar_plane_streaming::session_door::PER_SESSION_CLASS;
use busbar_plane_streaming::splice::{splice, LockedKeys};
use busbar_plugin_loader::dispatch::kinds::plane::Plane;
use busbar_plugin_loader::dispatch::Plugin;
use serde_json::Value;

use crate::decode::{
    DecodeState, GeminiLive, IrClientEvent, IrDuplexControl, IrDuplexTool, IrServerEvent,
    OpenAiRealtime,
};
use crate::door::{
    dials_socket, judge_planted, open, result, Emission, Log, Plant, Rig, Session, PUBLIC, SESSION,
};

// ── the two dialects ────────────────────────────────────────────────────────────────────────────

/// A duplex dialect the plane dials, and the session door that speaks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    /// OpenAI Realtime: plain server-to-server OpenAI Realtime over WS (claim 5).
    OpenAi,
    /// Gemini Live: the Gemini socket (claim 3).
    Gemini,
}

impl Dialect {
    /// The dialect a leg names.
    pub fn of(name: &str) -> Option<Self> {
        match name {
            "openai" => Some(Dialect::OpenAi),
            "gemini" => Some(Dialect::Gemini),
            _ => None,
        }
    }

    /// Its name in the legs.
    pub const fn name(self) -> &'static str {
        match self {
            Dialect::OpenAi => "openai",
            Dialect::Gemini => "gemini",
        }
    }

    /// The door claim that opens a session in it.
    pub const fn claim(self) -> u32 {
        match self {
            Dialect::OpenAi => 5,
            Dialect::Gemini => 3,
        }
    }

    /// The declared need its far frames ride.
    pub const fn ride(self) -> u32 {
        match self {
            Dialect::OpenAi => RIDES_REALTIME_SOCKET,
            Dialect::Gemini => RIDES_LIVE_SOCKET,
        }
    }

    /// The far end's socket, under the member's base URL.
    pub const fn socket(self) -> &'static str {
        match self {
            Dialect::OpenAi => REALTIME_SOCKET_PATH,
            Dialect::Gemini => GEMINI_SOCKET_PATH,
        }
    }

    /// Read a caller frame (the harness's own decoder, [`crate::decode`]).
    pub fn read_up(self, frame: &[u8], st: &mut DecodeState) -> Vec<IrClientEvent> {
        match self {
            Dialect::OpenAi => OpenAiRealtime.read_up(frame, st),
            Dialect::Gemini => GeminiLive.read_up(frame, st),
        }
    }

    /// Read a far-end frame (the harness's own decoder, [`crate::decode`]).
    pub fn read_down(self, frame: &[u8], st: &mut DecodeState) -> Vec<IrServerEvent> {
        match self {
            Dialect::OpenAi => OpenAiRealtime.read_down(frame, st),
            Dialect::Gemini => GeminiLive.read_down(frame, st),
        }
    }
}

fn bytes_of(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).expect("serialize wire value")
}

// ── the shared normal form (the old harness's, verbatim in meaning) ─────────────────────────────

/// Both direction-split IR enums collapsed onto one list, so concepts compare uniformly.
#[derive(Clone, Debug, PartialEq)]
pub enum Norm {
    /// Config essentials only: instructions|voice|tools|max (the survivable fields).
    Config(String),
    /// The configured modalities.
    ConfigModalities(Vec<String>),
    /// The session's acknowledgement.
    Connect,
    /// Uplink audio.
    AudioUp(Vec<u8>),
    /// Downlink audio.
    AudioDown(Vec<u8>),
    /// An item's audio is done.
    AudioDone,
    /// A conversation item.
    Item(Value),
    /// Speech began.
    SpeechStart,
    /// Speech stopped.
    SpeechStop,
    /// A truncate, at the ms heard.
    Truncate(u64),
    /// Commit the uplink.
    Commit,
    /// Clear the uplink.
    Clear,
    /// Ask for a response.
    ResponseCreate,
    /// Cancel the response.
    ResponseCancel,
    /// Delete an item.
    ItemDelete,
    /// Usage: audio in, audio out, text in, text out.
    Usage(u64, u64, u64, u64),
    /// A rate-limit report.
    RateLimits,
    /// An error: code, message.
    Error(String, String),
    /// A call opened: id, name.
    ToolOpen(String, String),
    /// A call's arguments: id, parsed.
    ToolArgs(String, Value),
    /// A call closed: id.
    ToolClose(String),
    /// A call's result: id, parsed output.
    ToolResult(String, Value),
}

fn cfg_essentials(config: &SessionConfig) -> Norm {
    Norm::Config(format!(
        "{:?}|{:?}|{}|{:?}",
        config.instructions,
        config.voice,
        config.tools.len(),
        config.max_output_tokens
    ))
}

fn norm_tool(t: &IrDuplexTool) -> Norm {
    match t {
        IrDuplexTool::CallOpen { call_id, name, .. } => {
            Norm::ToolOpen(call_id.clone(), name.clone())
        }
        IrDuplexTool::CallArgs {
            call_id,
            json_delta,
            ..
        } => Norm::ToolArgs(
            call_id.clone(),
            serde_json::from_slice(json_delta).unwrap_or(Value::Null),
        ),
        IrDuplexTool::CallClose { call_id, .. } => Norm::ToolClose(call_id.clone()),
        IrDuplexTool::CallResult {
            call_id, output, ..
        } => Norm::ToolResult(
            call_id.clone(),
            serde_json::from_slice(output).unwrap_or(Value::Null),
        ),
    }
}

/// Caller events, normalized.
pub fn norm_up(evs: &[IrClientEvent]) -> Vec<Norm> {
    let mut out = Vec::new();
    for e in evs {
        match e {
            IrClientEvent::AudioFrame(f) => out.push(Norm::AudioUp(f.media.to_vec())),
            IrClientEvent::Tool(t) => out.push(norm_tool(t)),
            IrClientEvent::Control(c) => match c {
                IrDuplexControl::SessionConfigure { config } => {
                    out.push(cfg_essentials(config));
                    out.push(Norm::ConfigModalities(config.modalities.clone()));
                }
                IrDuplexControl::ItemCreate { item } => out.push(Norm::Item(item.clone())),
                IrDuplexControl::ItemTruncate {
                    audio_played_ms, ..
                } => out.push(Norm::Truncate(*audio_played_ms)),
                IrDuplexControl::InputAudioCommit => out.push(Norm::Commit),
                IrDuplexControl::InputAudioClear => out.push(Norm::Clear),
                IrDuplexControl::ResponseCreate { .. } => out.push(Norm::ResponseCreate),
                IrDuplexControl::ResponseCancel => out.push(Norm::ResponseCancel),
                IrDuplexControl::ItemDelete { .. } => out.push(Norm::ItemDelete),
            },
        }
    }
    out
}

/// Far-end events, normalized.
pub fn norm_down(evs: &[IrServerEvent]) -> Vec<Norm> {
    let mut out = Vec::new();
    for e in evs {
        match e {
            IrServerEvent::SessionCreated { .. } => out.push(Norm::Connect),
            IrServerEvent::Tool(t) => out.push(norm_tool(t)),
            IrServerEvent::SpeechStarted { .. } => out.push(Norm::SpeechStart),
            IrServerEvent::SpeechStopped { .. } => out.push(Norm::SpeechStop),
            IrServerEvent::AudioFrame(f) => out.push(Norm::AudioDown(f.media.to_vec())),
            IrServerEvent::AudioDone { .. } => out.push(Norm::AudioDone),
            IrServerEvent::Usage(u) => {
                out.push(Norm::Usage(u.audio_in, u.audio_out, u.text_in, u.text_out));
            }
            IrServerEvent::RateLimits => out.push(Norm::RateLimits),
            IrServerEvent::Error { code, message } => {
                out.push(Norm::Error(code.clone(), message.clone()));
            }
        }
    }
    out
}

/// A concept's tag, for the replay skeleton.
fn tag(n: &Norm) -> &'static str {
    match n {
        Norm::Config(_) => "config",
        Norm::ConfigModalities(_) => "config-modalities",
        Norm::Connect => "connect",
        Norm::AudioUp(_) => "audio-in",
        Norm::AudioDown(_) => "audio-out",
        Norm::AudioDone => "audio-done",
        Norm::Item(_) => "item",
        Norm::SpeechStart => "speech-start",
        Norm::SpeechStop => "speech-stop",
        Norm::Truncate(_) => "truncate",
        Norm::Commit => "commit",
        Norm::Clear => "clear",
        Norm::ResponseCreate => "response-create",
        Norm::ResponseCancel => "cancel",
        Norm::ItemDelete => "item-delete",
        Norm::Usage(..) => "usage",
        Norm::RateLimits => "rate-limits",
        Norm::Error(..) => "error",
        Norm::ToolOpen(..) => "tool-open",
        Norm::ToolArgs(..) => "tool-args",
        Norm::ToolClose(..) => "tool-close",
        Norm::ToolResult(..) => "tool-result",
    }
}

/// `expected` appears, in order, as a subsequence of `tags`.
fn ordered_subsequence(tags: &[&str], expected: &[&str]) -> Result<(), String> {
    let mut i = 0;
    for want in expected {
        match tags[i..].iter().position(|t| t == want) {
            Some(off) => i += off + 1,
            None => {
                return Err(format!(
                    "expected concept '{want}' not found after position {i} in {tags:?}"
                ))
            }
        }
    }
    Ok(())
}

// ── wires, and what a session does with them ────────────────────────────────────────────────────

/// Which side of the session a frame comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// The caller.
    Up,
    /// The far end.
    Down,
}

/// One frame pushed to a session.
#[derive(Clone, Debug)]
pub struct Wire {
    /// Its side.
    pub dir: Dir,
    /// Its bytes.
    pub bytes: Vec<u8>,
}

/// Drive `wires` through ONE session on `p`'s door for `dialect`.
pub fn relay(p: &Plugin<Plane>, dialect: Dialect, wires: &[Wire]) -> Log {
    let opened = open(p, SESSION, Some(PUBLIC));
    let mut s = Session::open(p, 77, dialect.claim());
    if opened.outcome != Outcome::Ready {
        s.log.anomalies.push(format!("open {:?}", opened.outcome));
    }
    for w in wires {
        let from = match w.dir {
            Dir::Up => FROM_CALLER,
            Dir::Down => FROM_FAR_END,
        };
        s.push(from, 0, &w.bytes);
    }
    s.log
}

/// What the session rules say the door must relay for a sequence of wires: the exact frames each
/// side is owed, in order, and the token units the far end's reports add up to.
pub struct Expected {
    far: Vec<Vec<u8>>,
    caller: Vec<Vec<u8>>,
    tokens: Vec<(u32, u64)>,
    answered: bool,
}

/// The caller frame `bytes` as the session's lock leaves it: a frame configuring the session, or
/// asking for a response, takes the locked keys; any other frame is as it arrived. `None` for a
/// frame that is not JSON, or a governed frame that cannot be read as one object (it goes nowhere).
fn governed(dialect: Dialect, locked: &SessionConfig, bytes: &[u8]) -> Option<Vec<u8>> {
    serde_json::from_slice::<Value>(bytes).ok()?;
    let mut st = DecodeState::default();
    let evs = dialect.read_up(bytes, &mut st);
    let configures = evs.iter().any(|e| {
        matches!(
            e,
            IrClientEvent::Control(IrDuplexControl::SessionConfigure { .. })
        )
    });
    let responds = evs.iter().any(|e| {
        matches!(
            e,
            IrClientEvent::Control(IrDuplexControl::ResponseCreate { .. })
        )
    });
    let rule = match (dialect, configures, responds) {
        (Dialect::OpenAi, true, _) => Some(OpenAiRealtimeCodec.configure_rule(locked)),
        (Dialect::Gemini, true, _) => Some(GeminiLiveCodec.configure_rule(locked)),
        (Dialect::OpenAi, false, true) => OpenAiRealtimeCodec.response_rule(locked),
        (Dialect::Gemini, false, true) => GeminiLiveCodec.response_rule(locked),
        _ => None,
    };
    match rule {
        None => Some(bytes.to_vec()),
        Some(rule) => splice(bytes, &rule),
    }
}

/// The session rules over `wires`, computed from the wire (never by the door).
pub fn expect(dialect: Dialect, wires: &[Wire]) -> Expected {
    let cfg = read_settings(SESSION).expect("the leg's section reads");
    // The session as the door tells the provider it is locked: the model by its wire name, under
    // the section's two token ceilings.
    let locked = with_ceilings(cfg.wire_session(cfg.session.clone()), &cfg);
    let mut st = DecodeState::default();
    let mut x = Expected {
        far: Vec::new(),
        caller: Vec::new(),
        tokens: Vec::new(),
        answered: false,
    };
    let mut sums = [0_u64; 4];
    for w in wires {
        match w.dir {
            Dir::Up => {
                if let Some(bytes) = governed(dialect, &locked, &w.bytes) {
                    x.far.push(bytes);
                }
            }
            Dir::Down => {
                x.answered = true;
                let evs = dialect.read_down(&w.bytes, &mut st);
                for ev in &evs {
                    if let IrServerEvent::Usage(u) = ev {
                        for (sum, n) in
                            sums.iter_mut()
                                .zip([u.audio_in, u.audio_out, u.text_in, u.text_out])
                        {
                            *sum = sum.saturating_add(n);
                        }
                    }
                }
                x.caller.push(w.bytes.clone());
            }
        }
    }
    x.tokens = (0_u32..).zip(sums).filter(|(_, n)| *n != 0).collect();
    x
}

/// What the door relayed, read back by the codec: the far end's frames, the caller's, and every
/// concept's tag in the order the door emitted it.
pub struct Relayed {
    far: Vec<Norm>,
    caller: Vec<Norm>,
    tags: Vec<&'static str>,
}

/// Read `log`'s frames back under `dialect`.
pub fn decode_relay(dialect: Dialect, log: &Log) -> Relayed {
    let (mut su, mut sd) = (DecodeState::default(), DecodeState::default());
    let mut r = Relayed {
        far: Vec::new(),
        caller: Vec::new(),
        tags: Vec::new(),
    };
    for e in &log.emissions {
        let n = if e.to_far {
            norm_up(&dialect.read_up(&e.frame, &mut su))
        } else {
            norm_down(&dialect.read_down(&e.frame, &mut sd))
        };
        r.tags.extend(n.iter().map(tag));
        if e.to_far {
            r.far.extend(n);
        } else {
            r.caller.extend(n);
        }
    }
    r
}

/// The door's relay of a session against the session rules: every answer READY, no record row,
/// every far frame on the dialect's socket need, both sides' frames byte for byte the frames the
/// session rules owe them, the far end's token reports as reported units (and the session fee once
/// the far end answered).
pub fn relay_check(dialect: Dialect, log: &Log, x: &Expected) -> Result<String, String> {
    if !log.anomalies.is_empty() {
        return Err(format!(
            "the door did not answer every piece READY: {:?}",
            log.anomalies
        ));
    }
    if log.records != 0 {
        return Err(format!(
            "the door wrote {} record row(s) on a session",
            log.records
        ));
    }
    for e in log.emissions.iter().filter(|e| e.to_far) {
        if e.need != dialect.ride() || e.verb != "GET" || !dials_socket(&e.target, dialect.socket())
        {
            return Err(format!(
                "a far frame rode need {} to {} {}, not the {} socket (need {})",
                e.need,
                e.verb,
                e.target,
                dialect.name(),
                dialect.ride()
            ));
        }
    }
    for (to_far, side, want) in [(true, "far end", &x.far), (false, "caller", &x.caller)] {
        let sent: Vec<&[u8]> = log
            .emissions
            .iter()
            .filter(|e| e.to_far == to_far)
            .map(|e| e.frame.as_slice())
            .collect();
        if let Some(e) = frames_differ(&sent, want) {
            return Err(format!("the {side} {e}"));
        }
    }
    if let Some(u) = log.units.iter().find(|u| u.1 != UNITS_REPORTED) {
        return Err(format!("a unit was not reported by the far end: {u:?}"));
    }
    let tokens: Vec<(u32, u64)> = log
        .units
        .iter()
        .filter(|u| u.0 < 4)
        .map(|u| (u.0, u.2))
        .collect();
    if tokens != x.tokens {
        return Err(format!(
            "the token units reported {tokens:?}; the far end's reports add up to {:?}",
            x.tokens
        ));
    }
    let fee = log
        .units
        .iter()
        .any(|u| u.0 == PER_SESSION_CLASS && u.2 == 1);
    if fee != x.answered {
        return Err(format!(
            "the session fee unit reported {fee}, the far end answered {}",
            x.answered
        ));
    }
    Ok(format!(
        "the {} door relayed {} far / {} caller frame(s) byte for byte by the session rules, {} \
         token unit(s)",
        dialect.name(),
        x.far.len(),
        x.caller.len(),
        tokens.len()
    ))
}

/// How the frames a side was `sent` differ from the frames it is owed, if they do: the first frame
/// that differs, both ways, as text.
fn frames_differ(sent: &[&[u8]], want: &[Vec<u8>]) -> Option<String> {
    fn text(b: Option<&[u8]>) -> String {
        b.map_or_else(
            || "<none>".to_string(),
            |b| String::from_utf8_lossy(b).into_owned(),
        )
    }
    let n = sent.len().max(want.len());
    let i = (0..n).find(|&i| sent.get(i).copied() != want.get(i).map(Vec::as_slice))?;
    Some(format!(
        "was sent {} frame(s), owed {}; frame {i} was {} where the session rules owe {}",
        sent.len(),
        want.len(),
        text(sent.get(i).copied()),
        text(want.get(i).map(Vec::as_slice))
    ))
}

/// THE PLANTED WRONG DOOR for a relay: one that reports a token the far end never sent.
pub fn plant_units(log: &Log) -> Log {
    let mut planted = log.clone();
    planted.units.push((1, UNITS_REPORTED, 7));
    planted
}

/// THE PLANTED WRONG DOOR that RE-ENCODES instead of relaying bytes (Law 5, `BUSBAR-1.6.0.md`
/// l.1903: a relay "sends the client's bytes"): the first frame it emits loses a member no reader
/// models (`event_id`), or gains one, and is re-serialized. Every concept the reader reads is
/// unchanged, so only a byte-level comparison refuses it. A log with no frame gains one the session
/// never owed.
pub fn plant_rewrite(log: &Log) -> Log {
    let mut planted = log.clone();
    let first = planted.emissions.iter().enumerate().find_map(|(i, e)| {
        let v = serde_json::from_slice::<Value>(&e.frame).ok()?;
        v.as_object().cloned().map(|o| (i, o))
    });
    match first {
        Some((i, mut o)) => {
            if o.remove("event_id").is_none() {
                o.insert("event_id".to_string(), Value::from("evt_planted"));
            }
            planted.emissions[i].frame = bytes_of(&Value::Object(o));
        }
        None => planted.emissions.push(Emission {
            to_far: false,
            frame: b"{}".to_vec(),
            need: 0,
            verb: String::new(),
            target: String::new(),
        }),
    }
    planted
}

/// THE PLANTED WRONG DOOR that DROPS a frame: the last frame it owed is never emitted (a far-end
/// transcript, `goAway` or `response.output_item.done` the reader decodes to nothing is dropped as
/// silently as a modelled one). A log with no frame gains one the session never owed.
pub fn plant_drop(log: &Log) -> Log {
    let mut planted = log.clone();
    if planted.emissions.pop().is_none() {
        planted.emissions.push(Emission {
            to_far: true,
            frame: b"{}".to_vec(),
            need: 0,
            verb: String::new(),
            target: String::new(),
        });
    }
    planted
}

/// The relay legs' planted wrong doors: a phantom unit, a re-encoded frame, a dropped frame.
pub fn relay_plants() -> [Plant<'static, Log>; 3] {
    const UNITS: &dyn Fn(&Log) -> Log = &plant_units;
    const REWRITE: &dyn Fn(&Log) -> Log = &plant_rewrite;
    const DROP: &dyn Fn(&Log) -> Log = &plant_drop;
    [
        ("a token the far end never sent", UNITS),
        ("a frame re-encoded, not relayed", REWRITE),
        ("a frame dropped", DROP),
    ]
}

/// Judge one relay of `wires` on both doors, with `codec` (the codec half's verdict) folded in.
fn judge_relay(
    rig: &Rig,
    slice: &str,
    what: &str,
    dialect: Dialect,
    wires: &[Wire],
    codec: &Result<String, String>,
) -> bool {
    let x = expect(dialect, wires);
    let seen = rig.both(|p| relay(p, dialect, wires));
    judge_planted(
        slice,
        what,
        &seen,
        &|log: &Log| {
            let codec = codec.clone()?;
            let door = relay_check(dialect, log, &x)?;
            Ok(format!("codec: {codec}; door: {door}"))
        },
        &relay_plants(),
    )
}

// ══════════════════════════════════════════════════════════════════════════════════════════════
// spec-per-dialect
// ══════════════════════════════════════════════════════════════════════════════════════════════

/// Fixtures that legitimately decode to NOTHING, with the reason (the codec's documented drops).
fn drop_reason(dialect: Dialect, fixture: &str) -> Option<&'static str> {
    match (dialect, fixture) {
        (Dialect::Gemini, "goAway.json") => {
            Some("gemini_go_away: no OpenAI advance-disconnect twin (drop+warn)")
        }
        (Dialect::Gemini, "toolCallCancellation.json") => {
            Some("gemini_tool_call_cancellation: no OpenAI server-driven cancel (drop+warn)")
        }
        (Dialect::Gemini, "serverContent.inputTranscription.json") => {
            Some("input transcription side-channel: no shared IR home (drop+warn)")
        }
        (Dialect::Gemini, "serverContent.outputTranscription.json") => {
            Some("output transcription side-channel: no shared IR home (drop+warn)")
        }
        _ => None,
    }
}

/// A decoded fixture: the direction the codec recognized it in, and its IR.
enum Decoded {
    Up(Vec<IrClientEvent>),
    Down(Vec<IrServerEvent>),
    Empty,
}

fn decode(dialect: Dialect, frame: &[u8]) -> Decoded {
    let up = dialect.read_up(frame, &mut DecodeState::default());
    if !up.is_empty() {
        return Decoded::Up(up);
    }
    let down = dialect.read_down(frame, &mut DecodeState::default());
    if !down.is_empty() {
        return Decoded::Down(down);
    }
    Decoded::Empty
}

/// The decode half of one fixture, and the direction its frame is pushed from.
fn spec_codec(dialect: Dialect, fixture: &str, frame: &[u8]) -> (Result<String, String>, Dir) {
    match decode(dialect, frame) {
        Decoded::Up(ir) => (Ok(format!("decodes to {} IR event(s)", ir.len())), Dir::Up),
        Decoded::Down(ir) => (
            Ok(format!("decodes to {} IR event(s)", ir.len())),
            Dir::Down,
        ),
        // A documented drop is a far-end concept with no shared IR home: the reader decodes it to
        // nothing, and pushed from the far end the door relays its bytes to the caller unchanged
        // (a relay is byte-level, Law 5), which the door half holds it to.
        Decoded::Empty => match drop_reason(dialect, fixture) {
            Some(reason) => (Ok(format!("documented drop — {reason}")), Dir::Down),
            None => (
                Err("decoded to NO IR events and is not a documented drop".to_string()),
                Dir::Down,
            ),
        },
    }
}

/// `spec <dialect> <dir>`: every fixture, codec half and door half.
pub fn spec(rig: &Rig, dialect_name: &str, dir: &Path) -> i32 {
    let Some(dialect) = Dialect::of(dialect_name) else {
        result(dialect_name, false, "dialect", "unknown dialect");
        return 1;
    };
    let mut fixtures: Vec<String> = match fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".json"))
            .collect(),
        Err(e) => {
            result(
                dialect.name(),
                false,
                "fixtures",
                &format!("{}: {e}", dir.display()),
            );
            return 1;
        }
    };
    fixtures.sort();
    if fixtures.is_empty() {
        result(dialect.name(), false, "fixtures", "no .json fixtures");
        return 1;
    }
    let mut fails = 0;
    for f in &fixtures {
        let frame = match fs::read(dir.join(f))
            .map_err(|e| e.to_string())
            .and_then(|b| {
                serde_json::from_slice::<Value>(&b)
                    .map(|v| bytes_of(&v))
                    .map_err(|e| e.to_string())
            }) {
            Ok(frame) => frame,
            Err(e) => {
                result(
                    dialect.name(),
                    false,
                    f,
                    &format!("unreadable fixture: {e}"),
                );
                fails += 1;
                continue;
            }
        };
        let (codec, side) = spec_codec(dialect, f, &frame);
        let wires = [Wire {
            dir: side,
            bytes: frame,
        }];
        if !judge_relay(rig, dialect.name(), f, dialect, &wires, &codec) {
            fails += 1;
        }
    }
    i32::from(fails > 0)
}

// ══════════════════════════════════════════════════════════════════════════════════════════════
// replay
// ══════════════════════════════════════════════════════════════════════════════════════════════

fn read_jsonl(path: &Path) -> Result<Vec<Value>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| format!("{}: {e}", path.display())))
        .collect()
}

/// A transcript's frames, as the session saw them (client → caller, server → far end).
fn transcript_wires(lines: &[Value]) -> Vec<Wire> {
    lines
        .iter()
        .filter_map(|line| {
            let ev = line.get("event")?;
            let dir = match line.get("dir").and_then(Value::as_str) {
                Some("client") => Dir::Up,
                Some("server") => Dir::Down,
                _ => return None,
            };
            Some(Wire {
                dir,
                bytes: bytes_of(ev),
            })
        })
        .collect()
}

/// The decode half of a replay: one session state, every frame decoded in order, and the skeleton
/// in order.
fn replay_codec(dialect: Dialect, wires: &[Wire], skeleton: &[&str]) -> Result<String, String> {
    let mut st = DecodeState::default();
    let (mut tags, mut decoded) = (Vec::new(), 0_usize);
    for w in wires {
        let norms = match w.dir {
            Dir::Up => {
                let irs = dialect.read_up(&w.bytes, &mut st);
                decoded += irs.len();
                norm_up(&irs)
            }
            Dir::Down => {
                let irs = dialect.read_down(&w.bytes, &mut st);
                decoded += irs.len();
                norm_down(&irs)
            }
        };
        tags.extend(norms.iter().map(tag));
    }
    ordered_subsequence(&tags, skeleton)?;
    Ok(format!(
        "{decoded} IR events, skeleton {skeleton:?} in order"
    ))
}

/// `replay <root>`: each dialect's captured transcript, codec half and door half.
pub fn replay(rig: &Rig, root: &Path) -> i32 {
    // The skeletons the codec re-derives; the door's relay must show the same concepts in the
    // order it emitted them: it relays every frame, the far end's usage reports included.
    let cases: [(Dialect, &[&str]); 2] = [
        (
            Dialect::OpenAi,
            &[
                "config",
                "connect",
                "audio-in",
                "speech-start",
                "tool-args",
                "tool-close",
                "tool-result",
                "audio-out",
                "speech-start",
                "cancel",
            ],
        ),
        (
            Dialect::Gemini,
            &[
                "config",
                "connect",
                "audio-in",
                "tool-open",
                "tool-args",
                "tool-close",
                "tool-result",
                "audio-out",
                "speech-start",
                "audio-done",
                "usage",
            ],
        ),
    ];
    let mut fails = 0;
    for (dialect, skeleton) in cases {
        let what = format!("{}-transcript", dialect.name());
        let lines = match read_jsonl(&root.join(dialect.name()).join("transcript.jsonl")) {
            Ok(lines) => lines,
            Err(e) => {
                result("default", false, &what, &e);
                fails += 1;
                continue;
            }
        };
        let wires = transcript_wires(&lines);
        let codec = replay_codec(dialect, &wires, skeleton);
        let x = expect(dialect, &wires);
        let seen = rig.both(|p| relay(p, dialect, &wires));
        let pass = judge_planted(
            "default",
            &what,
            &seen,
            &|log: &Log| {
                let codec = codec.clone()?;
                let door = relay_check(dialect, log, &x)?;
                ordered_subsequence(&decode_relay(dialect, log).tags, skeleton)?;
                Ok(format!(
                    "codec: {codec}; door: {door}, skeleton {skeleton:?} in emitted order"
                ))
            },
            &relay_plants(),
        );
        if !pass {
            fails += 1;
        }
    }
    i32::from(fails > 0)
}

/// The frame bytes of a JSON value.
pub fn frame(v: &Value) -> Vec<u8> {
    bytes_of(v)
}

/// The far-end concepts a log's frames carry toward the caller, decoded under `dialect`.
pub fn caller_norms(dialect: Dialect, log: &Log) -> Vec<Norm> {
    decode_relay(dialect, log).caller
}

/// The concepts a log's frames carry toward the far end, decoded under `dialect`.
pub fn far_norms(dialect: Dialect, log: &Log) -> Vec<Norm> {
    decode_relay(dialect, log).far
}

/// The token units a usage frame reports, read by the codec: `(class, amount)`, zeros left out.
pub fn usage_tokens(dialect: Dialect, usage_frame: &[u8]) -> Vec<(u32, u64)> {
    expect(
        dialect,
        &[Wire {
            dir: Dir::Down,
            bytes: usage_frame.to_vec(),
        }],
    )
    .tokens
}
