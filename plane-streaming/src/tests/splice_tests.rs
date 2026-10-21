// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Tests for `crates/busbar-plane-streaming/src/splice.rs`: only the governed object is rebuilt,
//! the caller's members keep their bytes, keys are read as the far end reads them.

use super::*;

fn rule(set: serde_json::Value) -> Rule {
    Rule {
        at: "session",
        cut: &["instructions", "tools"],
        set: match set {
            Value::Object(m) => m,
            _ => Map::new(),
        },
        proto_names: false,
        cap: None,
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn the_bytes_around_the_governed_object_are_copied_as_they_arrived() {
    let frame = br#"{ "type" : "session.update",  "session":{"voice":"verse" ,"instructions":"x"}, "event_id":"e"}"#;
    let out = splice(frame, &rule(serde_json::json!({"instructions":"locked"}))).expect("spliced");
    assert_eq!(
        text(&out),
        r#"{ "type" : "session.update",  "session":{"voice":"verse","instructions":"locked"}, "event_id":"e"}"#
    );
}

#[test]
fn a_frame_without_the_governed_object_is_unchanged() {
    let frame = br#"{"type":"response.create"}"#;
    let out = splice(frame, &rule(serde_json::json!({}))).expect("spliced");
    assert_eq!(out, frame.to_vec());
}

#[test]
fn an_escaped_or_repeated_key_is_cut_as_the_far_end_reads_it() {
    let frame = br#"{"session":{"instructions":"a","instructions":"b","tools":[1],"x":{"y":[2,{"z":"}"}]}}}"#;
    let out = splice(frame, &rule(serde_json::json!({}))).expect("spliced");
    assert_eq!(text(&out), r#"{"session":{"x":{"y":[2,{"z":"}"}]}}}"#);
}

#[test]
fn a_locked_object_is_merged_into_the_callers_member_by_member() {
    let frame =
        br#"{"session":{"turn_detection":{"type":"server_vad","threshold":0.9,"extra":true}}}"#;
    let out = splice(
        frame,
        &rule(serde_json::json!({"turn_detection":{"threshold":0.5}})),
    )
    .expect("spliced");
    let v: Value = serde_json::from_slice(&out).expect("json");
    assert_eq!(v["session"]["turn_detection"]["type"], "server_vad");
    assert_eq!(v["session"]["turn_detection"]["extra"], true);
    assert_eq!(v["session"]["turn_detection"]["threshold"], 0.5);
}

#[test]
fn a_frame_that_is_not_one_object_is_not_spliced() {
    for bad in [&b"[1]"[..], b"{\"session\":{}} trailing", b"{\"session\":"] {
        assert!(
            splice(bad, &rule(serde_json::json!({}))).is_none(),
            "{}",
            text(bad)
        );
    }
}

/// OpenAI Realtime's `response.create` rule, under a session whose output ceiling is `cap`.
fn response_rule(cap: u32) -> Rule {
    let locked = SessionConfig {
        max_output_tokens: Some(MaxOutputTokens::Limit(cap)),
        ..SessionConfig::default()
    };
    OpenAiRealtimeCodec
        .response_rule(&locked)
        .expect("OpenAI Realtime governs the response object")
}

/// The `max_output_tokens` the spliced `response.create` asks the far end for.
fn asked(frame: &[u8], cap: u32) -> Value {
    let out = splice(frame, &response_rule(cap)).expect("spliced");
    let v: Value = serde_json::from_slice(&out).expect("json");
    v["response"]["max_output_tokens"].clone()
}

/// RED-BEFORE-GREEN (lead ruling: a caller's own `max_output_tokens` never bypasses the configured
/// output cap). RED: the response rule set nothing, so a caller's `response.create` asking for
/// more than `streams.max_output_tokens` (or for `"inf"`) went to the far end as written, and the
/// response ran past the operator's ceiling.
#[test]
fn a_callers_response_cap_above_the_ceiling_is_lowered_to_it() {
    let frame = br#"{"type":"response.create","response":{"max_output_tokens":5000,"modalities":["text"]}}"#;
    assert_eq!(asked(frame, 1000), 1000);
    let inf = br#"{"type":"response.create","response":{"max_output_tokens":"inf"}}"#;
    assert_eq!(
        asked(inf, 1000),
        1000,
        "\"inf\" asks for no ceiling of its own"
    );
    let twice =
        br#"{"type":"response.create","response":{"max_output_tokens":10,"max_output_tokens":9000}}"#;
    let out = splice(twice, &response_rule(1000)).expect("spliced");
    assert_eq!(
        text(&out),
        r#"{"type":"response.create","response":{"max_output_tokens":10}}"#,
        "every spelling is read and the member is written once"
    );
}

/// A caller's lower `max_output_tokens` stands: the effective value is the lower of the two, and
/// the caller's other members keep their bytes.
#[test]
fn a_callers_response_cap_below_the_ceiling_stands() {
    let frame =
        br#"{"type":"response.create","response":{"modalities":["text"],"max_output_tokens":200}}"#;
    let out = splice(frame, &response_rule(1000)).expect("spliced");
    assert_eq!(
        text(&out),
        r#"{"type":"response.create","response":{"modalities":["text"],"max_output_tokens":200}}"#
    );
}

/// RED-BEFORE-GREEN (lead ruling): a `response.create` that names no `max_output_tokens` asks for
/// the configured cap. RED: it went out naming none, and the far end read none as its own default.
#[test]
fn a_response_naming_no_cap_asks_for_the_ceiling() {
    let frame = br#"{"type":"response.create","response":{"modalities":["text"]}}"#;
    assert_eq!(asked(frame, 1000), 1000);
}
