// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Tests for `crates/busbar-plane-streaming/src/splice.rs`: only the governed object is
//! rebuilt, the caller's members keep their bytes, keys are read as the far end reads them.

use super::*;

fn rule(set: serde_json::Value) -> Rule {
    Rule {
        at: "session",
        cut: vec!["instructions", "tools"],
        cut_under: Vec::new(),
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
fn a_locked_tagged_union_replaces_the_callers_variant() {
    let frame =
        br#"{"session":{"turn_detection":{"type":"server_vad","threshold":0.9,"extra":true}}}"#;
    let out = splice(
        frame,
        &rule(serde_json::json!({"turn_detection":{"threshold":0.5}})),
    )
    .expect("spliced");
    let v: Value = serde_json::from_slice(&out).expect("json");
    assert!(v["session"]["turn_detection"].get("type").is_none());
    assert!(v["session"]["turn_detection"].get("extra").is_none());
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

/// A caller's response cap above the configured ceiling is lowered to it.
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

/// A caller's lower `max_output_tokens` stands: the effective value is the lower of the two,
/// and the caller's other members keep their bytes.
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

/// A response without an explicit cap uses the configured ceiling.
#[test]
fn a_response_naming_no_cap_asks_for_the_ceiling() {
    let frame = br#"{"type":"response.create","response":{"modalities":["text"]}}"#;
    assert_eq!(asked(frame, 1000), 1000);
}

/// A caller's pre-GA session output cap stays under the configured ceiling.
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

/// A GA session uses the lower of the caller's output cap and the configured ceiling.
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

/// A Gemini setup uses the lower of the caller's output cap and the configured ceiling.
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

/// A frame repeating a governed member is refused without amplification.
#[test]
fn a_frame_repeating_the_governed_member_is_refused_never_amplified() {
    let locked = rule(serde_json::json!({"instructions": "x".repeat(4096)}));
    let mut frame = String::from(r#"{"type":"session.update""#);
    for _ in 0..1000 {
        frame.push_str(r#","session":{}"#);
    }
    frame.push('}');
    assert!(
        splice(frame.as_bytes(), &locked).is_none(),
        "a thousand copies"
    );
    for frame in [
        &br#"{"session":{},"session":{}}"#[..],
        br#"{"session":1,"session":{"voice":"verse"}}"#,
        br#"{"session":{},"session":{}}"#,
    ] {
        assert!(splice(frame, &locked).is_none(), "{}", text(frame));
    }
    let once = splice(br#"{"session":{}}"#, &locked).expect("one copy is spliced");
    assert!(text(&once).contains("xxxx"));
}

/// The Realtime ceiling error event states its error type.
#[test]
fn the_ceiling_error_event_states_its_error_type() {
    let frame = OpenAiRealtimeCodec
        .ceiling_frame("session_expired", "the session reached its ceiling")
        .expect("OpenAI Realtime has an in-band error");
    let v: Value = serde_json::from_slice(&frame).expect("json");
    assert_eq!(v["type"], "error");
    assert_eq!(v["error"]["type"], "invalid_request_error", "{v}");
    assert_eq!(v["error"]["code"], "session_expired");
}

#[test]
fn audit_response_cannot_override_locked_voice_modality_or_tool_choice() {
    let locked = SessionConfig {
        voice: Some("alloy".into()),
        modalities: vec!["text".into()],
        tool_choice: Some(serde_json::json!("none")),
        output_audio_format: Some(AudioFormat::G711Ulaw),
        max_output_tokens: Some(MaxOutputTokens::Limit(99)),
        ..SessionConfig::default()
    };
    let rule = OpenAiRealtimeCodec.response_rule(&locked).unwrap();
    let frame = br#"{"type":"response.create","response":{"voice":"verse","modalities":["audio"],"output_modalities":["audio"],"tool_choice":"required","audio":{"output":{"voice":"verse","format":{"type":"audio/pcm","rate":24000}}},"max_output_tokens":999}}"#;
    let out = splice(frame, &rule).unwrap();
    let value: Value = serde_json::from_slice(&out).unwrap();
    let response = &value["response"];
    assert!(response.get("voice").is_none());
    assert!(response.get("modalities").is_none());
    assert_eq!(response["audio"]["output"]["voice"], "alloy");
    assert_eq!(response["audio"]["output"]["format"]["type"], "audio/pcmu");
    assert_eq!(response["output_modalities"], serde_json::json!(["text"]));
    assert_eq!(response["tool_choice"], "none");
    assert_eq!(response["max_output_tokens"], 99);
}

#[test]
fn audit_transcription_is_removed_even_without_an_explicit_audio_lock() {
    let rule = OpenAiRealtimeCodec.configure_rule(&SessionConfig::default());
    let frame = br#"{"session":{"input_audio_transcription":{"model":"transcribe"},"audio":{"input":{"transcription":{"model":"transcribe"},"noise_reduction":{"type":"near_field"}}}}}"#;
    let out = splice(frame, &rule).unwrap();
    let value: Value = serde_json::from_slice(&out).unwrap();
    assert!(value["session"].get("input_audio_transcription").is_none());
    assert!(value["session"]["audio"]["input"]
        .get("transcription")
        .is_none());
    assert_eq!(
        value["session"]["audio"]["input"]["noise_reduction"]["type"],
        "near_field"
    );
}

#[test]
fn audit_tagged_vad_union_does_not_keep_the_callers_other_variant_fields() {
    let locked = SessionConfig {
        turn_detection: Some(Some(crate::codec::ir::config::IrVad::SemanticVad {
            eagerness: crate::codec::ir::config::Eagerness::Auto,
        })),
        ..SessionConfig::default()
    };
    let rule = OpenAiRealtimeCodec.configure_rule(&locked);
    let out = splice(br#"{"session":{"turn_detection":{"type":"server_vad"},"audio":{"input":{"turn_detection":{"type":"server_vad","threshold":0.9}}}}}"#, &rule).unwrap();
    let value: Value = serde_json::from_slice(&out).unwrap();
    assert!(value["session"].get("turn_detection").is_none());
    let vad = &value["session"]["audio"]["input"]["turn_detection"];
    assert_eq!(vad["type"], "semantic_vad");
    assert!(vad.get("threshold").is_none());
}
