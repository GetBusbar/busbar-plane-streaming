// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The second dialect, Google **Gemini Live** (`BidiGenerateContent`): what a session meters off

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

/// Map a Gemini UPLINK audio `mimeType` to the shared [`AudioFormat`] its bytes are metered at.
#[must_use]
pub fn audio_format_from_mime(mime: &str) -> Option<AudioFormat> {
    let m = mime.to_ascii_lowercase();
    if !m.starts_with("audio/pcm") {
        return None;
    }

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

    if rate_16k || untagged {
        Some(AudioFormat::Pcm16At16k)
    } else if rate_24k {
        Some(AudioFormat::Pcm16)
    } else {
        None
    }
}

/// The `setup` members a session locked to `cfg` states, as the governed splice puts them over a
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
                "prefixPaddingMs": prefix_padding_ms,
                "silenceDurationMs": silence_duration_ms,
            }});

            if !interrupt_response {
                config["activityHandling"] = json!("NO_INTERRUPTION");
            }
            setup.insert("realtimeInputConfig".into(), config);
        }
        Some(Some(IrVad::SemanticVad { .. })) => {
            setup.insert(
                "realtimeInputConfig".into(),
                json!({ "automaticActivityDetection": {} }),
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

/// Extract the split token classes from a Gemini `usageMetadata` object (audio vs text are
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

impl DuplexReader for GeminiLiveCodec {
    /// Each call a `toolCall` opens (a frame that is a `setupComplete` or a `serverContent` opens
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
    fn uplink_audio(
        &self,
        top: &Lite<'_>,
        admit: &mut dyn FnMut(Option<AudioFormat>, usize),
    ) -> Result<(), crate::splice::Unmeterable> {
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
const AUDIO_MIME_PREFIX: &str = "audio/";

#[cfg(test)]
mod tests;
