// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! CODEC UNIT TESTS — what the meter reads off the OpenAI Realtime GA dialect: the usage a
//! `response.done` reports, the errors and opened tool calls that close or count in a turn, and the
//! uplink audio a caller sends.

use super::*;
use serde_json::json;

// ── helpers ──────────────────────────────────────────────────────────────────────────────────────

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

// ── media: the uplink audio counted ──────────────────────────────────────────────────────────────

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
/// never a zero-length frame indistinguishable from a caller saying nothing.
#[test]
fn an_undecodable_audio_payload_counts_nothing() {
    assert!(up(&json!({ "type": "input_audio_buffer.append", "audio": "!!!!" })).is_empty());
    assert_eq!(
        up(&json!({ "type": "input_audio_buffer.append", "audio": b64(&[1, 2, 3, 4]) })),
        vec![(None, 4)],
        "well-formed audio still counts"
    );
}

// ── tools and errors ─────────────────────────────────────────────────────────────────────────────

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

// ── degrade, don't error ─────────────────────────────────────────────────────────────────────────

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

// ── usage extraction (`BUSBAR-1.6.0.md` #18/#45) ──────────────────────────────────────────────────────────────────────

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

/// RED-BEFORE-GREEN (MONEY; the Realtime API reference: `usage` "will correspond to billing", and
/// `input_token_details.image_tokens` is "The number of image tokens used as input for the
/// Response"): a turn whose input carried an image bills its image tokens under their own class,
/// and the input classes sum to the stated `input_tokens`. RED: only the audio and text entries
/// were read, so the 10 image tokens billed zero.
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

/// RED-BEFORE-GREEN (MONEY): the image tokens bill even when the stated total is absent: each side
/// bills the sum of every entry of its breakdown. RED: the image entry was never read.
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
    // OpenAI Realtime reports `input_token_details.cached_tokens` as a SUBSET of the input classes it
    // sits beside (`audio_tokens` + `text_tokens`), exactly as Chat Completions reports
    // `prompt_tokens_details.cached_tokens` inside `prompt_tokens`. So the billing fold must bill the
    // UNCACHED remainder as `input` and the cached portion as `cache_read` — never the full input
    // figure alongside the cache figure, which would charge the cached tokens on both lanes.
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
    // The IR carrier stays wire-faithful: extraction only, the raw provider figures.
    assert_eq!(u.audio_in, 1000);
    assert_eq!(u.cached, 800);
    // The BILLING fold is where the subset is netted out.
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

// ── usage: the modality breakdown is a REFINEMENT, not the only source ──────────────────────────
//
// The two tests below are anchored to `testing/voice-conformance/fixtures/openai/response.done.json`
// — the committed capture of a real OpenAI Realtime `response.done` — rather than to a literal typed
// here. The defect was "the reader ignores the stated totals", and a hand-written fixture cannot
// distinguish a reader that consults them from one that does not: it would be testing the test. The
// capture supplies the shape; each test states exactly what it varies and why.

/// Load the committed OpenAI Realtime `response.done` capture.
///
/// It lives in this repo's voice-conformance fixture tree (moved here from busbar with the harness),
/// where it was recorded and where the conformance battery judges against it; a missing file is a
/// failure, never a skip.
fn captured_response_done() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testing/voice-conformance/fixtures/openai/response.done.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the captured response.done is missing at {path:?} ({e})"));
    serde_json::from_str(&text).expect("the capture is JSON")
}

/// A turn that states only its TOTALS is billed those totals, not ZERO.
///
/// `extract_usage` read only the per-modality breakdown (`input_token_details`/`output_token_details`)
/// and defaulted a missing one to `0`. The breakdown is a REFINEMENT of the stated
/// `input_tokens`/`output_tokens`, never their sole source — OpenAI Realtime can state the totals and
/// omit the split — so a real, answered turn METERED AT ZERO. That is silent under-billing: no error,
/// no warning, a ledger row faithfully reporting that nothing happened.
///
/// The Gemini Live dialect in this same crate already had this fallback (`usage_from_metadata`).
/// Only the OpenAI twin was missing it, so the identical session metered correctly on one dialect and
/// at zero on the other — which is why this is an oversight, not a decision.
///
/// VARIED FROM THE CAPTURE: the two `*_token_details` members are REMOVED, and nothing else. That is
/// precisely the wire case the capture cannot show (it is a breakdown-present recording) and the one
/// the defect lived in. The stated totals are the capture's own.
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

    // The lane the billing fold sums onto is what actually decides the invoice.
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
///
/// Driven by the capture EXACTLY AS RECORDED, no variation at all: this is the ordinary wire case,
/// and the fallback must be invisible on it.
#[test]
fn a_present_breakdown_is_not_overridden_by_the_stated_totals() {
    let src = captured_response_done();
    let u = &usage_of(&src);
    assert_eq!(u.audio_in, 100, "the captured audio-in slice: {u:?}");
    assert_eq!(u.text_in, 40, "the captured text-in slice: {u:?}");
    assert_eq!(u.audio_out, 60, "the captured audio-out slice: {u:?}");
    assert_eq!(u.text_out, 20, "the captured text-out slice: {u:?}");
}

/// RED (audit plane-streaming round 52b0f93ac9, S2): a far end stating counts near `u64::MAX` saturates
/// the input total, which then sits below the audio and image breakdown it must cover. The text
/// slice is what is left of the input, never a wrapped subtraction: zero, as the Gemini twin
/// (`usage_from_metadata`) answers the same report.
#[test]
fn a_saturated_input_total_leaves_no_text_slice_and_never_wraps() {
    let u = usage_of(&json!({"type":"response.done","response":{"usage":{
        "input_tokens": u64::MAX,
        "input_token_details":{"audio_tokens": u64::MAX - 2, "image_tokens": 5}}}}));
    assert_eq!(u.text_in, 0, "{u:?}");
    assert_eq!(u.audio_in, u64::MAX - 2, "{u:?}");
    assert_eq!(u.image_in, 5, "{u:?}");
}
