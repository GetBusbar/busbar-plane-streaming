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

use super::peek::{base64_len, Head, Lite};
use super::{billed_count, usage_floor, DuplexReader, Metered};
use crate::codec::ir::config::{IrVad, MaxOutputTokens, SessionConfig};
use crate::codec::ir::media::AudioFormat;
use crate::codec::ir::usage::IrDuplexUsage;
use serde_json::{json, Value};

/// The Gemini Live top-level message keys the meter and the locked `setup` name.
mod wire {
    pub const SETUP: &str = "setup";
    pub const CLIENT_CONTENT: &str = "clientContent";
    pub const REALTIME_INPUT: &str = "realtimeInput";
    pub const SETUP_COMPLETE: &str = "setupComplete";
    pub const SERVER_CONTENT: &str = "serverContent";
    pub const TOOL_CALL: &str = "toolCall";
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
/// shape. Modalities are UPPERCASED to Gemini's enum tokens; the OpenAI-only VAD knobs have no
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
            ..
        })) => {
            setup.insert(
                "realtimeInputConfig".into(),
                json!({ "automaticActivityDetection": {
                    "prefixPaddingMs": prefix_padding_ms,
                    "silenceDurationMs": silence_duration_ms,
                }}),
            );
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
            h.keys
                .iter()
                .any(|k| k == wire::USAGE_METADATA || k == wire::TOOL_CALL)
        });
        if !metered {
            return Vec::new();
        }
        let Ok(v) = serde_json::from_slice::<Value>(frame) else {
            return Vec::new();
        };
        if v.get(wire::SETUP_COMPLETE).is_some() {
            return Vec::new();
        }
        let opened = match (v.get(wire::SERVER_CONTENT), v.get(wire::TOOL_CALL)) {
            (None, Some(tc)) => tc
                .get("functionCalls")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            _ => 0,
        };
        let mut out = vec![Metered::ToolOpen; opened];
        if let Some(um) = v.get(wire::USAGE_METADATA) {
            // A present-but-unreadable billed count bills the plane's floor and says so,
            // never meters it at zero.
            out.push(Metered::Usage(
                usage_from_metadata(um)
                    .unwrap_or_else(|message| usage_floor(frame.len(), &message)),
            ));
        }
        out
    }

    /// `realtimeInput`'s GA `audio` blob, else each legacy `mediaChunks[]` blob, at the format its
    /// mime states; a frame that is a `setup` or a `clientContent` carries none.
    fn uplink_audio(&self, top: &Lite<'_>, admit: &mut dyn FnMut(Option<AudioFormat>, usize)) {
        if top.get(wire::SETUP).is_some() || top.get(wire::CLIENT_CONTENT).is_some() {
            return;
        }
        let Some(ri) = top.get(wire::REALTIME_INPUT) else {
            return;
        };
        let mut blob = |b: &Lite<'_>| {
            if let (Some(format), Some(len)) = (
                audio_format_from_mime(b.str_at("mimeType")),
                base64_len(b.str_at("data")),
            ) {
                admit(Some(format), len);
            }
        };
        match (ri.get("audio"), ri.get("mediaChunks")) {
            (Some(audio), _) => blob(audio),
            (None, Some(Lite::Arr(chunks))) => chunks.iter().for_each(blob),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
