// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! GEMINI LIVE CODEC TESTS — what the meter reads off the second dialect (the usage a
//! `usageMetadata` reports wherever it rides, the tool calls a `toolCall` opens, the uplink audio a
//! `realtimeInput` carries at the rate its mime states) and the `setup` a locked session is told.

use super::*;
use crate::splice::LockedKeys;

// ── helpers ──────────────────────────────────────────────────────────────────────────────────────

fn b64(bytes: &[u8]) -> String {
    busbar_contract::media::base64_encode(bytes)
}

fn down(src: &Value) -> Vec<Metered> {
    GeminiLiveCodec.meter_down(src.to_string().as_bytes())
}

/// The audio the meter counts in a caller frame: each blob's stated format and decoded length.
fn up(src: &Value) -> Vec<(Option<AudioFormat>, usize)> {
    let bytes = src.to_string().into_bytes();
    let top = Lite::read(&bytes).expect("the frame is JSON");
    let mut counted = Vec::new();
    GeminiLiveCodec.uplink_audio(&top, &mut |format, len| counted.push((format, len)));
    counted
}

// ── the locked `setup` ───────────────────────────────────────────────────────────────────────────

#[test]
fn the_locked_setup_is_told_in_gemini_shape() {
    let locked = SessionConfig {
        model: Some("models/gemini-2.0-flash-exp".into()),
        modalities: vec!["audio".into()],
        instructions: Some("You are a helpful voice agent.".into()),
        voice: Some("Puck".into()),
        turn_detection: Some(Some(IrVad::ServerVad {
            threshold: 0.5,
            prefix_padding_ms: 300,
            silence_duration_ms: 200,
            create_response: true,
            interrupt_response: true,
        })),
        tools: vec![json!({ "functionDeclarations": [ { "name": "lookup" } ] })],
        max_output_tokens: Some(MaxOutputTokens::Limit(2048)),
        ..SessionConfig::default()
    };
    let frame = GeminiLiveCodec
        .locked_frame(&locked)
        .expect("a Gemini session is told its locked setup");
    let back: Value = serde_json::from_slice(&frame).expect("the frame is JSON");
    assert_eq!(back, setup_from_session_config(&locked));
    let s = &back["setup"];
    assert_eq!(s["model"], "models/gemini-2.0-flash-exp");
    assert_eq!(
        s["generationConfig"]["responseModalities"][0], "AUDIO",
        "modalities uppercased to Gemini tokens"
    );
    assert_eq!(
        s["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"],
        "Puck"
    );
    assert_eq!(s["generationConfig"]["maxOutputTokens"], 2048);
    assert_eq!(
        s["systemInstruction"]["parts"][0]["text"],
        "You are a helpful voice agent."
    );
    assert_eq!(
        s["realtimeInputConfig"]["automaticActivityDetection"]["prefixPaddingMs"],
        300
    );
    assert_eq!(s["tools"][0]["functionDeclarations"][0]["name"], "lookup");
}

/// RED-BEFORE-GREEN (THE DESIGN section 7, "Where the protocol carries a place to say so"; the
/// cross-dialect map's `openai_structured_error`: "Gemini reports errors via WebSocket close codes,
/// not a structured in-band error message"): a Gemini session is told no error frame. RED: it was
/// told a `serverContent.error` member no Gemini client reads.
#[test]
fn a_gemini_session_is_told_no_in_band_error() {
    let framed =
        GeminiLiveCodec.ceiling_frame("session_expired", "the session reached its ceiling");
    assert!(framed.is_none(), "{framed:?}");
}

// ── realtimeInput (uplink audio counted) ─────────────────────────────────────────────────────────

#[test]
fn realtime_input_audio_counts_each_blob_at_its_stated_rate() {
    let payload = b"pretend-uplink-audio-bytes";
    // The LEGACY `mediaChunks[]` spelling is still read (a peer may speak it): one count per chunk.
    assert_eq!(
        up(&json!({ "realtimeInput": { "mediaChunks": [
            { "mimeType": "audio/pcm;rate=16000", "data": b64(payload) },
            { "mimeType": "audio/pcm;rate=24000", "data": b64(b"b") },
            { "mimeType": "image/png", "data": b64(b"c") }
        ] } })),
        vec![
            (Some(AudioFormat::Pcm16At16k), payload.len()),
            (Some(AudioFormat::Pcm16), 1)
        ],
        "each audio chunk at its own rate; a non-audio chunk is not audio"
    );
    // GA (`v1beta`) shape: realtimeInput.audio is a SINGLE inline blob.
    assert_eq!(
        up(
            &json!({ "realtimeInput": { "audio": { "mimeType": "audio/pcm;rate=16000", "data": b64(payload) } } })
        ),
        vec![(Some(AudioFormat::Pcm16At16k), payload.len())]
    );
}

#[test]
fn realtime_input_prefers_ga_audio_over_media_chunks() {
    // When BOTH spellings are present the GA single blob wins (one count), so a GA peer's audio is
    // never counted twice.
    let src = json!({
        "realtimeInput": {
            "audio": { "mimeType": "audio/pcm;rate=16000", "data": b64(b"ga") },
            "mediaChunks": [ { "mimeType": "audio/pcm;rate=16000", "data": b64(b"legacy") } ]
        }
    });
    assert_eq!(up(&src), vec![(Some(AudioFormat::Pcm16At16k), 2)]);
}

/// An uplink blob whose payload is not base64 counts nothing, never an empty frame; and a frame
/// that is a `setup` or a `clientContent` carries no audio the meter counts.
#[test]
fn an_undecodable_or_misplaced_gemini_audio_payload_counts_nothing() {
    let blob = json!({ "mimeType": "audio/pcm;rate=16000", "data": b64(b"x") });
    assert!(up(&json!({ "realtimeInput": { "audio": { "mimeType": "audio/pcm;rate=16000", "data": "!!!!" } } })).is_empty());
    assert!(up(&json!({ "realtimeInput": { "audio": blob }, "setup": {} })).is_empty());
    assert!(up(&json!({ "realtimeInput": { "audio": blob }, "clientContent": {} })).is_empty());
    assert!(up(&json!({ "realtimeInput": { "audioStreamEnd": true } })).is_empty());
}

// ── tool calls and what is not metered ───────────────────────────────────────────────────────────

#[test]
fn each_call_a_tool_call_delivers_is_one_opened_call() {
    let src = json!({
        "toolCall": { "functionCalls": [
            { "id": "fc_1", "name": "get_weather", "args": { "city": "SF" } },
            { "id": "fc_2", "name": "get_time", "args": {} }
        ] }
    });
    assert_eq!(down(&src), vec![Metered::ToolOpen, Metered::ToolOpen]);
}

#[test]
fn transcription_audio_and_unknown_frames_meter_nothing() {
    for src in [
        json!({ "serverContent": { "modelTurn": { "parts": [
            { "inlineData": { "mimeType": "audio/pcm;rate=24000", "data": b64(&[1, 2]) } }
        ] } } }),
        json!({ "serverContent": { "inputTranscription": { "text": "hi" } } }),
        json!({ "serverContent": { "interrupted": true } }),
        json!({ "setupComplete": {} }),
        json!({ "setupComplete": {}, "usageMetadata": { "promptTokenCount": 5 } }),
        json!({ "toolCallCancellation": { "ids": ["fc_1"] } }),
        json!({
            "serverContent": { "turnComplete": true },
            "toolCall": { "functionCalls": [ { "id": "fc_1", "name": "f", "args": {} } ] }
        }),
        json!({ "goAway": { "timeLeft": "10s" } }),
    ] {
        assert!(down(&src).is_empty(), "{src}");
    }
    assert!(GeminiLiveCodec.meter_down(b"not json").is_empty());
}

// ── usageMetadata extraction (`BUSBAR-1.6.0.md` #18/#45) ──────────────────────────────────────────────────────────────

#[test]
fn usage_metadata_extracts_split_token_classes() {
    let src = json!({
        "usageMetadata": {
            "promptTokenCount": 95,
            "responseTokenCount": 50,
            "totalTokenCount": 145,
            "cachedContentTokenCount": 5,
            "promptTokensDetails": [
                { "modality": "AUDIO", "tokenCount": 80 },
                { "modality": "TEXT", "tokenCount": 15 }
            ],
            "responseTokensDetails": [
                { "modality": "AUDIO", "tokenCount": 40 },
                { "modality": "TEXT", "tokenCount": 10 }
            ]
        }
    });
    let u = &usage_of(src["usageMetadata"].clone());
    assert_eq!(u.audio_in, 80);
    assert_eq!(u.text_in, 15);
    assert_eq!(u.audio_out, 40);
    assert_eq!(u.text_out, 10);
    assert_eq!(u.cached, 5);
}

/// RED-BEFORE-GREEN (MONEY-AUDIT STR-4): `usageMetadata` is a TOP-LEVEL field Gemini Live may send on
/// the same message as `serverContent` (typically the `turnComplete` frame) or `toolCall`. The reader
/// dispatched on the message type and returned before it looked at `usageMetadata`, so usage riding on
/// a content frame produced no `Usage` event and the turn billed zero tokens.
#[test]
fn usage_metadata_riding_on_a_content_frame_is_billed() {
    let usage = json!({
        "promptTokenCount": 95,
        "responseTokenCount": 50,
        "totalTokenCount": 145,
        "promptTokensDetails": [
            { "modality": "AUDIO", "tokenCount": 80 },
            { "modality": "TEXT", "tokenCount": 15 }
        ],
        "responseTokensDetails": [
            { "modality": "AUDIO", "tokenCount": 40 },
            { "modality": "TEXT", "tokenCount": 10 }
        ]
    });
    let frames = [
        json!({ "serverContent": { "turnComplete": true }, "usageMetadata": usage }),
        json!({
            "toolCall": { "functionCalls": [ { "id": "fc_1", "name": "f", "args": {} } ] },
            "usageMetadata": usage
        }),
    ];
    for (src, opened) in frames.into_iter().zip([0, 1]) {
        let ir = down(&src);
        let usages: Vec<&IrDuplexUsage> = ir
            .iter()
            .filter_map(|e| match e {
                Metered::Usage(u) => Some(u),
                _ => None,
            })
            .collect();
        assert_eq!(usages.len(), 1, "one usage report for {src}: {ir:?}");
        let u = usages[0];
        assert_eq!(
            (u.audio_in, u.text_in, u.audio_out, u.text_out),
            (80, 15, 40, 10)
        );
        // The tool call the frame opens is counted ahead of the usage that closes the turn.
        assert_eq!(ir.len(), opened + 1, "{ir:?}");
        assert!(matches!(ir.last(), Some(Metered::Usage(_))), "{ir:?}");
    }
}

/// RED-BEFORE-GREEN: a MISSING modality breakdown must not meter the turn at zero.
///
/// Gemini can omit `promptTokensDetails`/`responseTokensDetails` while still stating the turn's
/// TOTAL token counts (`promptTokenCount`/`responseTokenCount`). The breakdown is a REFINEMENT of the
/// stated total, never its sole source, so reading a missing breakdown as zero tokens silently bills
/// a real turn at zero — the one outcome #42 forbids outside billing being off. Billing must fall
/// back to the stated total when the per-modality breakdown yields nothing.
#[test]
fn usage_falls_back_to_stated_totals_when_the_modality_breakdown_is_absent() {
    let src = json!({
        "usageMetadata": {
            "promptTokenCount": 95,
            "responseTokenCount": 50,
            "totalTokenCount": 145
        }
    });
    let u = &usage_of(src["usageMetadata"].clone());
    assert_eq!(
        u.audio_in + u.text_in,
        95,
        "the turn's stated prompt total must not be dropped to zero"
    );
    assert_eq!(
        u.audio_out + u.text_out,
        50,
        "the turn's stated response total must not be dropped to zero"
    );
    // The served meter bills the stated totals across its input and output classes, never zero.
    let billed = crate::session::class_counts(Some(u), crate::session::TurnCounters::default());
    let sum_of = |names: &[&str]| -> u64 {
        billed
            .iter()
            .filter(|(c, _)| names.contains(&c.as_str()))
            .map(|(_, n)| n)
            .sum()
    };
    assert_eq!(
        sum_of(&["audio_tokens_in", "text_tokens_in"]),
        95,
        "the input classes bill the stated total, never zero"
    );
    assert_eq!(
        sum_of(&["audio_tokens_out", "text_tokens_out"]),
        50,
        "the output classes bill the stated total, never zero"
    );
}

/// The usage a Gemini `usageMetadata` object reads as.
fn usage_of(meta: Value) -> IrDuplexUsage {
    match down(&json!({ "usageMetadata": meta })).as_slice() {
        [Metered::Usage(u)] => *u,
        other => panic!("expected one usage report, got {other:?}"),
    }
}

/// RED-BEFORE-GREEN (MONEY; the `Modality` enum names TEXT, IMAGE, VIDEO, AUDIO and DOCUMENT): a
/// turn whose prompt carried camera frames bills its IMAGE and VIDEO tokens, each under its own
/// class, and the input classes sum to the prompt. RED: only the AUDIO and TEXT entries were read,
/// so the image and video tokens billed zero.
#[test]
fn image_and_video_prompt_tokens_are_billed() {
    let u = usage_of(json!({
        "promptTokenCount": 160,
        "responseTokenCount": 50,
        "totalTokenCount": 210,
        "promptTokensDetails": [
            { "modality": "AUDIO", "tokenCount": 80 },
            { "modality": "TEXT", "tokenCount": 15 },
            { "modality": "IMAGE", "tokenCount": 25 },
            { "modality": "VIDEO", "tokenCount": 40 }
        ],
        "responseTokensDetails": [
            { "modality": "AUDIO", "tokenCount": 40 },
            { "modality": "TEXT", "tokenCount": 10 }
        ]
    }));
    assert_eq!(
        (u.audio_in, u.text_in, u.image_in, u.video_in),
        (80, 15, 25, 40),
        "{u:?}"
    );
    assert_eq!((u.audio_out, u.text_out), (40, 10), "{u:?}");
    let billed = crate::session::class_counts(Some(&u), crate::session::TurnCounters::default());
    for (class, n) in [
        (crate::meta::CLASS_IMAGE_TOKENS_IN, 25),
        (crate::meta::CLASS_VIDEO_TOKENS_IN, 40),
    ] {
        assert!(billed.contains(&(class, n)), "{class:?}: {billed:?}");
    }
}

/// RED-BEFORE-GREEN (MONEY; the owner's gemini-tool-use-billing ruling: "a grounded Gemini turn
/// bills Google's total, tool-use prompt tokens included"; `toolUsePromptTokenCount` is the "Number
/// of tokens present in tool-use prompt(s)"): the tool-use prompt bills as input. RED: the field was
/// never read and billed zero.
#[test]
fn tool_use_prompt_tokens_are_billed() {
    let u = usage_of(json!({
        "promptTokenCount": 95,
        "toolUsePromptTokenCount": 30,
        "responseTokenCount": 50,
        "totalTokenCount": 175,
        "promptTokensDetails": [
            { "modality": "AUDIO", "tokenCount": 80 },
            { "modality": "TEXT", "tokenCount": 15 }
        ],
        "toolUsePromptTokensDetails": [ { "modality": "TEXT", "tokenCount": 30 } ]
    }));
    assert_eq!((u.audio_in, u.text_in), (80, 45), "{u:?}");
    assert_eq!(u.audio_out + u.text_out, 50, "{u:?}");
}

/// RED-BEFORE-GREEN (MONEY; `thoughtsTokenCount` is the "Number of tokens of thoughts for thinking
/// models", and Google's price list states "Output price (including thinking tokens)"): a thinking
/// model's thought tokens bill as output. RED: the field was never read and billed zero.
#[test]
fn thought_tokens_are_billed_as_output() {
    let u = usage_of(json!({
        "promptTokenCount": 95,
        "responseTokenCount": 50,
        "thoughtsTokenCount": 120,
        "totalTokenCount": 265,
        "responseTokensDetails": [
            { "modality": "AUDIO", "tokenCount": 40 },
            { "modality": "TEXT", "tokenCount": 10 }
        ]
    }));
    assert_eq!(u.audio_in + u.text_in, 95, "{u:?}");
    assert_eq!((u.audio_out, u.text_out), (40, 130), "{u:?}");
}

/// RED-BEFORE-GREEN (MONEY; the owner's ruling bills "Google's total"): where `totalTokenCount`
/// states more than the prompt, tool-use prompt, response and thoughts it breaks into, the
/// remainder bills as input, so the turn's classes sum to Google's total. RED: the total was never
/// read and the remainder billed zero.
#[test]
fn googles_total_is_billed_when_it_exceeds_its_parts() {
    let u = usage_of(json!({
        "promptTokenCount": 95,
        "responseTokenCount": 50,
        "totalTokenCount": 160
    }));
    assert_eq!(u.audio_in + u.text_in, 110, "{u:?}");
    assert_eq!(u.audio_out + u.text_out, 50, "{u:?}");
}

#[test]
fn cached_content_tokens_are_not_billed_twice() {
    // Gemini reports `cachedContentTokenCount` as a SUBSET of `promptTokenCount` (cached content IS
    // part of the prompt), and `promptTokensDetails` is that same prompt broken out by modality. So
    // the billing fold must bill the UNCACHED remainder as `input` and the cached portion as
    // `cache_read` — the same netting the OpenAI dialect needs, not a per-dialect divergence.
    let src = json!({
        "usageMetadata": {
            "promptTokenCount": 1000,
            "responseTokenCount": 0,
            "totalTokenCount": 1000,
            "cachedContentTokenCount": 800,
            "promptTokensDetails": [
                { "modality": "AUDIO", "tokenCount": 1000 },
                { "modality": "TEXT", "tokenCount": 0 }
            ],
            "responseTokensDetails": []
        }
    });
    let u = &usage_of(src["usageMetadata"].clone());
    // Extraction stays wire-faithful.
    assert_eq!(u.audio_in, 1000);
    assert_eq!(u.cached, 800);
    // The served meter bills the input as reported: the cached subset is part of it, billed once
    // there, and no class bills it again (cached tokens are attribution only).
    let billed = crate::session::class_counts(Some(u), crate::session::TurnCounters::default());
    assert!(
        billed
            .iter()
            .all(|(c, _)| c.as_str() != crate::meta::CLASS_CACHED_TOKENS.as_str()),
        "the cached subset is never a class of its own: {billed:?}"
    );
    assert_eq!(
        billed.iter().map(|(_, n)| n).sum::<u64>(),
        1000,
        "the billed classes sum to the turn's input, never to 1800"
    );
}

// ── audio-format mime probe ──────────────────────────────────────────────────────────────────────

#[test]
fn audio_format_from_mime_probe() {
    // Each PCM rate is its own format, because the uplink meter divides by its rate.
    assert_eq!(
        audio_format_from_mime("audio/pcm;rate=24000"),
        Some(AudioFormat::Pcm16)
    );
    assert_eq!(
        audio_format_from_mime("audio/pcm;rate=16000"),
        Some(AudioFormat::Pcm16At16k)
    );
    assert_eq!(
        audio_format_from_mime("audio/pcm"),
        Some(AudioFormat::Pcm16At16k),
        "an untagged uplink blob is the uplink's own 16 kHz"
    );
    assert_eq!(audio_format_from_mime("text/plain"), None);
    assert_eq!(audio_format_from_mime("video/mp4"), None);
}

/// RED-BEFORE-GREEN: the rate parameter must be matched EXACTLY, never by substring.
///
/// `"rate=160000"` (10x the true 16 kHz rate) CONTAINS `"rate=16000"` as a literal substring, so a
/// `.contains("rate=16000")` probe mismeasures it as the 16 kHz rate it is not — a silent 10x error
/// in billed audio duration. A rate that IS stated but recognized as neither 16 kHz nor 24 kHz must
/// be REFUSED (`None`), never guessed at as the nearest-looking known value.
#[test]
fn a_stated_but_unrecognized_rate_is_refused_not_substring_matched() {
    assert_eq!(
        audio_format_from_mime("audio/pcm;rate=160000"),
        None,
        "160000 is a stated rate this dialect does not recognize, not 16 kHz"
    );
    assert_eq!(
        audio_format_from_mime("audio/pcm;rate=240000"),
        None,
        "240000 is a stated rate this dialect does not recognize, not an untagged blob"
    );
}
