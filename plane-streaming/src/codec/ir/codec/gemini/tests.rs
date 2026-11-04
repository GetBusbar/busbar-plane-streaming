// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! GEMINI LIVE CODEC TESTS — what the meter reads off the second dialect (the usage a

use super::*;
use crate::splice::LockedKeys;

fn b64(bytes: &[u8]) -> String {
    busbar_contract::media::base64_encode(bytes)
}

fn down(src: &Value) -> Vec<Metered> {
    GeminiLiveCodec.meter_down(src.to_string().as_bytes())
}

/// The audio the meter counts in a caller frame: each blob's stated format and decoded length;
fn up(src: &Value) -> Vec<(Option<AudioFormat>, usize)> {
    let bytes = src.to_string().into_bytes();
    let top = Lite::read(&bytes).expect("the frame is JSON");
    let mut counted = Vec::new();
    GeminiLiveCodec
        .uplink_audio(&top, &mut |format, len| counted.push((format, len)))
        .expect("valid audio");
    counted
}

/// Whether the meter refuses a caller frame's audio as audio it cannot meter.
fn refused(src: &Value) -> bool {
    let bytes = src.to_string().into_bytes();
    let top = Lite::read(&bytes).expect("the frame is JSON");
    GeminiLiveCodec.uplink_audio(&top, &mut |_, _| {}).is_err()
}

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

/// Gemini Live has no in-band session ceiling error frame.
#[test]
fn a_gemini_session_is_told_no_in_band_error() {
    let framed =
        GeminiLiveCodec.ceiling_frame("session_expired", "the session reached its ceiling");
    assert!(framed.is_none(), "{framed:?}");
}

#[test]
fn realtime_input_audio_counts_each_blob_at_its_stated_rate() {
    let payload = b"pretend-uplink-audio-bytes";

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

    assert_eq!(
        up(
            &json!({ "realtimeInput": { "audio": { "mimeType": "audio/pcm;rate=16000", "data": b64(payload) } } })
        ),
        vec![(Some(AudioFormat::Pcm16At16k), payload.len())]
    );
}

#[test]
fn realtime_input_with_both_audio_spellings_is_refused() {
    let src = json!({
        "realtimeInput": {
            "audio": { "mimeType": "audio/pcm;rate=16000", "data": b64(b"ga") },
            "mediaChunks": [ { "mimeType": "audio/pcm;rate=16000", "data": b64(b"legacy") } ]
        }
    });
    assert!(refused(&src));
    let src = json!({
        "realtimeInput": {
            "audio": { "mimeType": "audio/pcm;rate=16000", "data": b64(b"ga") },
            "mediaChunks": [ { "mimeType": "image/jpeg", "data": b64(b"frame") } ]
        }
    });
    assert_eq!(up(&src), vec![(Some(AudioFormat::Pcm16At16k), 2)]);
}

/// An uplink blob whose payload is not base64 counts nothing, never an empty frame; and a frame
#[test]
fn an_undecodable_or_misplaced_gemini_audio_payload_counts_nothing() {
    let blob = json!({ "mimeType": "audio/pcm;rate=16000", "data": b64(b"x") });
    assert!(refused(
        &json!({ "realtimeInput": { "audio": { "mimeType": "audio/pcm;rate=16000", "data": "!!!!" } } })
    ));
    assert!(refused(
        &json!({ "realtimeInput": { "audio": blob }, "setup": {} })
    ));
    assert!(up(&json!({ "realtimeInput": { "audioStreamEnd": true } })).is_empty());
}

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
        json!({ "goAway": { "timeLeft": "10s" } }),
    ] {
        assert!(down(&src).is_empty(), "{src}");
    }
    assert!(GeminiLiveCodec.meter_down(b"not json").is_empty());
}

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

/// Top-level `usageMetadata` riding on a content frame is billed.
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

        assert_eq!(ir.len(), opened + 1, "{ir:?}");
        assert!(matches!(ir.last(), Some(Metered::Usage(_))), "{ir:?}");
    }
}

/// Missing modality breakdowns fall back to the stated usage totals.
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

/// Image and video prompt tokens are billed.
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

/// Tool-use prompt tokens are billed.
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

/// Thought tokens are billed as output.
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

/// Google's total is billed when it exceeds the sum of its parts.
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

    assert_eq!(u.audio_in, 1000);
    assert_eq!(u.cached, 800);

    let billed = crate::session::class_counts(Some(u), crate::session::TurnCounters::default());
    assert!(
        billed.iter().all(|(c, _)| c.as_str() != "cached_tokens"),
        "the cached subset is never a class of its own: {billed:?}"
    );
    assert_eq!(
        billed.iter().map(|(_, n)| n).sum::<u64>(),
        1000,
        "the billed classes sum to the turn's input, never to 1800"
    );
}

#[test]
fn audio_format_from_mime_probe() {
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

/// An unrecognized rate is refused rather than matched by substring.
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

/// The Gemini session receives the operator lock that disables barge-in.
#[test]
fn a_lock_that_turns_barge_in_off_is_told_to_gemini() {
    let vad = |interrupt_response| SessionConfig {
        turn_detection: Some(Some(IrVad::ServerVad {
            threshold: 0.5,
            prefix_padding_ms: 300,
            silence_duration_ms: 500,
            create_response: true,
            interrupt_response,
        })),
        ..SessionConfig::default()
    };
    let off = setup_from_session_config(&vad(false));
    assert_eq!(
        off["setup"]["realtimeInputConfig"]["activityHandling"], "NO_INTERRUPTION",
        "{off}"
    );
    assert_eq!(
        off["setup"]["realtimeInputConfig"]["automaticActivityDetection"]["silenceDurationMs"],
        500
    );
    let on = setup_from_session_config(&vad(true));
    assert!(
        on["setup"]["realtimeInputConfig"]
            .get("activityHandling")
            .is_none(),
        "{on}"
    );
}

#[test]
fn proto_audio_names_are_accepted_at_every_consumed_level() {
    for ri in ["realtimeInput", "realtime_input"] {
        for chunks in ["mediaChunks", "media_chunks"] {
            for mime in ["mimeType", "mime_type"] {
                let mut blob = serde_json::Map::new();
                blob.insert(mime.into(), json!("audio/pcm;rate=16000"));
                blob.insert("data".into(), json!(b64(&[0; 32_000])));
                let mut input = serde_json::Map::new();
                input.insert(chunks.into(), json!([blob]));
                let mut top = serde_json::Map::new();
                top.insert(ri.into(), Value::Object(input));
                assert_eq!(
                    up(&Value::Object(top)),
                    vec![(Some(AudioFormat::Pcm16At16k), 32_000)]
                );
            }
        }
    }
    assert!(refused(&json!({"realtime_input": {
        "audio": {"mime_type":"audio/pcm", "data":b64(b"ga")},
        "media_chunks":[{"mime_type":"audio/pcm", "data":b64(b"legacy")}]
    }})));
}

#[test]
fn proto_audio_duplicates_follow_wire_order_and_malformed_values_refuse() {
    for (frame, expected) in [
        (
            r#"{"realtimeInput":{"audio":{"mimeType":"audio/pcm;rate=24000","mime_type":"audio/pcm;rate=16000","data":"AAAA"}}}"#,
            Some(AudioFormat::Pcm16At16k),
        ),
        (
            r#"{"realtime_input":{"audio":{"mime_type":"audio/pcm;rate=16000","mimeType":"audio/pcm;rate=24000","data":"AAAA"}}}"#,
            Some(AudioFormat::Pcm16),
        ),
        (
            r#"{"client_content":{},"realtime_input":{"audio":{"mime_type":"audio/pcm","data":"AAAA"}}}"#,
            Some(AudioFormat::Pcm16At16k),
        ),
        (
            r#"{"realtime_input":{"audio":{"mime_type":"audio/pcm","data":"AAAA"}},"client_content":{}}"#,
            None,
        ),
    ] {
        let top = Lite::read(frame.as_bytes()).unwrap();
        let mut got = Vec::new();
        assert!(GeminiLiveCodec
            .uplink_audio(&top, &mut |f, n| got.push((f, n)))
            .is_ok());
        assert_eq!(
            got,
            expected.map(|f| vec![(Some(f), 3)]).unwrap_or_default(),
            "{frame}"
        );
    }
    for frame in [
        r#"{"realtime_input":"bad"}"#,
        r#"{"realtime_input":{"media_chunks":{}}}"#,
        r#"{"realtime_input":{"audio":{"mime_type":false,"data":"AAAA"}}}"#,
        r#"{"realtime_input":{"audio":{"mime_type":false,"data":"AAAA"},"audio":{"mimeType":"audio/pcm","data":"AAAA"}}}"#,
        r#"{"realtime_input":{"media_chunks":false,"mediaChunks":[]}}"#,
        r#"{"realtime_input":{"audio":{"mimeType":"audio/pcm","data":"!!!!","data":"AAAA"}}}"#,
        r#"{"realtime_input":{"audio":{"mime_type":false,"mimeType":"audio/pcm","data":"AAAA"}}}"#,
        r#"{"realtime_input":"bad","realtimeInput":{"audio":{"mimeType":"audio/pcm","data":"AAAA"}}}"#,
        r#"{"realtime_input":{"audio":{"mimeType":"audio/pcm","mime_type":null,"data":"AAAA"}}}"#,
        r#"{"realtime_input":null,"realtimeInput":{"audio":{"mimeType":"audio/pcm","data":"AAAA"}}}"#,
    ] {
        let top = Lite::read(frame.as_bytes()).unwrap();
        assert!(
            GeminiLiveCodec.uplink_audio(&top, &mut |_, _| {}).is_err(),
            "{frame}"
        );
    }
}

#[test]
fn proto_usage_and_tool_aliases_keep_all_billable_classes() {
    let src = json!({"tool_call":{"function_calls":[{"id":"one"},{"id":"two"}]},
    "usage_metadata":{
        "prompt_token_count":10, "response_token_count":7,
        "tool_use_prompt_token_count":3, "thoughts_token_count":2,
        "cached_content_token_count":4, "total_token_count":23,
        "prompt_tokens_details":[{"modality":"AUDIO","token_count":5},{"modality":"IMAGE","token_count":2},{"modality":"VIDEO","token_count":1}],
        "tool_use_prompt_tokens_details":[{"modality":"TEXT","token_count":3}],
        "response_tokens_details":[{"modality":"AUDIO","token_count":6}]
    }});
    let got = down(&src);
    assert_eq!(got.len(), 3);
    assert!(matches!(got[0], Metered::ToolOpen));
    assert!(matches!(got[1], Metered::ToolOpen));
    let Metered::Usage(u) = got[2] else {
        panic!("usage: {got:?}");
    };
    assert_eq!(
        (
            u.audio_in,
            u.text_in,
            u.image_in,
            u.video_in,
            u.audio_out,
            u.text_out,
            u.cached
        ),
        (5, 6, 2, 1, 6, 3, 4)
    );
}

#[test]
fn proto_usage_duplicate_aliases_are_ordered_and_invalid_selected_counts_floor() {
    for (frame, want) in [
        (
            r#"{"usageMetadata":{"promptTokenCount":2,"prompt_token_count":5}}"#,
            5,
        ),
        (
            r#"{"usage_metadata":{"prompt_token_count":5,"promptTokenCount":2}}"#,
            2,
        ),
        (
            r#"{"usageMetadata":{"promptTokenCount":2},"usage_metadata":{"prompt_token_count":7}}"#,
            7,
        ),
    ] {
        let got = GeminiLiveCodec.meter_down(frame.as_bytes());
        assert!(
            matches!(got.as_slice(), [Metered::Usage(u)] if u.text_in == want && !u.floor),
            "{got:?}"
        );
    }
    for frame in [
        r#"{"usage_metadata":{"prompt_token_count":false}}"#,
        r#"{"usage_metadata":{"prompt_tokens_details":{}}}"#,
        r#"{"usage_metadata":{"prompt_tokens_details":[{"token_count":false}]}}"#,
        r#"{"usageMetadata":null,"usage_metadata":{"prompt_token_count":2}}"#,
    ] {
        let got = GeminiLiveCodec.meter_down(frame.as_bytes());
        assert!(
            matches!(got.as_slice(), [Metered::Usage(u)] if u.floor),
            "{got:?}"
        );
    }
}

#[test]
fn downlink_distinct_oneof_members_floor_but_aliases_keep_wire_order() {
    for frame in [
        r#"{"server_content":{},"tool_call":{"function_calls":[{}]}}"#,
        r#"{"toolCall":{"functionCalls":[{}]},"server_content":{}}"#,
    ] {
        assert!(
            matches!(GeminiLiveCodec.meter_down(frame.as_bytes()).as_slice(), [Metered::Usage(u)] if u.floor)
        );
    }
    assert_eq!(
        GeminiLiveCodec.meter_down(
            br#"{"tool_call":{"function_calls":[{},{}]},"toolCall":{"functionCalls":[{}]}}"#
        ),
        vec![Metered::ToolOpen]
    );
}

#[test]
fn audit_mime_and_client_content() {
    for mime in [
        "audio/pcmu",
        "audio/pcma",
        "audio/pcm;rate=8000;rate=24000",
        "audio/pcm;rate=16000;rate=8000",
    ] {
        assert_eq!(audio_format_from_mime(mime), None);
    }
    assert_eq!(
        audio_format_from_mime("audio/pcm;rate=\"16000\""),
        Some(AudioFormat::Pcm16At16k)
    );
    assert_eq!(
        up(
            &json!({"clientContent":{"turns":[{"parts":[{"inlineData":{"mimeType":"audio/pcm;rate=16000","data":"AQID"}}]}]}})
        ),
        vec![(Some(AudioFormat::Pcm16At16k), 3)]
    );
    let cfg = SessionConfig {
        model: Some("gemini-live".into()),
        context_window_tokens: Some(32768),
        max_output_tokens: Some(MaxOutputTokens::Limit(4096)),
        ..Default::default()
    };
    let setup = setup_from_session_config(&cfg);
    assert_eq!(setup["setup"]["model"], "models/gemini-live");
    assert_eq!(
        setup["setup"]["contextWindowCompression"]["triggerTokens"],
        28672
    );
    assert!(
        matches!(down(&json!({"toolCall":{"functionCalls":[]},"setupComplete":{}})).as_slice(),[Metered::Usage(u)] if u.floor)
    );
}

#[test]
fn audit_billing_parse_skips_payloads_and_bounds_nodes() {
    let frame = br#"{"serverContent":{"modelTurn":{"parts":[{"inlineData":{"data":"secret audio"}}]}},"usageMetadata":{"promptTokenCount":3}}"#;
    let value = meter_value(frame).unwrap();
    assert!(value.get("serverContent").is_none());
    assert_eq!(value["usageMetadata"]["promptTokenCount"], 3);
    let frame = format!(
        "{{\"toolCall\":{{\"functionCalls\":[{}]}}}}",
        vec!["0"; super::super::peek::MAX_LITE_NODES].join(",")
    );
    assert!(
        matches!(GeminiLiveCodec.meter_down(frame.as_bytes()).as_slice(), [Metered::Usage(u)] if u.floor)
    );
}
