// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE HARNESS'S OWN DECODER: what a dialect frame says, read into one neutral vocabulary so the
//! legs can name the concepts a door relayed (`Norm`) and the units a far end's reports add up to.
//!
//! The plane reads a frame only to meter it (a session is a relay, Law 5), so it no longer carries
//! a full reader. This is that reader, moved here from the plane crate's `codec` module as it stood
//! (busbar-plane-streaming dev 02f3824): the OpenAI Realtime GA and Gemini Live readers and the IR
//! they read into. Its writers are not moved: nothing the harness judges writes a frame.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};

use busbar_plane_streaming::codec::ir::config::{IrVad, MaxOutputTokens, SessionConfig};
use busbar_plane_streaming::codec::ir::media::AudioFormat;
use busbar_plane_streaming::codec::ir::usage::{read_count_u64, IrDuplexUsage};
use bytes::Bytes;
use serde_json::Value;

// ── the IR ──────────────────────────────────────────────────────────────────────────────────────

/// CLIENT → SERVER events (`BUSBAR-1.6.0.md` #18/#45). The union of the client-originated cases across the four layers.
/// **This vocabulary has no analog anywhere in the tree today** — building it is the net-new IR work.
#[derive(Debug, Clone, PartialEq)]
pub enum IrClientEvent {
    /// An uplink audio frame (`dir: Up`) — `input_audio_buffer.append`.
    AudioFrame(IrAudioFrame),
    /// A session-control / config event the client sent (reconciled against the locked config).
    Control(IrDuplexControl),
    /// The caller's tool RESULT, carried toward the upstream (`CallResult`).
    Tool(IrDuplexTool),
}

/// SERVER → CLIENT events (`BUSBAR-1.6.0.md` #18/#45). The union of the server-originated cases across the four layers —
/// the sibling of `IrStreamEvent`, plane-owned.
#[derive(Debug, Clone, PartialEq)]
pub enum IrServerEvent {
    /// The upstream acknowledged the session and returned its resolved config — `session.created`.
    /// Carried VERBATIM as opaque JSON (it holds server-assigned fields — `id`, `model`, `expires_at`
    /// — beyond the writable `SessionConfig` subset).
    SessionCreated {
        /// The resolved `session` object, opaque.
        session: serde_json::Value,
    },
    /// A tool-call announcement / argument delta / close (`CallOpen` / `CallArgs` / `CallClose`).
    Tool(IrDuplexTool),
    /// The user began speaking — barge-in trigger (`input_audio_buffer.speech_started`).
    SpeechStarted {
        /// Uplink-buffer offset (ms) where speech began.
        audio_start_ms: u64,
        /// The conversation item the buffered speech is being attributed to.
        item_id: String,
    },
    /// The user stopped speaking (`input_audio_buffer.speech_stopped`). `audio_end_ms` is the carrier
    /// the truncate math reads to bound the just-heard turn.
    SpeechStopped {
        /// Uplink-buffer offset (ms) where speech ended.
        audio_end_ms: u64,
        /// The conversation item the buffered speech is being attributed to.
        item_id: String,
    },
    /// A downlink audio frame (`dir: Down`) — `response.output_audio.delta`.
    AudioFrame(IrAudioFrame),
    /// The downlink audio for an item is complete — `response.output_audio.done`.
    AudioDone {
        /// The item whose audio just completed.
        item_id: String,
    },
    /// Extracted usage for a completed turn (`response.done.usage`).
    Usage(IrDuplexUsage),
    /// A rate-limit update (`rate_limits.updated`) — extraction-only.
    RateLimits,
    /// An upstream error surfaced on the session.
    Error {
        /// A stable, dialect-normalized error code.
        code: String,
        /// A human-readable message.
        message: String,
    },
}

/// THE NEUTRAL CONTROL / CONFIG IR (`BUSBAR-1.6.0.md` #18/#45). Translatable, but same-dialect it is verbatim carriage.
#[derive(Debug, Clone, PartialEq)]
pub enum IrDuplexControl {
    /// Configure the session — the authoritative copy the plane holds server-side and re-applies; a
    /// client-originated one is a hint reconciled against the lock, never trusted blind. Carries the
    /// full typed GA `session` object ([`SessionConfig`]).
    SessionConfigure {
        /// The GA `session` config object.
        config: SessionConfig,
    },
    /// Ask the upstream to begin generating a response. Carries the optional per-response override
    /// object (`response.create`'s `response` field), verbatim.
    ResponseCreate {
        /// The optional per-response override object, opaque.
        response: Option<serde_json::Value>,
    },
    /// Cancel the in-flight response (e.g. on barge-in) — `response.cancel`.
    ResponseCancel,
    /// Commit the buffered uplink audio as a user turn — `input_audio_buffer.commit`.
    InputAudioCommit,
    /// Discard the buffered uplink audio — `input_audio_buffer.clear`.
    InputAudioClear,
    /// Inject a conversation item (a non-tool message) — `conversation.item.create` whose `item` is
    /// carried VERBATIM as opaque JSON (a `function_call_output` item is modeled instead as
    /// [`IrDuplexTool::CallResult`]).
    ItemCreate {
        /// The `item` object, opaque.
        item: serde_json::Value,
    },
    /// Delete a conversation item — `conversation.item.delete`.
    ItemDelete {
        /// The item to delete.
        item_ref: String,
    },
    /// Barge-in bookkeeping (`BUSBAR-1.6.0.md` #18/#45): truncate a played item at the audio the user ACTUALLY heard —
    /// `conversation.item.truncate`. `audio_played_ms` is plane-computed state (busbar tracks playback
    /// position on WebSocket, where the server emits audio faster than realtime), NOT a field copied
    /// off the wire; it maps to/from the wire's `audio_end_ms`.
    ItemTruncate {
        /// The conversation item being truncated.
        item_ref: String,
        /// Which content part of the item (the wire `content_index`).
        content_index: u32,
        /// Milliseconds of audio the user actually heard before the barge-in.
        audio_played_ms: u64,
    },
}

/// THE CORRELATION ABSTRACTION for one in-flight tool call — NOT the wire `call_id`.
///
/// Modeled on the LLM plane's `IrDelta::InputJsonDelta` id-remap move (`BUSBAR-1.6.0.md` #18/#45): a
/// `CallRef → (client_call_id, upstream_call_id)` table held in the session scope lets a client that
/// speaks OpenAI `call_id` be bridged to a Gemini Live tool-call that correlates by NAME, not id.
///
/// An opaque newtype over a monotonic per-session counter, minted in
/// [`DecodeState`]. The `CallRef ↔ call_id` map lives there; each tool IR variant
/// ALSO carries the raw wire `call_id` so the stateless writer can re-frame `function_call_output`
/// without consulting the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallRef(pub u64);

/// THE NEUTRAL TOOL-CALL IR — busbar-owned, names no OpenAI noun in core. Modeled on
/// `IrDelta::InputJsonDelta`. `call_ref` is the join key across every variant; `call_id` is the raw
/// dialect id the wire correlates on (kept so a stateless writer can re-frame the result).
///
/// The Realtime tool loop this normalizes: the model announces a call
/// (`response.output_item.added` with a `function_call` item) then streams
/// `response.function_call_arguments.delta` → `…done`; the caller runs the tool and returns
/// `conversation.item.create{ function_call_output }` then `response.create`. A tool-call turn
/// often produces NO audio until the result is fed back.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrDuplexTool {
    /// A function call was ANNOUNCED by the model (server→client). Carries the tool name.
    CallOpen {
        /// The correlation handle minted for this call.
        call_ref: CallRef,
        /// The raw wire id the dialect correlates on.
        call_id: String,
        /// The tool name the model wants to invoke.
        name: String,
    },
    /// A STREAMED argument delta (server→client) — opaque JSON bytes appended to the call's arguments.
    CallArgs {
        /// The call this delta belongs to.
        call_ref: CallRef,
        /// The raw wire id the dialect correlates on.
        call_id: String,
        /// One chunk of the streamed argument JSON, verbatim.
        json_delta: Bytes,
    },
    /// Arguments are COMPLETE (server→client) — the model has finished streaming this call's args.
    CallClose {
        /// The call whose arguments are now complete.
        call_ref: CallRef,
        /// The raw wire id the dialect correlates on.
        call_id: String,
    },
    /// The caller's RESULT (client→server), carried upstream as `function_call_output` +
    /// `response.create` once the node's open-call table took it. busbar never authors one.
    CallResult {
        /// The call this result answers.
        call_ref: CallRef,
        /// The raw wire id the dialect correlates on.
        call_id: String,
        /// The tool NAME the result answers for, remembered from the call that opened it. Gemini's
        /// `functionResponse` REQUIRES a name and OpenAI's `function_call_output` carries none, so the
        /// name rides the IR rather than being looked up by a writer that is deliberately stateless.
        /// EMPTY when the result answers a call this session never saw announced — a name nobody told
        /// us is not a name to invent, so it is simply omitted from the wire.
        name: String,
        /// The tool's opaque output payload.
        output: Bytes,
    },
}

impl IrDuplexTool {
    /// The correlation handle common to every variant.
    #[cfg(test)]
    #[must_use]
    pub fn call_ref(&self) -> CallRef {
        match self {
            IrDuplexTool::CallOpen { call_ref, .. }
            | IrDuplexTool::CallArgs { call_ref, .. }
            | IrDuplexTool::CallClose { call_ref, .. }
            | IrDuplexTool::CallResult { call_ref, .. } => *call_ref,
        }
    }
}

/// DIRECTION OF TRAVEL for one framed audio message — the plane frames on top of a byte-duplex pipe,
/// so each frame carries which way it flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpDown {
    /// Client → server (`input_audio_buffer.append`).
    Up,
    /// Server → client (`response.output_audio.delta`).
    Down,
}

/// WHICH ITEM THIS AUDIO IS PART OF — an OPAQUE correlation the plane carries but never interprets.
///
/// A downlink frame belongs to one response, one output item, one content part. The plane's own
/// barge-in bookkeeping does not need those ids (it counts bytes), but the CLIENT does: every downlink
/// frame is relayed through the writer, and a client handed audio with no item named cannot issue its
/// own truncate when the user interrupts. Each field is `None` when the source dialect never said it —
/// Gemini's `modelTurn` audio names no item at all, and an id nobody issued is worse than an absent
/// one, so nothing here is ever invented.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IrAudioRef {
    /// The response this audio belongs to.
    pub response_id: Option<String>,
    /// The conversation item this audio belongs to — the id a truncate names.
    pub item_id: Option<String>,
    /// Which output of that response.
    pub output_index: Option<u32>,
    /// Which content part of that output.
    pub content_index: Option<u32>,
}

/// THE NEUTRAL AUDIO-FRAME IR (`BUSBAR-1.6.0.md` #18/#45). `media` is OPAQUE — the identity transform by default; the IR
/// exists for the meter/audit tap, not the reshape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrAudioFrame {
    /// Which direction this frame travels.
    pub dir: UpDown,
    /// Monotonic per-direction sequence number (audit ordering / gap detection).
    pub seq: u64,
    /// The audio payload — opaque bytes, relayed verbatim under the identity transform by default.
    pub media: Bytes,
    /// Which item this audio is part of, as the source dialect named it (absent when it named none).
    pub origin: IrAudioRef,
    /// The format this frame's own wire states its bytes are in, where the dialect states one per
    /// frame (Gemini's blob `mimeType`); `None` where it states none (a Realtime append), and the
    /// session's negotiated format governs. The uplink meter counts a frame at this format first:
    /// a 16 kHz frame counted at the 24 kHz rate bills two-thirds of the audio it carries.
    pub format: Option<AudioFormat>,
}

// ── per-session decode state ────────────────────────────────────────────────────────────────────

/// PER-SESSION DECODE STATE threaded through the reader — the analog of the LLM reader's
/// `StreamDecodeState`. Holds what is per-session, not per-frame: monotonic frame sequencing, the
/// `CallRef ↔ call_id` correlation table (`BUSBAR-1.6.0.md` #18/#45), the negotiated output format, and the barge-in
/// playback-position bookkeeping (`BUSBAR-1.6.0.md` #18/#45 — the plane tracks bytes played because the upstream emits
/// audio faster than realtime).
/// THE CEILING ON THE `call_id → CallRef` TABLE, stated here rather than left implicit.
///
/// The table is a correlation convenience — open, args, close and result for ONE call meeting under
/// one handle — not a ledger, and nothing removes from it within a session (a `conversation.item.delete`
/// deletes an ITEM, which is not the call). A client that mints distinct `call_id`s therefore grows
/// it for as long as the call lasts. 1024 is far past any real turn's concurrent tool calls, so the
/// eviction below is unreachable in ordinary use; past it the OLDEST entry goes and a re-sighting of
/// an evicted id correlates as a NEW call, which is the honest answer once the older correlation is
/// no longer held.
pub const MAX_TRACKED_CALL_IDS: usize = 1024;

/// THE CEILING ON THE DROPPED-FIELD DIAGNOSTIC LIST. The list is a diagnostic tap, not a ledger: a
/// peer that patches the session with unmodelled fields every frame must not be able to grow it
/// without limit. Past the ceiling the OLDEST note goes — the most recent drops are the ones an
/// operator is looking at.
pub const MAX_TRACKED_DROPPED_FIELDS: usize = 64;

#[derive(Debug)]
pub struct DecodeState {
    up_seq: u64,
    down_seq: u64,
    next_call_ref: u64,
    call_ids: HashMap<String, CallRef>,
    /// Insertion order of `call_ids`, oldest first — what makes "evict the oldest" answerable
    /// without asking the map, which has no order.
    call_id_order: VecDeque<String>,
    /// The tool NAME the model announced for a `call_id` — the slot Gemini's `functionResponse`
    /// requires and OpenAI's `function_call_output` does not carry, remembered from the originating
    /// call so a cross-dialect result can still name its tool.
    call_names: HashMap<String, String>,
    /// Wire fields the decode could not model and therefore DROPPED, newest last. This crate links no
    /// logging surface, so the plane's "warn" is realized as a recorded, readable drop rather than a
    /// silent one.
    dropped_fields: VecDeque<String>,
}

impl Default for DecodeState {
    fn default() -> Self {
        DecodeState {
            up_seq: 0,
            down_seq: 0,
            next_call_ref: 0,
            call_ids: HashMap::new(),
            call_id_order: VecDeque::new(),
            call_names: HashMap::new(),
            dropped_fields: VecDeque::new(),
        }
    }
}

impl DecodeState {
    /// The next uplink frame sequence number (monotonic per session).
    pub fn next_up_seq(&mut self) -> u64 {
        let s = self.up_seq;
        self.up_seq += 1;
        s
    }

    /// The next downlink frame sequence number (monotonic per session).
    pub fn next_down_seq(&mut self) -> u64 {
        let s = self.down_seq;
        self.down_seq += 1;
        s
    }

    /// The [`CallRef`] for a wire `call_id`, minting a fresh monotonic handle the first time the id is
    /// seen and returning the SAME handle on every later sighting (open → args → close → result all
    /// correlate to one ref). Bounded by [`MAX_TRACKED_CALL_IDS`]: past the ceiling the oldest id is
    /// evicted, so a session cannot be made to grow this table without limit by minting call ids.
    pub fn ref_for_call_id(&mut self, call_id: &str) -> CallRef {
        if let Some(r) = self.call_ids.get(call_id) {
            return *r;
        }
        let r = CallRef(self.next_call_ref);
        self.next_call_ref += 1;
        self.call_ids.insert(call_id.to_string(), r);
        self.call_id_order.push_back(call_id.to_string());
        while self.call_id_order.len() > MAX_TRACKED_CALL_IDS {
            if let Some(oldest) = self.call_id_order.pop_front() {
                self.call_ids.remove(&oldest);
                self.call_names.remove(&oldest);
            }
        }
        r
    }

    /// REMEMBER the tool name the model announced for a `call_id`. OpenAI's `function_call_output`
    /// carries no name; Gemini's `functionResponse` REQUIRES one, so the name is kept from the
    /// originating call and handed back on the result. An empty name records nothing.
    pub fn remember_call_name(&mut self, call_id: &str, name: &str) {
        if name.is_empty() || call_id.is_empty() {
            return;
        }
        self.call_names
            .insert(call_id.to_string(), name.to_string());
    }

    /// The tool name remembered for a `call_id` (empty when the call was never announced on this
    /// session — a result for an id we never saw open cannot invent a name).
    #[must_use]
    pub fn call_name(&self, call_id: &str) -> &str {
        self.call_names.get(call_id).map_or("", String::as_str)
    }

    /// RECORD a wire field the decode could not model and dropped — the plane's "warn" made readable.
    /// Bounded by [`MAX_TRACKED_DROPPED_FIELDS`].
    pub fn record_dropped_field(&mut self, field: &str) {
        self.dropped_fields.push_back(field.to_string());
        while self.dropped_fields.len() > MAX_TRACKED_DROPPED_FIELDS {
            self.dropped_fields.pop_front();
        }
    }

    /// The wire fields dropped as unmodelled so far this session, newest last.
    #[must_use]
    pub fn dropped_fields(&self) -> Vec<&str> {
        self.dropped_fields.iter().map(String::as_str).collect()
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────────────────────────

fn parse(wire: &[u8]) -> Option<Value> {
    serde_json::from_slice::<Value>(wire).ok()
}

fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn u64_at(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or_default()
}

/// One output token per this many bytes of an unreadable usage report: the llm plane's floor
/// estimate (`TRUNCATED_TAIL_BYTES_PER_TOKEN`).
const FLOOR_BYTES_PER_TOKEN: u64 = 4;

/// Read one BILLED count: an absent object, an absent field or a JSON `null` is 0 (no such quantity
/// was reported), a readable count is the count (through the crate's one seam,
/// [`read_count_u64`]), and a present-but-UNREADABLE count is a REFUSAL (#42) —
/// the old `unwrap_or_default` metered a count the provider really sent as zero, and every money view
/// over that turn was then faithfully wrong. The error is the operator-facing message: the field and
/// a BOUNDED spelling (cut on a character, so a hostile multi-byte spelling cannot panic the cut).
fn billed_count(o: Option<&Value>, key: &str) -> Result<u64, String> {
    match o.and_then(|x| x.get(key)) {
        None => Ok(0),
        Some(v) if v.is_null() => Ok(0),
        Some(v) => read_count_u64(v).ok_or_else(|| {
            let spelling: String = v.to_string().chars().take(64).collect();
            format!("usage field `{key}` is present but is not a count: {spelling}")
        }),
    }
}

/// WHAT AN UNREADABLE USAGE REPORT BILLS (owner ruling 2026-10-27: spend is always attributed and
/// nothing is silent): the llm plane's floor, one output token per [`FLOOR_BYTES_PER_TOKEN`] bytes
/// of the report and at least one, marked as the plane's floor. Its one log line names the field and
/// its spelling; the door's call capture carries it to the plugin's host log.
fn usage_floor(report_len: usize, _message: &str) -> IrDuplexUsage {
    IrDuplexUsage {
        text_out: (report_len as u64 / FLOOR_BYTES_PER_TOKEN).max(1),
        floor: true,
        ..IrDuplexUsage::default()
    }
}

/// Read the item correlation this dialect states on a downlink audio event. Each field is carried only
/// when the wire actually said it — an absent field stays absent rather than becoming an empty string
/// or a zero index, which name a different (real) item.
fn audio_ref_of(v: &Value) -> IrAudioRef {
    let text = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let index = |k: &str| {
        v.get(k)
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
    };
    IrAudioRef {
        response_id: text("response_id"),
        item_id: text("item_id"),
        output_index: index("output_index"),
        content_index: index("content_index"),
    }
}

/// base64-decode a wire audio string to opaque media bytes (the identity IR is the decoded bytes).
///
/// `None` on a payload that is not base64, and every read path must DROP that frame rather than
/// substitute an empty one. The callee returns `Option` deliberately — it refuses even a lone
/// trailing sextet "rather than silently drop it, honoring the fail-loud contract" — and an empty
/// frame relayed and metered in place of a refusal is indistinguishable from a caller who said
/// nothing, which is the one confusion a voice plane cannot afford. The Twilio grammar in this same
/// crate already answers this way (`BadPayload`, never an empty payload).
fn decode_audio(b64: &str) -> Option<Bytes> {
    busbar_contract::media::base64_decode(b64).map(Bytes::from_owner)
}

/// DECODE A `session` OBJECT LENIENTLY — the drop-and-warn discipline the cross-dialect map states
/// for values this plane does not model (`voice-cross-dialect-map.json`: an unmodelled telephony
/// codec, an eagerness with no twin, a noise-reduction knob are each recorded as a DROPPED FIELD, not
/// a refused session).
///
/// The whole-object parse is tried first (the ordinary path, no allocation beyond serde's). Only when
/// it fails is the patch salvaged FIELD BY FIELD: a key that cannot stand alone is the key the plane
/// cannot model, so that key alone is dropped — noted in [`DecodeState::dropped_fields`] — and the
/// rest of the client's patch survives. Defaulting the whole object instead would silently replace the
/// client's instructions, tools, voice and turn detection with an EMPTY session, which is the one
/// answer a session patch must never give.
///
/// Unknown KEYS are not drops: `SessionConfig` ignores them by design (a partial GA patch names only
/// what it changes), so they never reach this path.
fn session_config_lenient(session: &Value, st: &mut DecodeState) -> SessionConfig {
    let read = |v: &Value| serde_json::from_value::<SessionConfig>(v.clone());
    if let Ok(cfg) = read(session) {
        return cfg;
    }
    let Some(obj) = session.as_object() else {
        // Not an object at all — there is no field to salvage.
        st.record_dropped_field("session");
        return SessionConfig::default();
    };
    let mut kept = serde_json::Map::new();
    for (k, val) in obj {
        let probe = Value::Object(std::iter::once((k.clone(), val.clone())).collect());
        if read(&probe).is_ok() {
            kept.insert(k.clone(), val.clone());
        } else {
            st.record_dropped_field(k);
        }
    }
    let kept = Value::Object(kept);
    read(&kept).unwrap_or_else(|_| {
        // Every field stood alone yet the set does not parse — nothing left to salvage honestly.
        st.record_dropped_field("session");
        SessionConfig::default()
    })
}

// ── OpenAI Realtime GA ──────────────────────────────────────────────────────────────────────────

/// The OpenAI Realtime GA dialect.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenAiRealtime;

mod oa_wire {
    // client → server
    pub const SESSION_UPDATE: &str = "session.update";
    pub const INPUT_AUDIO_APPEND: &str = "input_audio_buffer.append";
    pub const INPUT_AUDIO_COMMIT: &str = "input_audio_buffer.commit";
    pub const INPUT_AUDIO_CLEAR: &str = "input_audio_buffer.clear";
    pub const ITEM_CREATE: &str = "conversation.item.create";
    pub const ITEM_TRUNCATE: &str = "conversation.item.truncate";
    pub const ITEM_DELETE: &str = "conversation.item.delete";
    pub const RESPONSE_CREATE: &str = "response.create";
    pub const RESPONSE_CANCEL: &str = "response.cancel";
    // server → client
    pub const SESSION_CREATED: &str = "session.created";
    pub const SPEECH_STARTED: &str = "input_audio_buffer.speech_started";
    pub const SPEECH_STOPPED: &str = "input_audio_buffer.speech_stopped";
    pub const OUTPUT_AUDIO_DELTA: &str = "response.output_audio.delta";
    pub const OUTPUT_AUDIO_DELTA_LEGACY: &str = "response.audio.delta";
    pub const OUTPUT_AUDIO_DONE: &str = "response.output_audio.done";
    pub const OUTPUT_AUDIO_DONE_LEGACY: &str = "response.audio.done";
    pub const OUTPUT_ITEM_ADDED: &str = "response.output_item.added";
    pub const FN_ARGS_DELTA: &str = "response.function_call_arguments.delta";
    pub const FN_ARGS_DONE: &str = "response.function_call_arguments.done";
    pub const RESPONSE_DONE: &str = "response.done";
    pub const RATE_LIMITS_UPDATED: &str = "rate_limits.updated";
    pub const ERROR: &str = "error";
    // shared item nouns
    pub const ITEM_FN_CALL_OUTPUT: &str = "function_call_output";
    pub const ITEM_FN_CALL: &str = "function_call";
}

/// The OpenAI Realtime GA reader.
impl OpenAiRealtime {
    pub fn read_up(&self, wire: &[u8], st: &mut DecodeState) -> Vec<IrClientEvent> {
        let Some(v) = parse(wire) else {
            return Vec::new();
        };
        let ty = str_at(&v, "type");
        match ty {
            oa_wire::SESSION_UPDATE => {
                let cfg = v
                    .get("session")
                    .map(|s| session_config_lenient(s, st))
                    .unwrap_or_default();
                vec![IrClientEvent::Control(IrDuplexControl::SessionConfigure {
                    config: cfg,
                })]
            }
            oa_wire::INPUT_AUDIO_APPEND => {
                let Some(media) = decode_audio(str_at(&v, "audio")) else {
                    return Vec::new();
                };
                vec![IrClientEvent::AudioFrame(IrAudioFrame {
                    dir: UpDown::Up,
                    seq: st.next_up_seq(),
                    media,
                    // The uplink append names no item — the item does not exist until the server
                    // makes one.
                    origin: IrAudioRef::default(),
                    // The append states no format: the session's negotiated input format governs.
                    format: None,
                })]
            }
            oa_wire::INPUT_AUDIO_COMMIT => {
                vec![IrClientEvent::Control(IrDuplexControl::InputAudioCommit)]
            }
            oa_wire::INPUT_AUDIO_CLEAR => {
                vec![IrClientEvent::Control(IrDuplexControl::InputAudioClear)]
            }
            oa_wire::ITEM_CREATE => {
                let item = v.get("item").cloned().unwrap_or(Value::Null);
                if str_at(&item, "type") == oa_wire::ITEM_FN_CALL_OUTPUT {
                    let call_id = str_at(&item, "call_id");
                    let call_ref = st.ref_for_call_id(call_id);
                    let output = Bytes::from(str_at(&item, "output").to_owned().into_bytes());
                    // The dialect's own result item carries no tool name; the one the model announced
                    // is remembered per call id, for the dialects that require it.
                    let name = st.call_name(call_id).to_string();
                    vec![IrClientEvent::Tool(IrDuplexTool::CallResult {
                        call_ref,
                        call_id: call_id.to_string(),
                        name,
                        output,
                    })]
                } else {
                    vec![IrClientEvent::Control(IrDuplexControl::ItemCreate { item })]
                }
            }
            oa_wire::ITEM_TRUNCATE => {
                vec![IrClientEvent::Control(IrDuplexControl::ItemTruncate {
                    item_ref: str_at(&v, "item_id").to_string(),
                    // A wire value that does not fit takes the field's documented default (`0`),
                    // the same answer an ABSENT `content_index` gets. Narrowing with `as` would
                    // instead hand back a different, perfectly valid index into different content.
                    content_index: u32::try_from(u64_at(&v, "content_index")).unwrap_or(0),
                    audio_played_ms: u64_at(&v, "audio_end_ms"),
                })]
            }
            oa_wire::ITEM_DELETE => {
                vec![IrClientEvent::Control(IrDuplexControl::ItemDelete {
                    item_ref: str_at(&v, "item_id").to_string(),
                })]
            }
            oa_wire::RESPONSE_CREATE => {
                let response = v.get("response").cloned();
                vec![IrClientEvent::Control(IrDuplexControl::ResponseCreate {
                    response,
                })]
            }
            oa_wire::RESPONSE_CANCEL => {
                vec![IrClientEvent::Control(IrDuplexControl::ResponseCancel)]
            }
            _ => Vec::new(),
        }
    }

    pub fn read_down(&self, wire: &[u8], st: &mut DecodeState) -> Vec<IrServerEvent> {
        let report_len = wire.len();
        let Some(v) = parse(wire) else {
            return Vec::new();
        };
        let ty = str_at(&v, "type");
        match ty {
            oa_wire::SESSION_CREATED => {
                let session = v.get("session").cloned().unwrap_or(Value::Null);
                vec![IrServerEvent::SessionCreated { session }]
            }
            oa_wire::SPEECH_STARTED => vec![IrServerEvent::SpeechStarted {
                audio_start_ms: u64_at(&v, "audio_start_ms"),
                item_id: str_at(&v, "item_id").to_string(),
            }],
            oa_wire::SPEECH_STOPPED => vec![IrServerEvent::SpeechStopped {
                audio_end_ms: u64_at(&v, "audio_end_ms"),
                item_id: str_at(&v, "item_id").to_string(),
            }],
            oa_wire::OUTPUT_AUDIO_DELTA | oa_wire::OUTPUT_AUDIO_DELTA_LEGACY => {
                let Some(media) = decode_audio(str_at(&v, "delta")) else {
                    return Vec::new();
                };
                vec![IrServerEvent::AudioFrame(IrAudioFrame {
                    dir: UpDown::Down,
                    seq: st.next_down_seq(),
                    media,
                    origin: audio_ref_of(&v),
                    format: None,
                })]
            }
            oa_wire::OUTPUT_AUDIO_DONE | oa_wire::OUTPUT_AUDIO_DONE_LEGACY => {
                vec![IrServerEvent::AudioDone {
                    item_id: str_at(&v, "item_id").to_string(),
                }]
            }
            oa_wire::OUTPUT_ITEM_ADDED => {
                let item = v.get("item").cloned().unwrap_or(Value::Null);
                if str_at(&item, "type") == oa_wire::ITEM_FN_CALL {
                    let call_id = str_at(&item, "call_id");
                    let call_ref = st.ref_for_call_id(call_id);
                    let name = str_at(&item, "name").to_string();
                    // Remember the name for the RESULT leg: OpenAI's `function_call_output` carries
                    // none, and a Gemini `functionResponse` requires one.
                    st.remember_call_name(call_id, &name);
                    vec![IrServerEvent::Tool(IrDuplexTool::CallOpen {
                        call_ref,
                        call_id: call_id.to_string(),
                        name,
                    })]
                } else {
                    Vec::new()
                }
            }
            oa_wire::FN_ARGS_DELTA => {
                let call_id = str_at(&v, "call_id");
                let call_ref = st.ref_for_call_id(call_id);
                let json_delta = Bytes::from(str_at(&v, "delta").to_owned().into_bytes());
                vec![IrServerEvent::Tool(IrDuplexTool::CallArgs {
                    call_ref,
                    call_id: call_id.to_string(),
                    json_delta,
                })]
            }
            oa_wire::FN_ARGS_DONE => {
                let call_id = str_at(&v, "call_id");
                let call_ref = st.ref_for_call_id(call_id);
                let mut out = Vec::new();
                // THE COMPLETE ARGUMENTS THE CLOSE STATES. This dialect repeats the whole argument
                // string on the done event; a dialect that delivers the call atomically needs exactly
                // that, and a peer whose deltas were never seen (a session joined mid-call) has
                // nothing else. Dropping it dispatched the call with no arguments at all.
                let arguments = str_at(&v, "arguments");
                if !arguments.is_empty() {
                    out.push(IrServerEvent::Tool(IrDuplexTool::CallArgs {
                        call_ref,
                        call_id: call_id.to_string(),
                        json_delta: Bytes::from(arguments.to_owned().into_bytes()),
                    }));
                }
                out.push(IrServerEvent::Tool(IrDuplexTool::CallClose {
                    call_ref,
                    call_id: call_id.to_string(),
                }));
                out
            }
            oa_wire::RESPONSE_DONE => {
                // An absent `usage` is no report (zero, as before); a present-but-unreadable billed
                // count bills the plane's floor and says so (#42: never metered at zero).
                match v
                    .get("response")
                    .and_then(|r| r.get("usage"))
                    .map(extract_usage)
                    .transpose()
                {
                    Ok(usage) => vec![IrServerEvent::Usage(usage.unwrap_or_default())],
                    Err(message) => vec![IrServerEvent::Usage(usage_floor(report_len, &message))],
                }
            }
            oa_wire::RATE_LIMITS_UPDATED => vec![IrServerEvent::RateLimits],
            oa_wire::ERROR => {
                let err = v.get("error").cloned().unwrap_or(Value::Null);
                vec![IrServerEvent::Error {
                    code: str_at(&err, "code").to_string(),
                    message: str_at(&err, "message").to_string(),
                }]
            }
            _ => Vec::new(),
        }
    }
}

/// Extract the split token classes from a `response.done.usage` object (`BUSBAR-1.6.0.md` #18/#45 — audio vs text are
/// SEPARATE classes; extraction-only, never client-translated).
///
/// The stated `input_tokens`/`output_tokens` totals are what the far end bills ("Usage statistics
/// for the Response, this will correspond to billing", the Realtime API reference), and the
/// per-modality breakdown (`input_token_details`/`output_token_details`) only SPLITS them. So each
/// side bills the larger of its stated total and the sum of every entry its breakdown carries
/// (`audio_tokens`, `text_tokens`, and on input `image_tokens`), never the audio and text entries
/// alone: a turn that also carried image input billed its audio and text and dropped the rest.
/// Audio bills as audio and image tokens as image tokens (each its own declared class); every
/// other token the side bills is attributed to `text`. It is an integer passthrough of the wire
/// figures, no rate lookup or multiply, so #71's hot-path rule is untouched.
///
/// This is the exact twin of the Gemini Live dialect's `usage_from_metadata` (`gemini/mod.rs`).
///
/// Every count is a BILLED count and is read through [`billed_count`]: absent is zero, and a
/// present-but-unreadable one is an `Err` the caller bills the floor for ([`usage_floor`], #42).
fn extract_usage(u: &Value) -> Result<IrDuplexUsage, String> {
    let ind = u.get("input_token_details");
    let outd = u.get("output_token_details");
    let audio_in = billed_count(ind, "audio_tokens")?;
    let image_in = billed_count(ind, "image_tokens")?;
    let input = billed_count(Some(u), "input_tokens")?.max(
        audio_in
            .saturating_add(billed_count(ind, "text_tokens")?)
            .saturating_add(image_in),
    );
    let audio_out = billed_count(outd, "audio_tokens")?;
    let output = billed_count(Some(u), "output_tokens")?
        .max(audio_out.saturating_add(billed_count(outd, "text_tokens")?));
    Ok(IrDuplexUsage {
        audio_in,
        audio_out,
        text_in: input - audio_in - image_in,
        text_out: output - audio_out,
        image_in,
        video_in: 0,
        cached: billed_count(ind, "cached_tokens")?,
        floor: false,
    })
}

// ── Gemini Live ─────────────────────────────────────────────────────────────────────────────────

/// The Gemini Live dialect.
#[derive(Debug, Default, Clone, Copy)]
pub struct GeminiLive;

mod gl_wire {
    // client → server
    pub const SETUP: &str = "setup";
    pub const CLIENT_CONTENT: &str = "clientContent";
    pub const REALTIME_INPUT: &str = "realtimeInput";
    /// The manual end-of-uplink marker nested under `realtimeInput`. Gemini's cross-dialect twin of
    /// OpenAI's discrete `input_audio_buffer.commit` — both end the buffered uplink turn.
    pub const AUDIO_STREAM_END: &str = "audioStreamEnd";
    pub const TOOL_RESPONSE: &str = "toolResponse";
    // server → client
    pub const SETUP_COMPLETE: &str = "setupComplete";
    pub const SERVER_CONTENT: &str = "serverContent";
    pub const TOOL_CALL: &str = "toolCall";
    pub const USAGE_METADATA: &str = "usageMetadata";
}

/// Map a Gemini audio `mimeType` to the shared [`AudioFormat`], IN THE DIRECTION IT ARRIVED.
///
/// The two directions of this dialect run at DIFFERENT rates — 16 kHz is what Gemini requires on its
/// uplink, 24 kHz is what the model synthesizes — and each format carries exactly one
/// bytes-per-millisecond constant. The uplink meter divides the client's bytes by it, and the
/// barge-in truncate math divides the downlink's.
///
/// So the probe is directional and states the rate. UPLINK PCM is `Pcm16At16k` at 16 kHz and `Pcm16`
/// at 24 kHz: counting 16 kHz bytes at the 24 kHz rate billed two-thirds of the audio the caller
/// spoke. DOWNLINK PCM is accepted only at the rate the truncate math measures in: a 16 kHz downlink
/// blob has no frame, and admitting it as `Pcm16` would meter 480 bytes of 16 kHz audio (30 ms) as
/// 10 ms, cutting the user off mid-word at the next barge-in. An untagged `audio/pcm` takes the
/// direction's own rate, which is what an untagged frame in that direction is.
#[must_use]
pub fn audio_format_from_mime(mime: &str, dir: UpDown) -> Option<AudioFormat> {
    let m = mime.to_ascii_lowercase();
    if !m.starts_with("audio/pcm") {
        return None;
    }
    // The `rate=` parameter is matched EXACTLY against each `;`-delimited part, never by substring:
    // `"rate=160000"` (10x the real 16 kHz rate) CONTAINS `"rate=16000"` as a literal substring, and a
    // `.contains` probe mismeasured it as the 16 kHz rate it is not — on the downlink that mismeasures
    // the truncate math's bytes-per-ms by 10x (premature barge-in truncation) and mismeters billed
    // audio duration by the same factor. A rate parameter that IS stated but matches neither known
    // value is a rate this dialect does not recognize, and is REFUSED — not an untagged blob, and
    // never guessed at as the nearest-looking known value.
    let mut rate_16k = false;
    let mut rate_24k = false;
    let mut untagged = true;
    for part in m.split(';').map(str::trim) {
        if let Some(rate) = part.strip_prefix("rate=") {
            untagged = false;
            match rate {
                "16000" => rate_16k = true,
                "24000" => rate_24k = true,
                _ => {}
            }
        }
    }
    match dir {
        // The uplink's own rate is 16 kHz; a blob that states 24 kHz is metered at 24 kHz.
        UpDown::Up if rate_16k || untagged => Some(AudioFormat::Pcm16At16k),
        UpDown::Up if rate_24k => Some(AudioFormat::Pcm16),
        // The truncate math measures against THIS rate; anything else is a different format.
        UpDown::Down if rate_24k || untagged => Some(AudioFormat::Pcm16),
        _ => None,
    }
}

/// Join a Gemini `systemInstruction` (`{ parts: [{ text }] }`) into a single instruction string.
fn system_instruction_text(si: &Value) -> Option<String> {
    let parts = si.get("parts")?.as_array()?;
    let joined: String = parts
        .iter()
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

/// Map Gemini `realtimeInputConfig.automaticActivityDetection` onto the shared VAD IR's THREE states
/// (see `SessionConfig::turn_detection`), which Gemini's own wire also distinguishes:
///
/// - a config that names no `automaticActivityDetection` says nothing about turn detection → `None`
///   (absent), so re-framing it does not invent a setting the caller never asked for;
/// - `disabled: true` is Gemini's explicit "the client drives turns" → `Some(None)`;
/// - anything else is a configured detector → `Some(Some(..))`.
///
/// Gemini's sensitivity enums have no shared home (dropped); OpenAI-only knobs (`threshold` /
/// `create_response` / `interrupt_response`) take their shared defaults.
fn vad_from_realtime_input_config(ric: &Value) -> Option<Option<IrVad>> {
    let aad = ric.get("automaticActivityDetection")?;
    if aad.get("disabled").and_then(Value::as_bool) == Some(true) {
        return Some(None);
    }
    Some(Some(IrVad::ServerVad {
        threshold: 0.5,
        // A timing that does not fit in the shared IR's `u32` takes the SAME documented default an
        // absent field takes. Narrowing with `as` would turn a nonsense value into "no padding at
        // all" / "no silence window at all" — working settings nobody configured.
        prefix_padding_ms: aad
            .get("prefixPaddingMs")
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(300),
        silence_duration_ms: aad
            .get("silenceDurationMs")
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(200),
        create_response: true,
        interrupt_response: true,
    }))
}

/// Decode a Gemini `setup` object into the shared [`SessionConfig`] (the cross-dialect superset).
///
/// A field the plane cannot model is DROPPED and RECORDED (the per-field drop-and-warn discipline the
/// OpenAI session patch already follows) — never defaulted into a working setting nobody asked for.
fn session_config_from_setup(setup: &Value, st: &mut DecodeState) -> SessionConfig {
    let gc = setup
        .get("generationConfig")
        .cloned()
        .unwrap_or(Value::Null);
    let modalities = gc
        .get("responseModalities")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_ascii_lowercase)
                .collect()
        })
        .unwrap_or_default();
    let voice = gc
        .get("speechConfig")
        .and_then(|s| s.get("voiceConfig"))
        .and_then(|s| s.get("prebuiltVoiceConfig"))
        .and_then(|s| s.get("voiceName"))
        .and_then(Value::as_str)
        .map(str::to_string);
    // A CAP THAT DOES NOT FIT IS NOT "NO CAP". Present-but-unreadable is a REFUSAL of that field: it
    // is dropped and recorded, and the rest of the setup stands. Letting it fall through to `None`
    // silently handed the session an UNBOUNDED response where the client asked for a bounded one —
    // the one direction a limit must never move on its own. (The OpenAI dialect refuses the same
    // narrowing on its own `max_response_output_tokens`.)
    let max_output_tokens = match gc.get("maxOutputTokens") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_u64().and_then(|n| u32::try_from(n).ok()) {
            Some(n) => Some(MaxOutputTokens::Limit(n)),
            None => {
                st.record_dropped_field("maxOutputTokens");
                None
            }
        },
    };
    SessionConfig {
        model: setup
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        modalities,
        instructions: setup
            .get("systemInstruction")
            .and_then(system_instruction_text),
        voice,
        input_audio_format: None,
        output_audio_format: None,
        turn_detection: setup
            .get("realtimeInputConfig")
            .and_then(vad_from_realtime_input_config),
        tools: setup
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        tool_choice: None,
        max_output_tokens,
        context_window_tokens: None,
    }
}

/// The modality split of a Gemini `*TokensDetails` array (`[{modality, tokenCount}]`): its AUDIO,
/// IMAGE and VIDEO entries, and the sum of every entry. BILLED COUNTS, read through
/// [`billed_count`]: an absent array, modality or count is zero, a present but unreadable count is
/// an `Err` the caller turns into a refusal (#42).
#[derive(Default)]
struct Split {
    audio: u64,
    image: u64,
    video: u64,
    all: u64,
}

fn modality_split(details: Option<&Value>) -> Result<Split, String> {
    let mut split = Split::default();
    for d in details.and_then(Value::as_array).into_iter().flatten() {
        let n = billed_count(Some(d), "tokenCount")?;
        split.all = split.all.saturating_add(n);
        let modality = d
            .get("modality")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let class = if modality.eq_ignore_ascii_case("AUDIO") {
            &mut split.audio
        } else if modality.eq_ignore_ascii_case("IMAGE") {
            &mut split.image
        } else if modality.eq_ignore_ascii_case("VIDEO") {
            &mut split.video
        } else {
            continue;
        };
        *class = class.saturating_add(n);
    }
    Ok(split)
}

/// Extract the split token classes from a Gemini `usageMetadata` object (`BUSBAR-1.6.0.md` #18/#45 — audio vs text are
/// SEPARATE classes; extraction-only, never client-translated).
///
/// GOOGLE'S TOTAL IS BILLED (the owner's gemini-tool-use-billing ruling, `BUSBAR-1.6.0.md`: "a
/// grounded Gemini turn bills Google's total, tool-use prompt tokens included"). Input is the
/// prompt (the larger of `promptTokenCount` and the sum of every `promptTokensDetails` entry, so
/// IMAGE, VIDEO and DOCUMENT tokens bill beside AUDIO and TEXT) plus the tool-use prompt (the larger
/// of `toolUsePromptTokenCount` and its `toolUsePromptTokensDetails`); output is the response (the
/// larger of `responseTokenCount` and its `responseTokensDetails`) plus `thoughtsTokenCount`, which
/// Google prices as output ("Output price (including thinking tokens)"). Where `totalTokenCount`
/// states more than that sum, the remainder bills as input text. AUDIO, IMAGE and VIDEO entries bill
/// as their own declared classes; every other token is attributed to `text`. It is an integer
/// passthrough of the wire figures, no rate lookup or multiply.
///
/// Every count is a BILLED count and is read through [`billed_count`]: absent is zero, and a
/// present-but-unreadable one is an `Err` the caller turns into a refusal (#42).
fn usage_from_metadata(u: &Value) -> Result<IrDuplexUsage, String> {
    let prompt = modality_split(u.get("promptTokensDetails"))?;
    let tool = modality_split(u.get("toolUsePromptTokensDetails"))?;
    let response = modality_split(u.get("responseTokensDetails"))?;
    let audio_in = prompt.audio.saturating_add(tool.audio);
    let image_in = prompt.image.saturating_add(tool.image);
    let video_in = prompt.video.saturating_add(tool.video);
    let input = billed_count(Some(u), "promptTokenCount")?
        .max(prompt.all)
        .saturating_add(billed_count(Some(u), "toolUsePromptTokenCount")?.max(tool.all));
    let output = billed_count(Some(u), "responseTokenCount")?
        .max(response.all)
        .saturating_add(billed_count(Some(u), "thoughtsTokenCount")?);
    let input = input.max(billed_count(Some(u), "totalTokenCount")?.saturating_sub(output));
    Ok(IrDuplexUsage {
        audio_in,
        text_in: input
            .saturating_sub(audio_in)
            .saturating_sub(image_in)
            .saturating_sub(video_in),
        audio_out: response.audio,
        text_out: output.saturating_sub(response.audio),
        image_in,
        video_in,
        cached: billed_count(Some(u), "cachedContentTokenCount")?,
        floor: false,
    })
}

/// The Gemini Live reader.
impl GeminiLive {
    pub fn read_up(&self, wire: &[u8], st: &mut DecodeState) -> Vec<IrClientEvent> {
        let Some(v) = parse(wire) else {
            return Vec::new();
        };

        if let Some(setup) = v.get(gl_wire::SETUP) {
            let cfg = session_config_from_setup(setup, st);
            return vec![IrClientEvent::Control(IrDuplexControl::SessionConfigure {
                config: cfg,
            })];
        }

        if let Some(cc) = v.get(gl_wire::CLIENT_CONTENT) {
            // A client turn (`turns` + `turnComplete`) is injected VERBATIM as a conversation item —
            // the plane locks/reconciles it but never reshapes the content bytes.
            return vec![IrClientEvent::Control(IrDuplexControl::ItemCreate {
                item: cc.clone(),
            })];
        }

        if let Some(ri) = v.get(gl_wire::REALTIME_INPUT) {
            // Uplink audio arrives in TWO Gemini spellings, both decoded to `IrAudioFrame{dir:Up}` with
            // a monotonic seq: the GA (`v1beta`) `realtimeInput.audio` SINGLE inline blob, and the
            // legacy `realtimeInput.mediaChunks[]` ARRAY. A `{mimeType,data}` blob whose mime is not a
            // modeled audio format has no shared frame (drop).
            let mut out = Vec::new();
            let mut push_blob = |blob: &Value, out: &mut Vec<IrClientEvent>| {
                let Some(format) = audio_format_from_mime(str_at(blob, "mimeType"), UpDown::Up)
                else {
                    return; // non-audio realtime input has no shared frame (drop).
                };
                let Some(media) = decode_audio(str_at(blob, "data")) else {
                    return; // a payload that is not base64 is not silence — emit no frame.
                };
                out.push(IrClientEvent::AudioFrame(IrAudioFrame {
                    dir: UpDown::Up,
                    seq: st.next_up_seq(),
                    media,
                    // This dialect names no item on either audio direction.
                    origin: IrAudioRef::default(),
                    // The rate the blob's mime states: the rate its bytes are metered at.
                    format: Some(format),
                }));
            };
            // Prefer the GA single blob when present so a GA peer never double-decodes; else fall back
            // to the legacy per-chunk array (one frame each).
            if let Some(audio) = ri.get("audio") {
                push_blob(audio, &mut out);
            } else if let Some(chunks) = ri.get("mediaChunks").and_then(Value::as_array) {
                for chunk in chunks {
                    push_blob(chunk, &mut out);
                }
            }
            // `realtimeInput.audioStreamEnd` is Gemini's manual end-of-uplink marker; it is the
            // cross-dialect twin of OpenAI's discrete `input_audio_buffer.commit`. Map it to the shared
            // `IrDuplexControl::InputAudioCommit` so the "end the buffered uplink turn" concept survives
            // cross-dialect (a frame may carry audio AND the end marker; emit the commit after the audio).
            if ri.get(gl_wire::AUDIO_STREAM_END).is_some() {
                out.push(IrClientEvent::Control(IrDuplexControl::InputAudioCommit));
            }
            return out;
        }

        if let Some(tr) = v.get(gl_wire::TOOL_RESPONSE) {
            let mut out = Vec::new();
            if let Some(rs) = tr.get("functionResponses").and_then(Value::as_array) {
                for r in rs {
                    let call_id = str_at(r, "id");
                    let call_ref = st.ref_for_call_id(call_id);
                    // The `response` object is the tool's opaque output payload (the moat normalizes
                    // correlation, not the bytes). `functionResponse.name` is REQUIRED by this dialect,
                    // so it is carried on the IR; a response that omits it falls back to the name the
                    // originating call announced.
                    let wire_name = str_at(r, "name");
                    let name = if wire_name.is_empty() {
                        st.call_name(call_id).to_string()
                    } else {
                        st.remember_call_name(call_id, wire_name);
                        wire_name.to_string()
                    };
                    let output = Bytes::from(
                        serde_json::to_vec(r.get("response").unwrap_or(&Value::Null))
                            .unwrap_or_default(),
                    );
                    out.push(IrClientEvent::Tool(IrDuplexTool::CallResult {
                        call_ref,
                        call_id: call_id.to_string(),
                        name,
                        output,
                    }));
                }
            }
            return out;
        }

        Vec::new()
    }

    pub fn read_down(&self, wire: &[u8], st: &mut DecodeState) -> Vec<IrServerEvent> {
        let report_len = wire.len();
        let Some(v) = parse(wire) else {
            return Vec::new();
        };

        if let Some(sc) = v.get(gl_wire::SETUP_COMPLETE) {
            return vec![IrServerEvent::SessionCreated {
                session: sc.clone(),
            }];
        }

        // The message-type branches frame the content; `usageMetadata` is a TOP-LEVEL field that may ride
        // on ANY downlink message (a `serverContent` or `toolCall` frame as well as a standalone one), so
        // it is read once, after the branch, never only when the frame carries nothing else — usage
        // riding on a content frame was otherwise billed as zero tokens (MONEY-AUDIT STR-4).
        let mut out = if let Some(sc) = v.get(gl_wire::SERVER_CONTENT) {
            let mut out = Vec::new();
            if let Some(parts) = sc
                .get("modelTurn")
                .and_then(|mt| mt.get("parts"))
                .and_then(Value::as_array)
            {
                for p in parts {
                    if let Some(inline) = p.get("inlineData") {
                        let mime = str_at(inline, "mimeType");
                        // The played-out position below is measured in the shared format's own
                        // bytes-per-ms, so a downlink blob at any other rate has no frame here.
                        let Some(format) = audio_format_from_mime(mime, UpDown::Down) else {
                            continue;
                        };
                        let Some(media) = decode_audio(str_at(inline, "data")) else {
                            continue; // a payload that is not base64 is not silence — emit no frame.
                        };
                        out.push(IrServerEvent::AudioFrame(IrAudioFrame {
                            dir: UpDown::Down,
                            seq: st.next_down_seq(),
                            media,
                            // A Gemini `modelTurn` part carries no response/item correlation, and an
                            // id nobody issued is worse than an absent one.
                            origin: IrAudioRef::default(),
                            format: Some(format),
                        }));
                    }
                    // A `text` part / transcription side-channel has no shared IR home (drop+warn).
                }
            }
            // `interrupted` = the model's generation was cut off by a barge-in → the shared barge-in
            // signal. `turnComplete` = the model turn's audio is complete → AudioDone.
            if sc.get("interrupted").and_then(Value::as_bool) == Some(true) {
                out.push(IrServerEvent::SpeechStarted {
                    audio_start_ms: 0,
                    item_id: String::new(),
                });
            }
            if sc.get("turnComplete").and_then(Value::as_bool) == Some(true) {
                out.push(IrServerEvent::AudioDone {
                    item_id: String::new(),
                });
            }
            out
        } else if let Some(tc) = v.get(gl_wire::TOOL_CALL) {
            let mut out = Vec::new();
            if let Some(calls) = tc.get("functionCalls").and_then(Value::as_array) {
                for c in calls {
                    let call_id = str_at(c, "id");
                    let call_ref = st.ref_for_call_id(call_id);
                    let name = str_at(c, "name").to_string();
                    // Remember the name for the RESULT leg — this dialect requires it back.
                    st.remember_call_name(call_id, &name);
                    // Gemini delivers the call ATOMICALLY; expand it into the shared STREAMED triple
                    // (open → args → close) so the correlation moat matches the OpenAI codec exactly.
                    out.push(IrServerEvent::Tool(IrDuplexTool::CallOpen {
                        call_ref,
                        call_id: call_id.to_string(),
                        name,
                    }));
                    let json_delta = Bytes::from(
                        serde_json::to_vec(c.get("args").unwrap_or(&Value::Null))
                            .unwrap_or_default(),
                    );
                    out.push(IrServerEvent::Tool(IrDuplexTool::CallArgs {
                        call_ref,
                        call_id: call_id.to_string(),
                        json_delta,
                    }));
                    out.push(IrServerEvent::Tool(IrDuplexTool::CallClose {
                        call_ref,
                        call_id: call_id.to_string(),
                    }));
                }
            }
            out
        } else {
            // `toolCallCancellation`, `goAway`, `sessionResumptionUpdate` and any unknown frame have no
            // shared IR home — degrade to empty (drop+warn).
            Vec::new()
        };

        if let Some(um) = v.get(gl_wire::USAGE_METADATA) {
            // A present-but-unreadable billed count bills the plane's floor and says so (#42),
            // never meters it at zero.
            out.push(IrServerEvent::Usage(
                usage_from_metadata(um).unwrap_or_else(|message| usage_floor(report_len, &message)),
            ));
        }
        out
    }
}
