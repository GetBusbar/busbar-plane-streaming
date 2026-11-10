// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The second dialect, Google **Gemini Live** (`BidiGenerateContent`): what a session meters off each frame.

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

/// The mime prefix of an audio blob (matched ignoring case, as [`audio_format_from_mime`] reads
/// the type): a blob of another type is not audio, and is not metered as audio.
const AUDIO_MIME_PREFIX: &str = "audio/";

/// The one audio media type the meter counts: linear PCM16 (`audio/pcm`, its `rate` a parameter).
const PCM_MIME: &str = "audio/pcm";

/// Map a Gemini UPLINK audio `mimeType` to the shared [`AudioFormat`] its bytes are metered at.
/// The media type must be exactly `audio/pcm` (ignoring case; `audio/pcmu` and the like are not
/// linear PCM), and its `rate` parameter (a token or a quoted string, named at most once with one
/// value) 16000, 24000, or absent (Gemini Live's 16 kHz). Anything else is `None`: refused as
/// audio the plane cannot meter, never counted at a rate it was not sent at.
#[must_use]
pub fn audio_format_from_mime(mime: &str) -> Option<AudioFormat> {
    let mut parts = mime.split(';');
    if !parts.next()?.trim().eq_ignore_ascii_case(PCM_MIME) {
        return None;
    }
    let mut rate: Option<&str> = None;
    for param in parts {
        if param.trim().is_empty() {
            continue;
        }
        let (name, value) = param.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("rate") {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value);
        if rate.is_some_and(|stated| stated != value) {
            return None;
        }
        rate = Some(value);
    }
    match rate {
        None | Some("16000") => Some(AudioFormat::Pcm16At16k),
        Some("24000") => Some(AudioFormat::Pcm16),
        Some(_) => None,
    }
}

/// The `setup` members a session locked to `cfg` states, as the governed splice puts them over a caller setup.
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

/// THE `setup` FRAME a session locked to `cfg` is told, with the locked session's members in Gemini's wire shape.
pub(crate) fn setup_from_session_config(cfg: &SessionConfig) -> Value {
    let mut setup = serde_json::Map::new();
    if let Some(model) = &cfg.model {
        let model = if model.contains('/') {
            model.clone()
        } else {
            format!("models/{model}")
        };
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
    if let Some(window) = cfg.context_window_tokens {
        // The context left once the response's own output is room, as the OpenAI dialect's
        // truncation limit reserves it: a full answer on a context at the trigger stays within
        // the window.
        let trigger = match cfg.max_output_tokens {
            Some(MaxOutputTokens::Limit(n)) => window.saturating_sub(n),
            _ => window,
        };
        setup.insert(
            "contextWindowCompression".into(),
            json!({ "triggerTokens": trigger, "slidingWindow": {} }),
        );
    }

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
                "disabled": false,
                "prefixPaddingMs": prefix_padding_ms,
                "silenceDurationMs": silence_duration_ms,
            }});
            // These defaults must be explicit: setup is merged over the caller's object,
            // so omission would preserve a conflicting caller policy.
            config["activityHandling"] = json!(if *interrupt_response {
                "START_OF_ACTIVITY_INTERRUPTS"
            } else {
                "NO_INTERRUPTION"
            });
            setup.insert("realtimeInputConfig".into(), config);
        }
        Some(Some(IrVad::SemanticVad { .. })) => {
            setup.insert(
                "realtimeInputConfig".into(),
                json!({ "automaticActivityDetection": { "disabled": false } }),
            );
        }
    }
    json!({ wire::SETUP: Value::Object(setup) })
}

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

/// Extract the split token classes from a Gemini `usageMetadata` object (audio vs text are attributed separately).
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

/// Read only fields used by billing; large audio payloads remain borrowed in the Lite tree.
fn meter_value(frame: &[u8]) -> Result<Value, serde_json::Error> {
    struct Billing;
    impl<'de> serde::de::Visitor<'de> for Billing {
        type Value = Value;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a Gemini server message")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
            let mut out = serde_json::Map::new();
            while let Some(key) = map.next_key::<String>()? {
                if matches!(
                    key.as_str(),
                    "usageMetadata" | "usage_metadata" | "toolCall" | "tool_call"
                ) {
                    out.insert(key, map.next_value()?);
                } else {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
            Ok(Value::Object(out))
        }
    }
    let mut de = serde_json::Deserializer::from_slice(frame);
    let value = serde::Deserializer::deserialize_map(&mut de, Billing)?;
    de.end()?;
    Ok(value)
}

impl DuplexReader for GeminiLiveCodec {
    /// Each call a `toolCall` opens (a frame that is a `setupComplete` or a `serverContent` opens no calls).
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
        let Some(top) = Lite::read(frame) else {
            // Valid JSON the meter will not read whole (more values than it reads) may carry a
            // usage report: it bills the floor. Bytes that are not JSON at all are said, once each.
            if serde_json::from_slice::<serde::de::IgnoredAny>(frame).is_ok() {
                return vec![Metered::Usage(usage_floor(
                    frame.len(),
                    "a Gemini frame holds more values than the meter reads",
                ))];
            }
            tracing::warn!(
                delivered = frame.len(),
                "a Gemini frame is not JSON: nothing in it is metered"
            );
            return Vec::new();
        };
        let Ok(v) = meter_value(frame) else {
            return Vec::new();
        };
        let read = || -> Result<Vec<Metered>, String> {
            let kind = oneof(&top, SERVER_KINDS).map_err(|_| {
                "a Gemini message that is not an object, or names more than one kind".to_owned()
            })?;
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
            // Usage metadata is read separately so a malformed report does not discard tool-open
            // entries already accumulated above: the floor covers only the usage, not the tools.
            let usage = proto_value(&top, &v, &["usageMetadata", "usage_metadata"]).and_then(
                |um| match um {
                    Some((um_lite, um)) => canonical_usage(um_lite, um)
                        .and_then(|um| usage_from_metadata(&um).map(Some)),
                    None => Ok(None),
                },
            );
            match usage {
                Ok(Some(u)) => out.push(Metered::Usage(u)),
                Ok(None) => {}
                Err(message) => out.push(Metered::Usage(usage_floor(frame.len(), &message))),
            }
            Ok(out)
        };
        read().unwrap_or_else(|message| vec![Metered::Usage(usage_floor(frame.len(), &message))])
    }

    /// `realtimeInput`'s GA `audio` blob, else each legacy `mediaChunks[]` blob, at the format its MIME type states.
    fn uplink_audio(
        &self,
        top: &Lite<'_>,
        admit: &mut dyn FnMut(Option<AudioFormat>, usize),
    ) -> Result<(), crate::splice::Unmeterable> {
        let Some((kind, ri)) = oneof(top, CLIENT_KINDS)? else {
            return Ok(());
        };
        if matches!(kind, "clientContent" | "client_content") {
            return client_audio(ri, admit);
        }
        if !matches!(kind, "realtimeInput" | "realtime_input") {
            return Ok(());
        }

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
            count_blob(chunk, admit)?;
        }
    }
    if let Some((_, audio)) = audio {
        count_blob(audio, admit)?;
    }
    Ok(())
}

/// Whether the Blob `b` is audio: its mime type is `audio/...`, ignoring case.
fn is_audio(b: &Lite<'_>) -> Result<bool, crate::splice::Unmeterable> {
    let mime = proto_text(b, &["mimeType", "mime_type"])?;
    let trimmed = mime.trim_start();
    Ok(trimmed
        .get(..AUDIO_MIME_PREFIX.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(AUDIO_MIME_PREFIX)))
}

/// Count the audio Blob `b` at the format its mime type states: `Err` for a format the plane
/// cannot meter or data that is not base64.
fn count_blob(
    b: &Lite<'_>,
    admit: &mut dyn FnMut(Option<AudioFormat>, usize),
) -> Result<(), crate::splice::Unmeterable> {
    match (
        audio_format_from_mime(proto_text(b, &["mimeType", "mime_type"])?),
        base64_len_any_alphabet(proto_text(b, &["data"])?),
    ) {
        (Some(format), Some(len)) => {
            admit(Some(format), len);
            Ok(())
        }
        _ => Err(crate::splice::Unmeterable),
    }
}

/// The audio a `clientContent` message carries in its turns: every part's `inlineData` Blob that
/// is audio, counted as a `realtimeInput` blob is (the model hears both alike). A turn list or a
/// part list that is not a list is `Err`.
fn client_audio(
    content: &Lite<'_>,
    admit: &mut dyn FnMut(Option<AudioFormat>, usize),
) -> Result<(), crate::splice::Unmeterable> {
    use crate::splice::Unmeterable;
    let turns = match proto_member(content, &["turns"])? {
        Some((_, Lite::Arr(turns))) => turns.as_slice(),
        Some(_) => return Err(Unmeterable),
        None => &[],
    };
    for turn in turns {
        let parts = match proto_member(turn, &["parts"])? {
            Some((_, Lite::Arr(parts))) => parts.as_slice(),
            Some(_) => return Err(Unmeterable),
            None => &[],
        };
        for part in parts {
            if let Some((_, blob)) = proto_member(part, &["inlineData", "inline_data"])? {
                validate_blob(blob)?;
                if is_audio(blob)? {
                    count_blob(blob, admit)?;
                }
            }
        }
    }
    Ok(())
}

/// The `BidiGenerateContentServerMessage` oneof's members, each with its snake_case alias.
const SERVER_KINDS: &[[&str; 2]] = &[
    ["setupComplete", "setup_complete"],
    ["serverContent", "server_content"],
    ["toolCall", "tool_call"],
    ["toolCallCancellation", "tool_call_cancellation"],
    ["goAway", "go_away"],
    ["sessionResumptionUpdate", "session_resumption_update"],
];

/// The `BidiGenerateContentClientMessage` oneof's members, each with its snake_case alias.
const CLIENT_KINDS: &[[&str; 2]] = &[
    ["setup", "setup"],
    ["clientContent", "client_content"],
    ["realtimeInput", "realtime_input"],
    ["toolResponse", "tool_response"],
];

/// The one member of the oneof `kinds` that `top` sets: `Err` where it sets more than one (a
/// reader that took either would read another message), or where `top` is not an object.
fn oneof<'a, 'frame>(
    top: &'a Lite<'frame>,
    kinds: &[[&str; 2]],
) -> Result<Option<(&'a str, &'a Lite<'frame>)>, crate::splice::Unmeterable> {
    let mut found = None;
    for names in kinds {
        if let Some(kind) = proto_member(top, names)? {
            if found.is_some() {
                return Err(crate::splice::Unmeterable);
            }
            found = Some(kind);
        }
    }
    Ok(found)
}

/// A Blob's known scalar fields must be well-typed even when a later alias replaces them.
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

/// Select the same ordered field in the borrowed shape and the numeric JSON value. Value keeps the final duplicate key.
fn proto_value<'a, 'frame>(
    top: &'a Lite<'frame>,
    value: &'a Value,
    names: &[&str],
) -> Result<Option<(&'a Lite<'frame>, &'a Value)>, String> {
    proto_member(top, names)
        .map_err(|_| {
            "a Gemini field set to null and to a value, or read off a non-object".to_owned()
        })?
        .map(|(key, lite)| {
            value
                .get(key)
                .map(|value| (lite, value))
                .ok_or_else(|| "unreadable Gemini field".to_owned())
        })
        .transpose()
}

/// Only schema fields the usage meter consumes are named here. Arbitrary caller/tool payloads remain opaque.
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

/// A known ProtoJSON field (or oneof), resolved in original wire order across its known camelCase and snake_case aliases.
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

/// A known string field; absent/null is its protobuf default, an empty string. A malformed scalar is refused.
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

#[cfg(test)]
mod tests;
