// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Tests for `plane-streaming/src/ticket.rs`.

use super::*;

#[test]
fn a_mint_asks_for_a_ten_minute_ticket() {
    assert_eq!(TTL_SECS, 600);
}

/// The mint door's answer is the OpenAI-compatible shape SDKs read, and nothing else: the ticket
/// sealed through the SDK (R7), members in name order.
#[test]
fn the_caller_is_answered_the_ticket_and_its_expiry_only() {
    let m = crate::tests::sealed_minted("ek_abc", 1_700_000_600);
    let mut answer = minted_answer(&m.ticket, m.expires_at_unix).expect("text");
    let wire = crate::tests::sealed_bytes(&mut answer);
    assert_eq!(
        wire,
        br#"{"expires_at_unix":1700000600,"value":"ek_abc"}"#.to_vec()
    );
    assert!(TICKET_PREFIX == "ek_", "SDKs pass the ek_ prefix through");
}

#[test]
fn a_tickets_record_locks_its_session_and_reads_back() {
    let locked = SessionConfig {
        instructions: Some("locked".into()),
        ..SessionConfig::default()
    };
    let record = ticket_record(&locked).expect("serializes");
    assert_eq!(locked_by(&record).expect("reads"), locked);
    // RED: a record that is not a session config.
    assert!(locked_by(b"not json").is_err());
    assert!(locked_by(b"[1,2]").is_err());
}

/// The upstream form: the locked session first, then the opened part the description fills.
#[test]
fn the_upstream_form_carries_the_session_and_opens_the_description_part() {
    let locked = SessionConfig {
        instructions: Some("be brief".into()),
        ..SessionConfig::default()
    };
    let lead = call_form(&locked).expect("serializes");
    assert_eq!(
        CALL_FORM_TYPE,
        format!("multipart/form-data; boundary={FORM_BOUNDARY}")
    );
    let lead = String::from_utf8(lead).expect("utf-8");
    let session =
        serde_json::to_string(&serde_json::Value::Object(locked.ga_session())).expect("serializes");
    assert_eq!(
        lead,
        format!(
            "--busbar-call-form\r\nContent-Disposition: form-data; name=\"session\"\r\n\
             Content-Type: application/json\r\n\r\n{session}\r\n--busbar-call-form\r\n\
             Content-Disposition: form-data; name=\"sdp\"\r\nContent-Type: application/sdp\r\n\r\n"
        )
    );
}

/// RED-BEFORE-GREEN (the GA calls endpoint reads the GA `session` object; OpenAI's beta-to-GA
/// migration: "set `session.type`"): busbar's call binds the browser's session in the GA shape,
/// `type: realtime`, its formats under `audio`, and under the section's token ceilings once the door
/// applies them. RED: the session went in the pre-GA shape with no `session.type`.
#[test]
fn the_upstream_form_binds_the_session_in_the_ga_shape() {
    let locked = SessionConfig {
        input_audio_format: Some(crate::codec::ir::media::AudioFormat::G711Ulaw),
        max_output_tokens: Some(crate::codec::ir::config::MaxOutputTokens::Limit(1000)),
        context_window_tokens: Some(16_384),
        ..SessionConfig::default()
    };
    let lead = call_form(&locked).expect("serializes");
    let lead = String::from_utf8(lead).expect("utf-8");
    let json = lead
        .split("\r\n\r\n")
        .nth(1)
        .and_then(|part| part.split("\r\n--").next())
        .expect("the session part");
    let session: serde_json::Value = serde_json::from_str(json).expect("json");
    assert_eq!(session["type"], "realtime", "{session}");
    assert!(session.get("input_audio_format").is_none(), "{session}");
    assert_eq!(
        session["audio"]["input"]["format"]["type"], "audio/pcmu",
        "{session}"
    );
    assert_eq!(session["max_output_tokens"], 1000, "{session}");
    assert_eq!(
        session["truncation"]["token_limits"]["post_instructions"],
        16_384 - 1000,
        "{session}"
    );
}

#[test]
fn busbars_call_id_is_opaque_and_named_by_its_location() {
    let id = call_id(&[0xab; 16]);
    assert_eq!(id, format!("rtc_{}", "ab".repeat(16)));
    assert_ne!(call_id(&[0; 16]), call_id(&[1; 16]));
    assert_eq!(call_location(&id), format!("/v1/realtime/calls/{id}"));
    assert_eq!(call_id_of(&call_location(&id)), Some(id));
}

#[test]
fn the_providers_call_id_is_the_call_segment_of_its_location() {
    assert_eq!(
        call_id_of("/v1/realtime/calls/rtc_abc?x=1"),
        Some("rtc_abc".into())
    );
    assert_eq!(
        call_id_of("https://p.example/v1/realtime/calls/rtc_z#f"),
        Some("rtc_z".into())
    );
    // RED: no call segment.
    assert_eq!(call_id_of("/v1/realtime/calls/abc"), None);
    assert_eq!(call_id_of("/v1/realtime/calls/rtc_"), None);
}

/// RED (audit plane-streaming round 52b0f93ac9, S76): the provider's call id a far end's `Location`
/// names is taken only when it is the same well-formed id a sideband arrival's is (ASCII letters,
/// digits, `_`, `-`): an untrusted answer naming `rtc_x&model=y` would otherwise add parameters to
/// the sideband's dial (`?call_id=`, spliced unescaped). RED: any `rtc_` segment was taken.
#[test]
fn a_provider_call_id_carrying_anything_but_id_characters_is_not_taken() {
    for location in [
        "/v1/realtime/calls/rtc_x&model=y",
        "/v1/realtime/calls/rtc_x%26model=y",
        "/v1/realtime/calls/rtc_x model",
        "/v1/realtime/calls/rtc_é",
    ] {
        assert_eq!(call_id_of(location), None, "{location}");
    }
    assert_eq!(
        call_id_of("/v1/realtime/calls/rtc_A-b_9"),
        Some("rtc_A-b_9".to_string())
    );
}
