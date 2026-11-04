// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! CODEC UNIT TESTS — what the meter reads off the OpenAI Realtime GA dialect: the usage a

use super::*;
use serde_json::json;

fn b64(bytes: &[u8]) -> String {
    busbar_contract::media::base64_encode(bytes)
}

fn down(src: &Value) -> Vec<Metered> {
    OpenAiRealtimeCodec.meter_down(src.to_string().as_bytes())
}

/// The one usage report a frame carries.
fn usage_of(src: &Value) -> IrDuplexUsage {
    match down(src).as_slice() {
        [Metered::Usage(u)] => *u,
        other => panic!("expected one usage report, got {other:?}"),
    }
}

/// The audio the meter counts in a caller frame: each blob's stated format and decoded length.
fn up(src: &Value) -> Vec<(Option<AudioFormat>, usize)> {
    let bytes = src.to_string().into_bytes();
    let top = peek::Lite::read(&bytes).expect("the frame is JSON");
    let mut counted = Vec::new();
    OpenAiRealtimeCodec
        .uplink_audio(&top, &mut |format, len| counted.push((format, len)))
        .expect("the OpenAI dialect refuses no uplink audio");
    counted
}

#[test]
fn audio_format_math() {
    assert_eq!(AudioFormat::Pcm16.bytes_per_ms(), 48);
    assert_eq!(AudioFormat::G711Ulaw.bytes_per_ms(), 8);
}

#[test]
fn uplink_audio_append_counts_its_decoded_bytes() {
    let payload = b"pretend-uplink-audio-bytes";
    let src = json!({ "type": "input_audio_buffer.append", "audio": b64(payload) });
    assert_eq!(
        up(&src),
        vec![(None, payload.len())],
        "the decoded length, no format stated"
    );
}

/// AUDIO THAT DOES NOT DECODE IS NOT SILENCE: an uplink payload that is not base64 counts nothing,
#[test]
fn an_undecodable_audio_payload_counts_nothing() {
    let frame = br#"{"type":"input_audio_buffer.append","audio":"!!!!"}"#;
    assert!(OpenAiRealtimeCodec
        .uplink_audio(&peek::Lite::read(frame).unwrap(), &mut |_, _| {})
        .is_err());
    assert_eq!(
        up(&json!({ "type": "input_audio_buffer.append", "audio": b64(&[1, 2, 3, 4]) })),
        vec![(None, 4)],
        "well-formed audio still counts"
    );
}

#[test]
fn an_added_function_call_item_opens_a_tool_call_and_no_other_item_does() {
    let added = |item: Value| down(&json!({ "type": "response.output_item.added", "item": item }));
    assert_eq!(
        added(json!({ "type": "function_call", "call_id": "call_1", "name": "get_weather" })),
        vec![Metered::ToolOpen]
    );
    assert!(added(json!({ "type": "message", "role": "assistant" })).is_empty());
    assert!(
        down(&json!({ "type": "response.function_call_arguments.done", "call_id": "call_1" }))
            .is_empty()
    );
}

#[test]
fn error_event_closes_the_turn() {
    assert_eq!(
        down(&json!({ "type": "error", "error": { "code": "x", "message": "boom" } })),
        vec![Metered::Error]
    );
}

#[test]
fn malformed_or_unmetered_frames_meter_nothing() {
    assert!(OpenAiRealtimeCodec.meter_down(b"{ broken").is_empty());
    for src in [
        json!({ "type": "some.server.only.thing" }),
        json!({ "type": "response.output_audio.delta", "delta": b64(&[1, 2, 3]) }),
        json!({ "type": "input_audio_buffer.speech_started", "audio_start_ms": 120 }),
        json!({ "type": "session.created", "session": { "id": "sess_1" } }),
    ] {
        assert!(down(&src).is_empty(), "{src}");
    }
    assert!(up(&json!({ "type": "never.heard.of.it", "audio": b64(&[1]) })).is_empty());
}

#[test]
fn response_done_usage_extracts_split_token_classes() {
    let src = json!({
        "type": "response.done",
        "response": {
            "usage": {
                "total_tokens": 145,
                "input_tokens": 95,
                "output_tokens": 50,
                "input_token_details": { "audio_tokens": 80, "text_tokens": 15, "cached_tokens": 5 },
                "output_token_details": { "audio_tokens": 40, "text_tokens": 10 }
            }
        }
    });
    let u = &usage_of(&src);
    assert_eq!(u.audio_in, 80);
    assert_eq!(u.text_in, 15);
    assert_eq!(u.cached, 5);
    assert_eq!(u.audio_out, 40);
    assert_eq!(u.text_out, 10);
}

/// Realtime image input tokens are billed.
#[test]
fn image_input_tokens_are_billed() {
    let src = json!({
        "type": "response.done",
        "response": {
            "usage": {
                "total_tokens": 230,
                "input_tokens": 150,
                "output_tokens": 80,
                "input_token_details": { "text_tokens": 40, "audio_tokens": 100, "image_tokens": 10 },
                "output_token_details": { "text_tokens": 20, "audio_tokens": 60 }
            }
        }
    });
    let u = &usage_of(&src);
    assert_eq!((u.audio_in, u.text_in, u.image_in), (100, 40, 10), "{u:?}");
    let billed = crate::session::class_counts(Some(u), crate::session::TurnCounters::default());
    assert!(
        billed.contains(&(crate::meta::CLASS_IMAGE_TOKENS_IN, 10)),
        "the image tokens bill as their own class: {billed:?}"
    );
    assert_eq!((u.audio_out, u.text_out), (60, 20), "{u:?}");
}

/// Image input tokens are billed even when the stated total is absent.
#[test]
fn image_input_tokens_are_billed_without_a_stated_total() {
    let src = json!({
        "type": "response.done",
        "response": { "usage": {
            "input_token_details": { "text_tokens": 4, "audio_tokens": 0, "image_tokens": 300 }
        } }
    });
    let u = &usage_of(&src);
    assert_eq!((u.audio_in, u.text_in, u.image_in), (0, 4, 300), "{u:?}");
}

#[test]
fn cached_input_tokens_are_not_billed_twice() {
    let src = json!({
        "type": "response.done",
        "response": {
            "usage": {
                "total_tokens": 1000,
                "input_tokens": 1000,
                "output_tokens": 0,
                "input_token_details": { "audio_tokens": 1000, "text_tokens": 0, "cached_tokens": 800 },
                "output_token_details": { "audio_tokens": 0, "text_tokens": 0 }
            }
        }
    });
    let u = &usage_of(&src);

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

/// Load the committed OpenAI Realtime `response.done` fixture (hand-written, not a capture).
fn captured_response_done() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testing/voice-conformance/fixtures/openai/response.done.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the captured response.done is missing at {path:?} ({e})"));
    serde_json::from_str(&text).expect("the capture is JSON")
}

/// A turn that states only its TOTALS is billed those totals, not ZERO.
#[test]
fn usage_falls_back_to_stated_totals_when_the_modality_breakdown_is_absent() {
    let mut src = captured_response_done();
    let usage = src
        .pointer_mut("/response/usage")
        .and_then(Value::as_object_mut)
        .expect("the capture carries a usage object");
    let stated_in = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .expect("the capture states an input total");
    let stated_out = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .expect("the capture states an output total");
    assert!(
        usage.remove("input_token_details").is_some()
            && usage.remove("output_token_details").is_some(),
        "the capture must HAVE the breakdown for its removal to be the thing under test"
    );

    let u = &usage_of(&src);
    assert_eq!(
        u.text_in, stated_in,
        "the stated input total is billed, not zero: {u:?}"
    );
    assert_eq!(
        u.text_out, stated_out,
        "the stated output total is billed, not zero: {u:?}"
    );
    assert_eq!(
        u.audio_in, 0,
        "the split is unknown, so nothing claims audio"
    );
    assert_eq!(
        u.audio_out, 0,
        "the split is unknown, so nothing claims audio"
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
        stated_in,
        "the input classes bill the stated total, never zero"
    );
    assert_eq!(
        sum_of(&["audio_tokens_out", "text_tokens_out"]),
        stated_out,
        "the output classes bill the stated total, never zero"
    );
}

/// When the breakdown IS present it WINS — the stated totals are a fallback, never an override.
#[test]
fn a_present_breakdown_is_not_overridden_by_the_stated_totals() {
    let src = captured_response_done();
    let u = &usage_of(&src);
    assert_eq!(u.audio_in, 100, "the captured audio-in slice: {u:?}");
    assert_eq!(u.text_in, 40, "the captured text-in slice: {u:?}");
    assert_eq!(u.audio_out, 60, "the captured audio-out slice: {u:?}");
    assert_eq!(u.text_out, 20, "the captured text-out slice: {u:?}");
}

/// Saturated input totals leave no text slice and never wrap.
#[test]
fn a_saturated_input_total_leaves_no_text_slice_and_never_wraps() {
    let u = usage_of(&json!({"type":"response.done","response":{"usage":{
        "input_tokens": u64::MAX,
        "input_token_details":{"audio_tokens": u64::MAX - 2, "image_tokens": 5}}}}));
    assert_eq!(u.text_in, 0, "{u:?}");
    assert_eq!(u.audio_in, u64::MAX - 2, "{u:?}");
    assert_eq!(u.image_in, 5, "{u:?}");
}

#[test]
fn audit_openai_audio_paths_and_duplicates() {
    assert_eq!(
        up(
            &json!({"type":"response.create", "response":{"input":[{"content":[{"type":"input_audio","audio":"AQID"}]}]}})
        ),
        vec![(None, 3)]
    );
    for frame in [
        r#"{"type":"input_audio_buffer.append","audio":"AQID","audio":""}"#,
        r#"{"type":"input_audio_buffer.append","audio":5}"#,
        r#"{"type":"input_audio_buffer.append","type":"ignored","audio":"AQID"}"#,
        r#"{"type":"conversation.item.create","item":{"content":[{"type":"input_audio","audio":"!"}]}}"#,
    ] {
        assert!(
            OpenAiRealtimeCodec
                .uplink_audio(&peek::Lite::read(frame.as_bytes()).unwrap(), &mut |_, _| {})
                .is_err(),
            "{frame}"
        );
    }
}

#[test]
fn audit_openai_owned_meter_parse_enforces_node_budget() {
    let frame = serde_json::to_vec(&serde_json::json!({"type":"response.done","response":{"usage":{"input_tokens":1},"output":vec![0; super::peek::MAX_LITE_NODES]}})).unwrap();
    let events = OpenAiRealtimeCodec.meter_down(&frame);
    assert!(matches!(events.as_slice(), [Metered::Usage(usage)] if usage.floor));
}
