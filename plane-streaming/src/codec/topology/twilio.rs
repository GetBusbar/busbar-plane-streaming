// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The Twilio Media Streams envelope for the telephony topology.

#[cfg(test)]
use serde::Serialize;

use busbar_contract::media::{base64_decode, base64_encode};

use crate::codec::ir::codec::peek::{base64_len, Head, Lite};
use crate::codec::ir::codec::{wire, WireEvent};

/// The most characters of a carrier-chosen string a log line repeats.
const LOGGED_CHARS: usize = 64;

/// `text` as a log line may carry it: escaped (no line break or control character the carrier
/// wrote reaches the log as itself) and cut at [`LOGGED_CHARS`] characters.
fn loggable(text: &str) -> String {
    let mut out: String = text
        .chars()
        .take(LOGGED_CHARS)
        .flat_map(char::escape_debug)
        .collect();
    if text.chars().nth(LOGGED_CHARS).is_some() {
        out.push_str("...");
    }
    out
}

/// Twilio's negotiated µ-law encoding string on the `start` event's media format.
pub const TWILIO_MULAW_ENCODING: &str = "audio/x-mulaw";
/// The only sample rate the 8 kHz passthrough carrier accepts.
pub const TWILIO_SAMPLE_RATE: u64 = 8000;
/// The only channel count the mono barge-in arithmetic accepts.
pub const TWILIO_CHANNELS: u64 = 1;

/// A REFUSAL on the Twilio envelope boundary — every variant fails closed rather than admitting a
/// frame the session engine would then trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TwilioError {
    /// The WS frame was not the expected Twilio JSON shape (unparseable, or a required field missing).
    Malformed,
    /// A well-formed frame carried an `event` the envelope does not model.
    UnknownEvent(String),
    /// A `media` payload was not valid base64.
    BadPayload,
    /// The `start` event's negotiated media format is not `g711_ulaw` / 8000 Hz / mono — refused so the
    /// wrong-format bytes never reach the barge-in truncate math.
    FormatMismatch {
        /// The encoding Twilio actually negotiated.
        encoding: String,
        /// The sample rate Twilio actually negotiated.
        sample_rate: u64,
        /// The channel count Twilio actually negotiated.
        channels: u64,
    },
}

impl std::fmt::Display for TwilioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TwilioError::Malformed => write!(f, "malformed Twilio Media Streams frame"),
            TwilioError::UnknownEvent(e) => write!(f, "unknown Twilio event: {}", loggable(e)),
            TwilioError::BadPayload => write!(f, "Twilio media payload is not valid base64"),
            TwilioError::FormatMismatch {
                encoding,
                sample_rate,
                channels,
            } => write!(
                f,
                "Twilio media format refused: {} {sample_rate}Hz {channels}ch is not g711_ulaw/8000Hz/mono",
                loggable(encoding)
            ),
        }
    }
}

impl std::error::Error for TwilioError {}

/// The negotiated media format Twilio declares on its `start` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaFormat {
    /// The wire encoding (`audio/x-mulaw` for the passthrough carrier).
    pub encoding: String,
    /// The sample rate in Hz (8000 for the passthrough carrier).
    pub sample_rate: u64,
    /// The channel count (1 for the passthrough carrier).
    pub channels: u64,
}

/// The `start` event's carried identity + format — the one place the call's `streamSid`, Twilio's
/// `callSid`, and the negotiated format arrive together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwilioStart {
    /// The `streamSid` Twilio minted for this Media Streams connection, repeated on every later frame.
    pub stream_sid: String,
    /// Twilio's `callSid` for the underlying PSTN call.
    pub call_sid: String,
    /// The negotiated media format to validate against the locked `g711_ulaw` config.
    pub media_format: MediaFormat,
}

/// One decoded Twilio Media Streams event. Only [`TwilioEvent::Media`] carries session audio; the
/// lifecycle variants drive the adapter, not the session pump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TwilioEvent {
    /// The one-time handshake event, first on the socket.
    Connected,
    /// The stream opened; carries identity + the negotiated media format.
    Start(TwilioStart),
    /// A ~20 ms audio frame — `payload` is the raw µ-law bytes for `client_in`.
    Media {
        /// The connection's `streamSid`, to check against the admitted value.
        stream_sid: String,
        /// The base64-decoded raw µ-law audio bytes.
        payload: Vec<u8>,
    },
    /// A playback-position ack — corroborates the session's own barge-in clock, never load-bearing.
    Mark {
        /// The connection's `streamSid`.
        stream_sid: String,
        /// The mark name the outbound side chose.
        name: String,
    },
    /// A touch-tone keypress heard on the inbound track — sent only when the stream has DTMF
    /// enabled. It carries no session audio: the adapter discards it exactly like [`Self::Mark`].
    /// It is modelled rather than left unknown because an unmodelled event is a decode ERROR, and a
    /// caller pressing a key must not be handled as a garbled or forged frame.
    Dtmf {
        /// The connection's `streamSid`.
        stream_sid: String,
        /// The key that was pressed (`0`-`9`, `*`, `#`, `A`-`D`), or empty when Twilio omits it.
        digit: String,
    },
    /// The terminal event; the adapter closes `client_in` on it.
    Stop,
}

/// The Twilio envelope codec: stateless decode/encode between Twilio's JSON Media Streams frames and the realtime audio envelope.
pub struct TwilioEnvelope;

impl TwilioEnvelope {
    /// DECODE one inbound Twilio WS frame into a [`TwilioEvent`]. A `media` event's base64 payload is decoded into raw audio bytes.
    pub fn decode(frame: &[u8]) -> Result<TwilioEvent, TwilioError> {
        Lite::read(frame).ok_or(TwilioError::Malformed)?;
        let v: serde_json::Value =
            serde_json::from_slice(frame).map_err(|_| TwilioError::Malformed)?;
        let event = v
            .get("event")
            .and_then(serde_json::Value::as_str)
            .ok_or(TwilioError::Malformed)?;
        match event {
            "connected" => Ok(TwilioEvent::Connected),
            "start" => {
                let start = v.get("start").ok_or(TwilioError::Malformed)?;
                let stream_sid = start
                    .get("streamSid")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| v.get("streamSid").and_then(serde_json::Value::as_str))
                    .ok_or(TwilioError::Malformed)?
                    .to_string();
                let call_sid = start
                    .get("callSid")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let mf = start.get("mediaFormat").ok_or(TwilioError::Malformed)?;
                let media_format = MediaFormat {
                    encoding: mf
                        .get("encoding")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    sample_rate: mf
                        .get("sampleRate")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or_default(),
                    channels: mf
                        .get("channels")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or_default(),
                };
                Ok(TwilioEvent::Start(TwilioStart {
                    stream_sid,
                    call_sid,
                    media_format,
                }))
            }
            "media" => {
                let stream_sid = v
                    .get("streamSid")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let payload_b64 = v
                    .get("media")
                    .and_then(|m| m.get("payload"))
                    .and_then(serde_json::Value::as_str)
                    .ok_or(TwilioError::Malformed)?;
                let payload = base64_decode(payload_b64)
                    .ok_or(TwilioError::BadPayload)?
                    .to_vec();
                Ok(TwilioEvent::Media {
                    stream_sid,
                    payload,
                })
            }
            "mark" => {
                let stream_sid = v
                    .get("streamSid")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = v
                    .get("mark")
                    .and_then(|m| m.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                Ok(TwilioEvent::Mark { stream_sid, name })
            }
            "dtmf" => {
                let stream_sid = v
                    .get("streamSid")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let digit = v
                    .get("dtmf")
                    .and_then(|d| d.get("digit"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                Ok(TwilioEvent::Dtmf { stream_sid, digit })
            }
            "stop" => Ok(TwilioEvent::Stop),
            other => Err(TwilioError::UnknownEvent(other.to_string())),
        }
    }

    /// ENCODE raw µ-law bytes into a Twilio outbound `media` envelope for a given `stream_sid`. The audio payload is standard base64.
    #[must_use]
    #[cfg(test)]
    pub fn encode_media(stream_sid: &str, mulaw: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        Self::encode_media_into(&mut out, stream_sid, mulaw);
        out
    }

    /// The same envelope, rendered into a buffer the caller already holds.
    #[cfg(test)]
    pub fn encode_media_into(out: &mut Vec<u8>, stream_sid: &str, mulaw: &[u8]) {
        out.clear();
        if !is_bare_json_string(stream_sid) {
            let doc = OutboundMedia {
                event: "media",
                stream_sid,
                media: OutboundPayload {
                    payload: base64_encode(mulaw),
                },
            };
            out.extend_from_slice(
                &serde_json::to_vec(&doc).expect("the outbound media envelope always serializes"),
            );
            return;
        }
        const HEAD: &[u8] = br#"{"event":"media","streamSid":""#;
        const MIDDLE: &[u8] = br#"","media":{"payload":""#;
        const TAIL: &[u8] = br#""}}"#;
        out.reserve(
            HEAD.len() + stream_sid.len() + MIDDLE.len() + mulaw.len().div_ceil(3) * 4 + TAIL.len(),
        );
        out.extend_from_slice(HEAD);
        out.extend_from_slice(stream_sid.as_bytes());
        out.extend_from_slice(MIDDLE);
        base64_encode_into(mulaw, out);
        out.extend_from_slice(TAIL);
    }
}

/// Whether a string is its own JSON body: printable ASCII with neither character a JSON string must escape.
#[cfg(test)]
fn is_bare_json_string(s: &str) -> bool {
    s.bytes()
        .all(|b| (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\')
}

/// Standard base64 (RFC 4648) with padding, appended to a buffer the caller holds: the same bytes as the contract encoder, without a temporary allocation.
#[cfg(test)]
fn base64_encode_into(data: &[u8], out: &mut Vec<u8>) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 63) as usize]);
        out.push(ALPHABET[((n >> 12) & 63) as usize]);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize]
        } else {
            b'='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize]
        } else {
            b'='
        });
    }
}

#[cfg(test)]
#[derive(Serialize)]
struct OutboundMedia<'a> {
    event: &'static str,
    #[serde(rename = "streamSid")]
    stream_sid: &'a str,
    media: OutboundPayload,
}

#[cfg(test)]
#[derive(Serialize)]
struct OutboundPayload {
    payload: String,
}

/// ASSERT the negotiated media format is the locked `g711_ulaw` / 8000 Hz / mono carrier, refusing every other format.
pub fn assert_g711_ulaw(fmt: &MediaFormat) -> Result<(), TwilioError> {
    if fmt.encoding == TWILIO_MULAW_ENCODING
        && fmt.sample_rate == TWILIO_SAMPLE_RATE
        && fmt.channels == TWILIO_CHANNELS
    {
        Ok(())
    } else {
        Err(TwilioError::FormatMismatch {
            encoding: fmt.encoding.clone(),
            sample_rate: fmt.sample_rate,
            channels: fmt.channels,
        })
    }
}

/// The telephony carrier's envelope around a session: the caller speaks Twilio Media Streams, the
/// far end the realtime dialect locked to `g711_ulaw` both ways, so the audio passes through
/// unchanged and only the envelope is rewritten.
#[derive(Debug, Default)]
pub struct TwilioBridge {
    stream_sid: Option<String>,
    /// Whether the bridge already said, once, that it dropped model audio it could not read.
    told_unreadable: std::cell::Cell<bool>,
}

/// What one caller frame on the telephony leg is.
#[derive(Debug, PartialEq, Eq)]
pub enum CallerStep {
    /// Caller audio, as the far end's dialect frames an uplink append.
    Audio(WireEvent),
    /// The call ended.
    Stop,
    /// The call is refused (no stream id, another media format, a frame not in Twilio's shape, a
    /// payload that is not clean base64): the session ends.
    Refused,
    /// Nothing for the far end (the handshake, a start, a mark, a keypress, an event this reader
    /// does not model, or media for another stream).
    Nothing,
}

/// A carrier frame that ends the call, said in one log line naming why: the call drops, and the caller receives no further audio.
fn refused(why: &TwilioError) -> CallerStep {
    tracing::warn!("the telephony carrier's frame ends the call: {why}");
    CallerStep::Refused
}

impl TwilioBridge {
    /// Whether the carrier has stated its stream: its `start` was read and admitted.
    #[must_use]
    pub fn started(&self) -> bool {
        self.stream_sid.is_some()
    }

    /// Read one caller frame. A start with no stream id, or in another media format, is refused; so is malformed media.
    #[allow(clippy::wrong_self_convention)]
    pub fn from_caller(&mut self, frame: &[u8]) -> CallerStep {
        let Some(top) = Lite::read(frame) else {
            return refused(&TwilioError::Malformed);
        };
        {
            if top.str_at("event") == "media" {
                let sid = top.str_at("streamSid");
                let payload = top.get("media").and_then(|m| m.get("payload"));
                let Some(Lite::Str(payload)) = payload else {
                    return refused(&TwilioError::Malformed);
                };
                if base64_len(payload).is_none() {
                    return refused(&TwilioError::BadPayload);
                }
                if sid.is_empty() || self.stream_sid.as_deref() != Some(sid) {
                    return CallerStep::Nothing;
                }
                let append = format!(
                    r#"{{"type":"{}","audio":"{}"}}"#,
                    wire::INPUT_AUDIO_APPEND,
                    payload
                );
                return CallerStep::Audio(WireEvent(bytes::Bytes::from(append)));
            }
        }
        match TwilioEnvelope::decode(frame) {
            Ok(TwilioEvent::Start(_)) if self.stream_sid.is_some() => CallerStep::Nothing,
            Ok(TwilioEvent::Start(start)) => {
                if start.stream_sid.is_empty() {
                    return refused(&TwilioError::Malformed);
                }
                if let Err(why) = assert_g711_ulaw(&start.media_format) {
                    return refused(&why);
                }
                self.stream_sid = Some(start.stream_sid);
                CallerStep::Nothing
            }
            Ok(TwilioEvent::Media {
                stream_sid,
                payload,
            }) => {
                if stream_sid.is_empty() || self.stream_sid.as_deref() != Some(stream_sid.as_str())
                {
                    return CallerStep::Nothing;
                }
                // Base64 holds no character a JSON string escapes, so the frame is written whole.
                let append = format!(
                    r#"{{"type":"{}","audio":"{}"}}"#,
                    wire::INPUT_AUDIO_APPEND,
                    base64_encode(&payload)
                );
                CallerStep::Audio(WireEvent(bytes::Bytes::from(append.into_bytes())))
            }
            Ok(TwilioEvent::Stop) => CallerStep::Stop,
            Ok(_) | Err(TwilioError::UnknownEvent(_)) => CallerStep::Nothing,
            Err(why) => refused(&why),
        }
    }

    /// Rewrite one frame bound for the caller into the carrier's envelope: model audio becomes a Twilio media event.
    #[must_use]
    pub fn to_caller(&self, frame: &[u8]) -> Option<Vec<u8>> {
        // The frame's type is read first, borrowing it: only a frame the carrier is told of is
        // read whole.
        let head = Head::read(frame)?;
        let kind = head.ty.as_deref()?;
        // Function-call frames are not forwarded: the carrier speaks only media, mark and clear,
        // and relaying tool names or arguments would leak data to the telephony transport.
        if kind.starts_with(FUNCTION_CALL_ARGUMENTS) {
            return None;
        }
        if ITEM_EVENTS.contains(&kind) {
            return None;
        }
        let sid = self.stream_sid.as_deref()?;
        match kind {
            "response.output_audio.delta" | "response.audio.delta" => {
                let top = Lite::read(frame)?;
                let audio = match top.get("delta") {
                    Some(Lite::Str(audio)) if base64_len(audio).is_some() => Some(audio.as_ref()),
                    _ => None,
                };
                if audio.is_none() && !self.told_unreadable.replace(true) {
                    tracing::warn!(
                        "the far end sent model audio that is not base64; the carrier hears a \
                         gap, and no later such frame on this call is said"
                    );
                }
                let sid = serde_json::to_string(sid).expect("a string serializes");
                Some(
                    format!(
                        r#"{{"event":"media","streamSid":{sid},"media":{{"payload":"{}"}}}}"#,
                        audio?
                    )
                    .into_bytes(),
                )
            }
            "input_audio_buffer.speech_started" => serde_json::to_vec(&serde_json::json!({
                "event": "clear",
                "streamSid": sid,
            }))
            .ok(),
            _ => None,
        }
    }
}

/// The far end's events streaming a tool call's arguments, by their type's prefix.
const FUNCTION_CALL_ARGUMENTS: &str = "response.function_call_arguments.";

/// The far end's item events: a tool call the model made is one whose item is a function call.
const ITEM_EVENTS: &[&str] = &[
    wire::OUTPUT_ITEM_ADDED,
    "response.output_item.done",
    "conversation.item.created",
    "conversation.item.added",
    "conversation.item.done",
];

#[cfg(test)]
#[path = "tests/twilio_tests.rs"]
mod twilio_tests;
