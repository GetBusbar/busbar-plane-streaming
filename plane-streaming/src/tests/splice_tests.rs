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
        caps: Vec::new(),
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

/// RED-BEFORE-GREEN (lead ruling): the pre-GA spelling of the output ceiling a caller writes in its
/// `session.update`, `max_response_output_tokens`, is held under the same ceiling as
/// `max_output_tokens`: the lower of the caller's value and the cap, the cap for `"inf"`. A frame
/// that does not write it is not given it; the GA `max_output_tokens` the lock sets carries the
/// cap. RED: the beta key was neither cut nor set, so a caller's 5000 or `"inf"` reached the far
/// end past the section's 1000.
#[test]
fn a_callers_pre_ga_session_output_cap_is_held_under_the_ceiling() {
    let locked = SessionConfig {
        max_output_tokens: Some(MaxOutputTokens::Limit(1000)),
        ..SessionConfig::default()
    };
    let rule = OpenAiRealtimeCodec.configure_rule(&locked);
    let session = |frame: &[u8]| -> Value {
        let out = splice(frame, &rule).expect("spliced");
        let v: Value = serde_json::from_slice(&out).expect("json");
        v["session"].clone()
    };
    let above =
        session(br#"{"type":"session.update","session":{"max_response_output_tokens":5000}}"#);
    assert_eq!(above["max_response_output_tokens"], 1000, "{above}");
    assert_eq!(above["max_output_tokens"], 1000, "{above}");
    let inf =
        session(br#"{"type":"session.update","session":{"max_response_output_tokens":"inf"}}"#);
    assert_eq!(inf["max_response_output_tokens"], 1000, "{inf}");
    let below =
        session(br#"{"type":"session.update","session":{"max_response_output_tokens":200}}"#);
    assert_eq!(below["max_response_output_tokens"], 200, "{below}");
    let absent = session(br#"{"type":"session.update","session":{"voice":"verse"}}"#);
    assert!(
        absent.get("max_response_output_tokens").is_none(),
        "{absent}"
    );
    assert_eq!(absent["max_output_tokens"], 1000, "{absent}");
}

/// The session locked under an output ceiling of `cap`.
fn capped_session(cap: u32) -> SessionConfig {
    SessionConfig {
        max_output_tokens: Some(MaxOutputTokens::Limit(cap)),
        ..SessionConfig::default()
    }
}

/// RED-BEFORE-GREEN (lead ruling: the effective output ceiling is the lower of the caller's value
/// and the cap): a caller's GA `session.update` asking for less than the cap keeps its own value; one
/// asking for more, or for `"inf"`, is lowered to the cap; one naming none gets the cap. RED (the
/// lower value): the lock set `max_output_tokens` over the caller's, so a caller's 200 went out as
/// the cap's 1000. The higher and absent cases held before as well.
#[test]
fn a_callers_ga_session_output_cap_is_the_lower_of_its_own_and_the_ceiling() {
    let rule = OpenAiRealtimeCodec.configure_rule(&capped_session(1000));
    let session = |frame: &[u8]| -> Value {
        let out = splice(frame, &rule).expect("spliced");
        let v: Value = serde_json::from_slice(&out).expect("json");
        v["session"].clone()
    };
    let lower = session(br#"{"type":"session.update","session":{"max_output_tokens":200}}"#);
    assert_eq!(lower["max_output_tokens"], 200, "{lower}");
    let higher = session(br#"{"type":"session.update","session":{"max_output_tokens":5000}}"#);
    assert_eq!(higher["max_output_tokens"], 1000, "{higher}");
    let inf = session(br#"{"type":"session.update","session":{"max_output_tokens":"inf"}}"#);
    assert_eq!(inf["max_output_tokens"], 1000, "{inf}");
    let absent = session(br#"{"type":"session.update","session":{"voice":"verse"}}"#);
    assert_eq!(absent["max_output_tokens"], 1000, "{absent}");
}

/// RED-BEFORE-GREEN (lead ruling), Gemini Live: a caller's `setup.generationConfig.maxOutputTokens`
/// below the cap is kept (under either spelling of its keys), one above it is lowered, and a setup
/// naming none, or no `generationConfig` at all, gets the cap; the caller's other
/// `generationConfig` members keep their values. RED (the lower value): the lock merged its
/// `maxOutputTokens` over the caller's, so a caller's 200 went out as the cap's 1000. The higher and
/// absent cases held before as well. RED (round 4): a `null` generationConfig passed through, and
/// the cap was never written.
#[test]
fn a_gemini_callers_output_cap_is_the_lower_of_its_own_and_the_ceiling() {
    let rule =
        crate::codec::ir::codec::gemini::GeminiLiveCodec.configure_rule(&capped_session(1000));
    let config = |frame: &[u8]| -> Value {
        let out = splice(frame, &rule).expect("spliced");
        let v: Value = serde_json::from_slice(&out).expect("json");
        v["setup"]["generationConfig"].clone()
    };
    let lower =
        config(br#"{"setup":{"generationConfig":{"maxOutputTokens":200,"temperature":0.3}}}"#);
    assert_eq!(lower["maxOutputTokens"], 200, "{lower}");
    assert_eq!(lower["temperature"], 0.3, "{lower}");
    let snake = config(br#"{"setup":{"generation_config":{"max_output_tokens":200}}}"#);
    assert_eq!(snake["maxOutputTokens"], 200, "{snake}");
    assert!(
        snake.get("max_output_tokens").is_none(),
        "one spelling: {snake}"
    );
    let higher = config(br#"{"setup":{"generationConfig":{"maxOutputTokens":5000}}}"#);
    assert_eq!(higher["maxOutputTokens"], 1000, "{higher}");
    let absent = config(br#"{"setup":{"generationConfig":{"temperature":0.3}}}"#);
    assert_eq!(absent["maxOutputTokens"], 1000, "{absent}");
    let none = config(br#"{"setup":{"model":"models/gemini-live"}}"#);
    assert_eq!(none["maxOutputTokens"], 1000, "{none}");
    // A `null` generationConfig (either spelling) reads under protobuf JSON as the field unset:
    // left as written, the far end would run at its own default, past the cap.
    for frame in [
        &br#"{"setup":{"generationConfig":null}}"#[..],
        br#"{"setup":{"generation_config":null}}"#,
    ] {
        let out = splice(frame, &rule).expect("spliced");
        let v: Value = serde_json::from_slice(&out).expect("json");
        assert_eq!(
            v["setup"]["generationConfig"]["maxOutputTokens"],
            1000,
            "{}",
            text(&out)
        );
        assert!(
            v["setup"].get("generation_config").is_none(),
            "one spelling: {}",
            text(&out)
        );
    }
}
