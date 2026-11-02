// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The second dialect, Google **Gemini Live** (`BidiGenerateContent`): what a session meters off
//! it, and the `setup` a locked session is told.
//!
//! WIRE SHAPE. Gemini Live is a JSON duplex where each message is an object with ONE top-level key
//! naming its kind (`setup` / `clientContent` / `realtimeInput` / `toolResponse` client→server;
//! `setupComplete` / `serverContent` / `toolCall` / `toolCallCancellation` / `usageMetadata`
//! server→client). `usageMetadata` may ride on any downlink message. Gemini delivers a tool call
//! ATOMICALLY (`toolCall.functionCalls[]`): each call in it is one opened call. Gemini Live has no
//! in-band error message (it reports errors with the socket's close).

use super::peek::{base64_len_any_alphabet, Head, Lite};
use super::{billed_count, usage_floor, usage_object, DuplexReader, Metered};
use crate::codec::ir::config::{IrVad, MaxOutputTokens, SessionConfig};
use crate::codec::ir::media::AudioFormat;
use crate::codec::ir::usage::IrDuplexUsage;
use serde_json::{json, Value};

/// The canonical message keys the setup and usage conversion write.
mod wire {
    pub const SETUP: &str = "setup";
    pub const USAGE_METADATA: &str = "usageMetadata";
}

/// THE Gemini Live DIALECT CODEC — the plane's SECOND dialect. A unit struct: stateless and
/// shareable, exactly like [`super::OpenAiRealtimeCodec`].
#[derive(Debug, Default, Clone, Copy)]
pub struct GeminiLiveCodec;

// ── mime / audio-format helpers ───────────────────────────────────────────────────────────────────

/// Map a Gemini UPLINK audio `mimeType` to the shared [`AudioFormat`] its bytes are metered at.
///
/// 16 kHz is what Gemini requires on its uplink, and each format carries exactly one
/// bytes-per-millisecond constant the uplink meter divides the client's bytes by. So uplink PCM is
/// `Pcm16At16k` at 16 kHz (and untagged, the uplink's own rate) and `Pcm16` at 24 kHz: counting
/// 16 kHz bytes at the 24 kHz rate billed two-thirds of the audio the caller spoke.
#[must_use]
pub fn audio_format_from_mime(mime: &str) -> Option<AudioFormat> {
    let m = mime.to_ascii_lowercase();
    if !m.starts_with("audio/pcm") {
        return None;
    }
    // The `rate=` parameter is matched EXACTLY against each `;`-delimited part, never by substring:
    // `"rate=160000"` (10x the real 16 kHz rate) CONTAINS `"rate=16000"` as a literal substring, and a
    // `.contains` probe mismeasured it as the 16 kHz rate it is not, and mismetered billed audio
    // duration by 10x. A rate parameter that IS stated but matches neither known
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
    // The uplink's own rate is 16 kHz; a blob that states 24 kHz is metered at 24 kHz.
    if rate_16k || untagged {
        Some(AudioFormat::Pcm16At16k)
    } else if rate_24k {
        Some(AudioFormat::Pcm16)
    } else {
        None
    }
}

// ── setup ─────────────────────────────────────────────────────────────────────────────────────────

/// The `setup` members a session locked to `cfg` states, as the governed splice puts them over a
/// caller's own `setup` ([`crate::splice`]).
#[must_use]
pub(crate) fn setup_locked(cfg: &SessionConfig) -> serde_json::Map<String, Value> {
    match setup_from_session_config(cfg) {
        Value::Object(mut m) => match m.remove(wire::SETUP) {
            Some(Value::Object(setup)) => setup,
            _ => serde_json::Map::new(),
        },
        _ => serde_json::Map::new(),
    }
}

/// THE `setup` FRAME a session locked to `cfg` is told, with the locked session's members in Gemini's
/// shape. Modalities are UPPERCASED to Gemini's enum tokens; server VAD's padding and silence
/// windows are `automaticActivityDetection`'s, and barge-in off (`interrupt_response: false`) is
/// `activityHandling: NO_INTERRUPTION`; the activation `threshold` and `create_response` have no
/// Gemini field.
pub(crate) fn setup_from_session_config(cfg: &SessionConfig) -> Value {
    let mut setup = serde_json::Map::new();
    if let Some(model) = &cfg.model {
        setup.insert("model".into(), json!(model));
    }
    let mut gc = serde_json::Map::new();
    if !cfg.modalities.is_empty() {
        let mods: Vec<Value> = cfg
            .modalities
            .iter()
            .map(|m| json!(m.to_ascii_uppercase()))
            .collect();
        gc.insert("responseModalities".into(), Value::Array(mods));
    }
    if let Some(voice) = &cfg.voice {
        gc.insert(
            "speechConfig".into(),
            json!({ "voiceConfig": { "prebuiltVoiceConfig": { "voiceName": voice } } }),
        );
    }
    if let Some(MaxOutputTokens::Limit(n)) = cfg.max_output_tokens {
        // The `"inf"` sentinel has no Gemini equivalent — Gemini simply omits the cap.
        gc.insert("maxOutputTokens".into(), json!(n));
    }
    if !gc.is_empty() {
        setup.insert("generationConfig".into(), Value::Object(gc));
    }
    if let Some(instr) = &cfg.instructions {
        setup.insert(
            "systemInstruction".into(),
            json!({ "parts": [{ "text": instr }] }),
        );
    }
    if !cfg.tools.is_empty() {
        setup.insert("tools".into(), Value::Array(cfg.tools.clone()));
    }
    if let Some(trigger) = cfg.context_window_tokens {
        // The plane's context-window ceiling: the far end compresses the context, on its sliding
        // window, once it reaches it.
        setup.insert(
            "contextWindowCompression".into(),
            json!({ "triggerTokens": trigger, "slidingWindow": {} }),
        );
    }
    // Three-state (see `SessionConfig::turn_detection`): an ABSENT turn_detection names no setting,
    // so no `realtimeInputConfig` is written at all and the session keeps what it had. Only an
    // explicit `null` writes Gemini's own disable marker.
    match &cfg.turn_detection {
        None => {}
        Some(None) => {
            setup.insert(
                "realtimeInputConfig".into(),
                json!({ "automaticActivityDetection": { "disabled": true } }),
            );
        }
        Some(Some(IrVad::ServerVad {
            prefix_padding_ms,
            silence_duration_ms,
            interrupt_response,
            ..
        })) => {
            let mut config = json!({ "automaticActivityDetection": {
                "prefixPaddingMs": prefix_padding_ms,
                "silenceDurationMs": silence_duration_ms,
            }});
            // Barge-in off is Gemini's `activityHandling`; on is its default, and names nothing.
            if !interrupt_response {
                config["activityHandling"] = json!("NO_INTERRUPTION");
            }
            setup.insert("realtimeInputConfig".into(), config);
        }
        Some(Some(IrVad::SemanticVad { .. })) => {
            // Semantic VAD has no Gemini knob set; enable automatic detection generically.
            setup.insert(
                "realtimeInputConfig".into(),
                json!({ "automaticActivityDetection": {} }),
            );
        }
    }
    json!({ wire::SETUP: Value::Object(setup) })
}

// ── usage ↔ usageMetadata ─────────────────────────────────────────────────────────────────────────

/// The modality split of a Gemini `*TokensDetails` array (`[{modality, tokenCount}]`): its AUDIO,
/// IMAGE and VIDEO entries, and the sum of every entry. BILLED COUNTS, read through
/// [`billed_count`]: an absent array, modality or count is zero, a present but unreadable count is
/// an `Err` the caller turns into a refusal.
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

/// Extract the split token classes from a Gemini `usageMetadata` object (audio vs text are
/// separate classes; extraction-only, never client-translated).
///
/// Google's total is billed: a grounded Gemini turn bills Google's total, tool-use prompt tokens
/// included. Input is the
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
/// present-but-unreadable one is an `Err` the caller turns into a refusal.
fn usage_from_metadata(u: &Value) -> Result<IrDuplexUsage, String> {
    usage_object(u, wire::USAGE_METADATA)?;
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

// ── the reads ───────────────────────────────────────────────────────────────────────────────────

impl DuplexReader for GeminiLiveCodec {
    /// Each call a `toolCall` opens (a frame that is a `setupComplete` or a `serverContent` opens
    /// none), then the `usageMetadata` any frame but a `setupComplete` carries.
    fn meter_down(&self, frame: &[u8]) -> Vec<Metered> {
        let metered = Head::read(frame).is_none_or(|h| {
            h.keys.iter().any(|k| {
                matches!(
                    k.as_ref(),
                    "usageMetadata" | "usage_metadata" | "toolCall" | "tool_call"
                )
            })
        });
        if !metered {
            return Vec::new();
        }
        let (Some(top), Ok(v)) = (Lite::read(frame), serde_json::from_slice::<Value>(frame)) else {
            return Vec::new();
        };
        let read = || -> Result<Vec<Metered>, String> {
            let kind = proto_member(
                &top,
                &[
                    "setupComplete",
                    "setup_complete",
                    "serverContent",
                    "server_content",
                    "toolCall",
                    "tool_call",
                    "toolCallCancellation",
                    "tool_call_cancellation",
                    "goAway",
                    "go_away",
                    "sessionResumptionUpdate",
                    "session_resumption_update",
                ],
            )
            .map_err(|_| "ambiguous Gemini message".to_owned())?;
            if kind.is_some_and(|(k, _)| matches!(k, "setupComplete" | "setup_complete")) {
                return Ok(Vec::new());
            }
            let mut out = Vec::new();
            if let Some((key, tc)) = kind.filter(|(k, _)| matches!(*k, "toolCall" | "tool_call")) {
                let calls = proto_value(tc, &v[key], &["functionCalls", "function_calls"])?;
                if let Some((_, calls)) = calls {
                    let calls = calls
                        .as_array()
                        .ok_or("Gemini functionCalls is not an array")?;
                    out.resize(calls.len(), Metered::ToolOpen);
                }
            }
            if let Some((um_lite, um)) =
                proto_value(&top, &v, &["usageMetadata", "usage_metadata"])?
            {
                let um = canonical_usage(um_lite, um)?;
                out.push(Metered::Usage(usage_from_metadata(&um)?));
            }
            Ok(out)
        };
        read().unwrap_or_else(|message| vec![Metered::Usage(usage_floor(frame.len(), &message))])
    }

    /// `realtimeInput`'s GA `audio` blob, else each legacy `mediaChunks[]` blob, at the format its
    /// mime states; a frame that is a `setup` or a `clientContent` carries none. Audio the plane
    /// cannot meter is refused (`Err`), never relayed unmetered: a GA `audio` blob whose mime is not
    /// a PCM rate the meter counts, or whose data does not decode in either base64 alphabet; a
    /// legacy chunk whose mime is `audio/*` and the same; and a GA `audio` blob beside any legacy
    /// audio chunk (the meter reads the GA blob alone, the far end may read both). A chunk that is
    /// not audio (video) is not audio the meter counts, and passes.
    fn uplink_audio(
        &self,
        top: &Lite<'_>,
        admit: &mut dyn FnMut(Option<AudioFormat>, usize),
    ) -> Result<(), crate::splice::Unmeterable> {
        use crate::splice::Unmeterable;
        // ProtoJSON accepts original proto names as well as lowerCamelCase. Its ordered
        // duplicate/oneof rule is last-wins (or a parse refusal), never an arbitrary spelling:
        // https://protobuf.dev/programming-guides/json/#duplicate-keys . Read the same final
        // message the far end reads, without changing one byte of the relayed frame.
        let Some((kind, ri)) = proto_member(
            top,
            &[
                "setup",
                "clientContent",
                "client_content",
                "realtimeInput",
                "realtime_input",
                "toolResponse",
                "tool_response",
            ],
        )?
        else {
            return Ok(());
        };
        if !matches!(kind, "realtimeInput" | "realtime_input") {
            return Ok(());
        }
        // Validate overwritten realtime payloads too. ProtoJSON's duplicate rule chooses the
        // final well-typed value; an earlier malformed value is not a second valid spelling.
        if let Lite::Obj(members) = top {
            for (key, value) in members {
                if matches!(key.as_ref(), "realtimeInput" | "realtime_input")
                    && !matches!(value, Lite::Null)
                    && !std::ptr::eq(value, ri)
                {
                    realtime_audio(value, &mut |_, _| {})?;
                }
            }
        }
        realtime_audio(ri, admit)
    }
}

fn realtime_audio(
    ri: &Lite<'_>,
    admit: &mut dyn FnMut(Option<AudioFormat>, usize),
) -> Result<(), crate::splice::Unmeterable> {
    use crate::splice::Unmeterable;
    let Lite::Obj(members) = ri else {
        return Err(Unmeterable);
    };
    // Validate every spelling before applying last-wins, including an overwritten object.
    for (key, value) in members {
        match (key.as_ref(), value) {
            ("audio", Lite::Null) | ("mediaChunks" | "media_chunks", Lite::Null) => {}
            ("audio", value) => validate_blob(value)?,
            ("mediaChunks" | "media_chunks", Lite::Arr(chunks)) => {
                for chunk in chunks {
                    validate_blob(chunk)?;
                }
            }
            ("mediaChunks" | "media_chunks", _) => return Err(Unmeterable),
            _ => {}
        }
    }
    let is_audio = |b: &Lite<'_>| -> Result<bool, Unmeterable> {
        Ok(proto_text(b, &["mimeType", "mime_type"])?
            .get(..AUDIO_MIME_PREFIX.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(AUDIO_MIME_PREFIX)))
    };
    let mut blob = |b: &Lite<'_>| match (
        audio_format_from_mime(proto_text(b, &["mimeType", "mime_type"])?),
        base64_len_any_alphabet(proto_text(b, &["data"])?),
    ) {
        (Some(format), Some(len)) => {
            admit(Some(format), len);
            Ok(())
        }
        _ => Err(Unmeterable),
    };
    let chunks = match proto_member(ri, &["mediaChunks", "media_chunks"])? {
        Some((_, Lite::Arr(chunks))) => chunks.as_slice(),
        Some(_) => return Err(Unmeterable),
        None => &[],
    };
    let audio = proto_member(ri, &["audio"])?;
    for chunk in chunks {
        if is_audio(chunk)? {
            if audio.is_some() {
                return Err(Unmeterable);
            }
            blob(chunk)?;
        }
    }
    if let Some((_, audio)) = audio {
        blob(audio)?;
    }
    Ok(())
}

/// A Blob's known scalar fields must be well-typed even when a later alias replaces them.
/// Bytes use ProtoJSON's accepted standard/URL-safe and padded/unpadded base64 forms.
fn validate_blob(blob: &Lite<'_>) -> Result<(), crate::splice::Unmeterable> {
    use crate::splice::Unmeterable;
    proto_text(blob, &["mimeType", "mime_type"])?;
    proto_text(blob, &["data"])?;
    let Lite::Obj(members) = blob else {
        return Err(Unmeterable);
    };
    for (key, value) in members {
        if key == "data" {
            if let Lite::Str(data) = value {
                if base64_len_any_alphabet(data).is_none() {
                    return Err(Unmeterable);
                }
            }
        }
    }
    Ok(())
}

/// Select the same ordered field in the borrowed shape and the numeric JSON value. Value keeps
/// the last exact spelling; Lite also retains the ordering between its two accepted spellings.
fn proto_value<'a, 'frame>(
    top: &'a Lite<'frame>,
    value: &'a Value,
    names: &[&str],
) -> Result<Option<(&'a Lite<'frame>, &'a Value)>, String> {
    proto_member(top, names)
        .map_err(|_| "ambiguous Gemini field".to_owned())?
        .map(|(key, lite)| {
            value
                .get(key)
                .map(|value| (lite, value))
                .ok_or_else(|| "unreadable Gemini field".to_owned())
        })
        .transpose()
}

/// Only schema fields the usage meter consumes are named here. Arbitrary caller/tool payloads
/// and the relayed frame remain untouched. Resolve aliases before the existing count validation.
fn canonical_usage(top: &Lite<'_>, value: &Value) -> Result<Value, String> {
    if !value.is_object() {
        return Err("Gemini usageMetadata is not an object".into());
    }
    let mut out = serde_json::Map::new();
    for (camel, snake) in [
        ("promptTokenCount", "prompt_token_count"),
        ("responseTokenCount", "response_token_count"),
        ("totalTokenCount", "total_token_count"),
        ("cachedContentTokenCount", "cached_content_token_count"),
        ("toolUsePromptTokenCount", "tool_use_prompt_token_count"),
        ("thoughtsTokenCount", "thoughts_token_count"),
        ("promptTokensDetails", "prompt_tokens_details"),
        ("responseTokensDetails", "response_tokens_details"),
        (
            "toolUsePromptTokensDetails",
            "tool_use_prompt_tokens_details",
        ),
    ] {
        if let Some((lite, value)) = proto_value(top, value, &[camel, snake])? {
            let value = if camel.ends_with("Details") {
                let (Lite::Arr(items), Some(values)) = (lite, value.as_array()) else {
                    return Err("Gemini token details is not an array".into());
                };
                let mut details = Vec::new();
                for (item, value) in items.iter().zip(values) {
                    if !value.is_object() {
                        return Err("Gemini token detail is not an object".into());
                    }
                    let mut detail = serde_json::Map::new();
                    for (name, alias) in [("modality", "modality"), ("tokenCount", "token_count")] {
                        if let Some((_, value)) = proto_value(item, value, &[name, alias])? {
                            detail.insert(name.into(), value.clone());
                        }
                    }
                    details.push(Value::Object(detail));
                }
                Value::Array(details)
            } else {
                value.clone()
            };
            out.insert(camel.into(), value);
        }
    }
    Ok(Value::Object(out))
}

/// A known ProtoJSON field (or oneof), resolved in original wire order across its known
/// spellings. Null is unset. A null beside another spelling is refused: peers may clear the
/// preceding field or ignore the null, so neither interpretation may admit unmetered audio.
fn proto_member<'a, 'frame>(
    top: &'a Lite<'frame>,
    names: &[&str],
) -> Result<Option<(&'a str, &'a Lite<'frame>)>, crate::splice::Unmeterable> {
    use crate::splice::Unmeterable;
    let Lite::Obj(members) = top else {
        return Err(Unmeterable);
    };
    let mut selected = None;
    let mut saw_null = false;
    for (key, value) in members.iter().filter(|(k, _)| names.contains(&k.as_ref())) {
        let null = matches!(value, Lite::Null);
        if selected.is_some() && (saw_null || null) {
            return Err(Unmeterable);
        }
        saw_null |= null;
        selected = Some((key.as_ref(), value));
    }
    Ok(selected.filter(|(_, value)| !matches!(value, Lite::Null)))
}

/// A known string field; absent/null is its protobuf default, an empty string. A malformed
/// selected value is refused rather than silently reinterpreted as that default.
fn proto_text<'a, 'frame>(
    top: &'a Lite<'frame>,
    names: &[&str],
) -> Result<&'a str, crate::splice::Unmeterable> {
    if let Lite::Obj(members) = top {
        for (key, value) in members {
            if names.contains(&key.as_ref()) && !matches!(value, Lite::Str(_) | Lite::Null) {
                return Err(crate::splice::Unmeterable);
            }
        }
    }
    match proto_member(top, names)? {
        Some((_, Lite::Str(text))) => Ok(text),
        Some(_) => Err(crate::splice::Unmeterable),
        None => Ok(""),
    }
}

/// The mime prefix of an audio blob (matched ignoring case, as [`audio_format_from_mime`] reads
/// the mime).
const AUDIO_MIME_PREFIX: &str = "audio/";

#[cfg(test)]
mod tests;
