// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Tests for `crates/busbar-plane-streaming/src/session_unit.rs`.

use super::*;
use crate::codec::ir::codec::OpenAiRealtimeCodec;
use crate::codec::ir::media::AudioFormat;

fn json(v: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&v).expect("json")
}

fn text(frames: &[Vec<u8>]) -> String {
    frames
        .iter()
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

fn start(encoding: &str) -> Vec<u8> {
    json(
        serde_json::json!({"event":"start","start":{"streamSid":"MZ1","callSid":"CA1",
        "mediaFormat":{"encoding":encoding,"sampleRate":8000,"channels":1}}}),
    )
}

fn media(sid: &str, bytes: &[u8]) -> Vec<u8> {
    json(serde_json::json!({"event":"media","streamSid":sid,
        "media":{"payload": busbar_contract::media::base64_encode(bytes)}}))
}

fn telephony() -> SessionUnit<OpenAiRealtimeCodec> {
    let g711 = SessionConfig {
        input_audio_format: Some(AudioFormat::G711Ulaw),
        output_audio_format: Some(AudioFormat::G711Ulaw),
        ..SessionConfig::default()
    };
    SessionUnit::open(OpenAiRealtimeCodec, g711, true)
}

#[test]
fn the_callers_audio_on_the_telephony_door_reaches_the_far_end() {
    // The served telephony leg handed Twilio's `event`-tagged frames to the realtime reader, which
    // reads only `type`-tagged ones: the caller's audio reached no one.
    let mut s = telephony();
    let started = s.from_caller(&start("audio/x-mulaw"));
    assert!(started.to_caller.is_empty() && !started.end, "{started:?}");
    assert_eq!(started.to_far_end.len(), 1, "the locked session, once");
    assert!(text(&started.to_far_end).contains("audio/pcmu"));
    let plan = s.from_caller(&media("MZ1", &[0x7f; 160]));
    let up = text(&plan.to_far_end);
    assert!(up.contains("input_audio_buffer.append"), "{up}");
    assert!(
        up.contains(&busbar_contract::media::base64_encode(&[0x7f; 160])),
        "the caller's audio passes through unchanged: {up}"
    );
}

#[test]
fn media_for_another_stream_and_unmodelled_events_reach_no_one() {
    let mut s = telephony();
    let _ = s.from_caller(&start("audio/x-mulaw"));
    assert_eq!(s.from_caller(&media("MZ-forged", &[1; 8])), Plan::default());
    assert_eq!(
        s.from_caller(&json(serde_json::json!({"event":"something-new"}))),
        Plan::default(),
        "an event this reader does not model is dropped, not a session end"
    );
    assert!(!s.ended());
}

#[test]
fn a_call_in_another_media_format_ends_the_session() {
    let mut s = telephony();
    assert!(s.from_caller(&start("audio/l16")).end);
    assert!(s.ended());
}

#[test]
fn model_audio_and_a_barge_in_reach_the_caller_in_the_carriers_envelope() {
    let mut s = telephony();
    let _ = s.from_caller(&start("audio/x-mulaw"));
    let delta = json(serde_json::json!({"type":"response.output_audio.delta",
        "delta": busbar_contract::media::base64_encode(&[9; 16]),
        "item_id":"it1","output_index":0,"content_index":0}));
    let plan = s.from_far_end(&delta);
    let down = text(&plan.to_caller);
    assert!(
        down.contains("\"event\":\"media\"") && down.contains("MZ1"),
        "{down}"
    );
    let barge = json(
        serde_json::json!({"type":"input_audio_buffer.speech_started",
        "audio_start_ms":0,"item_id":"it1"}),
    );
    let plan = s.from_far_end(&barge);
    assert!(text(&plan.to_caller).contains("\"event\":\"clear\""));
    assert!(
        plan.to_far_end.is_empty(),
        "busbar authors nothing upstream on a barge-in (Law 11)"
    );
}

/// RED-BEFORE-GREEN (Law 11: "a tool call on any session, Twilio included, is relayed down that
/// session as-is"): the model's tool call on a telephony session goes down the session byte for
/// byte (the item that opens it, its streamed arguments, the item when done), and busbar answers
/// nothing upstream; a frame the carrier has no event for and that is no tool call still reaches no
/// one. RED: the envelope kept only audio and barge-ins, so the call was dropped and the session
/// stalled with no one to answer it.
#[test]
fn a_tool_call_on_a_telephony_session_is_relayed_down_the_session_as_is() {
    let mut s = telephony();
    let _ = s.from_caller(&start("audio/x-mulaw"));
    let frames = [
        r#"{"type":"response.output_item.added","event_id":"e1","response_id":"r1","output_index":0,"item":{"id":"fc1","type":"function_call","call_id":"call_1","name":"lookup","arguments":""}}"#,
        r#"{"type":"response.function_call_arguments.delta","event_id":"e2","item_id":"fc1","call_id":"call_1","delta":"{\"q\":"}"#,
        r#"{"type":"response.function_call_arguments.done","event_id":"e3","item_id":"fc1","call_id":"call_1","name":"lookup","arguments":"{\"q\":1}"}"#,
        r#"{"type":"response.output_item.done","event_id":"e4","response_id":"r1","output_index":0,"item":{"id":"fc1","type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"q\":1}"}}"#,
    ];
    for frame in frames {
        let plan = s.from_far_end(frame.as_bytes());
        assert_eq!(
            plan.to_caller,
            vec![frame.as_bytes().to_vec()],
            "relayed as the far end wrote it"
        );
        assert!(
            plan.to_far_end.is_empty(),
            "busbar answers nothing (Law 11)"
        );
    }
    let message =
        r#"{"type":"response.output_item.added","item":{"type":"message","role":"assistant"}}"#;
    assert!(s.from_far_end(message.as_bytes()).to_caller.is_empty());
}

#[test]
fn a_stop_settles_the_open_turns_counters_once() {
    let mut s = telephony();
    let _ = s.from_caller(&start("audio/x-mulaw"));
    // Two seconds of 8 kHz mu-law: 8 bytes a millisecond.
    let _ = s.from_caller(&media("MZ1", &[0; 16_000]));
    let seconds = CLASSES
        .iter()
        .position(|c| *c == meta::CLASS_AUDIO_SECONDS_IN)
        .expect("declared");
    assert_eq!(s.units().0[seconds], 2, "the open turn, running");
    assert!(
        s.from_caller(&json(serde_json::json!({"event":"stop"})))
            .end
    );
    assert_eq!(s.units().0[seconds], 2, "settled once, not twice");
    let _ = s.end();
    assert_eq!(s.units().0[seconds], 2, "a second end settles nothing");
}

#[test]
fn usage_reports_accumulate_across_turns() {
    let mut s = SessionUnit::open(OpenAiRealtimeCodec, SessionConfig::default(), false);
    let done = json(
        serde_json::json!({"type":"response.done","response":{"usage":{
        "input_token_details":{"audio_tokens":10,"text_tokens":3,"cached_tokens":2},
        "output_token_details":{"audio_tokens":20,"text_tokens":4}}}}),
    );
    let _ = s.from_far_end(&done);
    let _ = s.from_far_end(&done);
    assert_eq!(
        s.units().0,
        [20, 40, 6, 8, 0, 0, 0, 0],
        "cached tokens are attribution, never billed"
    );
}

#[test]
fn a_session_past_its_ceiling_is_told_why_and_ends() {
    let mut s = SessionUnit::open(OpenAiRealtimeCodec, SessionConfig::default(), false);
    let plan = s.close_at_ceiling(2);
    assert!(plan.end);
    assert!(text(&plan.to_caller).contains(crate::session_pump::SESSION_CEILING_REASON));
    assert!(s.ended());
    assert_eq!(
        s.close_at_ceiling(2),
        Plan::default(),
        "an ended session is told once"
    );
}

#[test]
fn admitted_milliseconds_meter_as_whole_seconds_rounded_up() {
    assert_eq!(meta::audio_seconds_in(0), 0);
    assert_eq!(meta::audio_seconds_in(1), 1);
    assert_eq!(meta::audio_seconds_in(1_500), 2);
    assert_eq!(meta::audio_seconds_in(3_000), 3);
}

#[test]
fn a_stop_settles_the_open_turns_tool_calls_too() {
    let mut s = telephony();
    let _ = s.from_caller(&start("audio/x-mulaw"));
    let _ = s.from_caller(&media("MZ1", &[0; 8_000]));
    let open = json(serde_json::json!({"type":"response.output_item.added",
        "item":{"type":"function_call","call_id":"c1","name":"lookup"}}));
    let _ = s.from_far_end(&open);
    assert!(
        s.from_caller(&json(serde_json::json!({"event":"stop"})))
            .end
    );
    assert_eq!(s.units().0, [0, 0, 0, 0, 1, 1, 0, 0]);
}

#[test]
fn one_full_turn_reports_the_six_classes_the_served_plane_metered() {
    let mut s = SessionUnit::open(OpenAiRealtimeCodec, SessionConfig::default(), false);
    let pcm = busbar_contract::media::base64_encode(&vec![0u8; 48_000]);
    let _ = s.from_caller(&json(
        serde_json::json!({"type":"input_audio_buffer.append","audio":pcm}),
    ));
    let open = json(serde_json::json!({"type":"response.output_item.added",
        "item":{"type":"function_call","call_id":"c1","name":"lookup"}}));
    let _ = s.from_far_end(&open);
    let done = json(
        serde_json::json!({"type":"response.done","response":{"usage":{
        "input_token_details":{"audio_tokens":10,"text_tokens":3,"cached_tokens":1},
        "output_token_details":{"audio_tokens":20,"text_tokens":4}}}}),
    );
    let _ = s.from_far_end(&done);
    assert_eq!(s.units().0, [10, 20, 3, 4, 1, 1, 0, 0]);
}

#[test]
fn a_start_with_no_stream_id_refuses_the_call() {
    let mut s = telephony();
    let empty = json(
        serde_json::json!({"event":"start","start":{"streamSid":"","callSid":"CA1",
        "mediaFormat":{"encoding":"audio/x-mulaw","sampleRate":8000,"channels":1}}}),
    );
    assert!(s.from_caller(&empty).end);
    assert!(s.ended());
    assert_eq!(
        s.from_caller(&media("", &[1; 80])),
        Plan::default(),
        "no binding was opened for an empty stream id"
    );
}

#[test]
fn a_media_payload_resuming_after_padding_is_refused_and_never_billed() {
    for payload in ["QQ==QQ==".to_string(), "QQ==".repeat(2_000)] {
        let mut s = telephony();
        let _ = s.from_caller(&start("audio/x-mulaw"));
        let frame = json(serde_json::json!({"event":"media","streamSid":"MZ1",
            "media":{"payload": payload}}));
        let plan = s.from_caller(&frame);
        assert!(plan.end && plan.to_far_end.is_empty());
        assert_eq!(s.units(), CumulativeUnits::default(), "nothing is billed");
    }
}

#[test]
fn every_emitted_frame_is_a_text_message_and_either_opcode_is_read() {
    // Gemini Live's server frames arrive binary: the bytes are read as they are.
    let mut s = SessionUnit::open(
        crate::codec::ir::codec::gemini::GeminiLiveCodec,
        SessionConfig::default(),
        false,
    );
    let setup = json(serde_json::json!({"setupComplete": {}}));
    let plan = s.from_far_end(&setup);
    // The setupComplete is relayed to the caller, once, exactly as it came, and is text (audit
    // S85: the loop below held for a session that emitted nothing).
    assert_eq!(plan.to_caller, vec![setup.clone()], "relayed as it came");
    assert!(plan.to_far_end.is_empty());
    for frame in plan.to_caller.iter().chain(plan.to_far_end.iter()) {
        assert!(
            std::str::from_utf8(frame).is_ok(),
            "an emitted frame is text"
        );
    }
}

/// RED-BEFORE-GREEN (MONEY, #42 and the owner ruling of 2026-10-27: a present-but-unreadable
/// billed count is never a zero, and spend is always attributed): a far-end usage report the reader
/// cannot read bills the plane's floor for the turn (its output tokens) and the session goes on; the
/// report reaches the caller as sent, and the door is told once so the kernel counts the reader's
/// fault. RED before the ruling: the session ended unmetered.
#[test]
fn an_unreadable_usage_report_bills_the_floor_and_is_reported_once() {
    let mut s = SessionUnit::open(OpenAiRealtimeCodec, SessionConfig::default(), false);
    let report = serde_json::json!({"type":"response.done","response":{"usage":{
        "input_tokens":"1500","output_tokens":9}}});
    let floor = (report.to_string().len() as u64 / 4).max(1);
    let done = json(report);
    let plan = s.from_far_end(&done);
    assert!(!plan.end, "the session goes on: {plan:?}");
    let down = text(&plan.to_caller);
    assert!(
        down.contains("response.done"),
        "the report reaches the caller: {down}"
    );
    assert!(!s.ended());
    assert!(s.floored(), "the turn billed the floor");
    assert!(
        s.units()
            .reported()
            .contains(&(crate::session_unit::TEXT_TOKENS_OUT, floor)),
        "{:?}",
        s.units()
    );
    assert!(s.take_unreadable(), "the door is told");
    assert!(!s.take_unreadable(), "once");
}
