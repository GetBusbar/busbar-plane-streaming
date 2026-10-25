// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Tests for `crates/busbar-plane-streaming/src/broker.rs`.

use super::*;
use busbar_contract::abi::sdk::test_support::chip::cred;

use crate::tests::sealed_bytes;

#[test]
fn a_mint_asks_for_the_requested_lifetime_held_inside_its_bounds() {
    assert_eq!(clamped_ttl_secs(None), 600);
    assert_eq!(clamped_ttl_secs(Some(1)), 10);
    assert_eq!(clamped_ttl_secs(Some(99_999)), 7200);
    assert_eq!(clamped_ttl_secs(Some(300)), 300);
}

#[test]
fn the_mint_body_anchors_the_lifetime_and_carries_the_locked_session() {
    let cfg = SessionConfig {
        instructions: Some("be brief".into()),
        ..SessionConfig::default()
    };
    let body: serde_json::Value =
        serde_json::from_slice(&mint_request_body(600, &cfg).expect("serializes")).expect("json");
    assert_eq!(
        body["expires_after"],
        serde_json::json!({"anchor": "created_at", "seconds": 600})
    );
    assert_eq!(body["session"]["instructions"], "be brief");
}

/// RED-BEFORE-GREEN (the GA client-secret endpoint reads the GA `session` object; OpenAI's
/// beta-to-GA migration: "set `session.type`"): the minted browser session is bound in the GA
/// shape, `type: realtime`, its formats and turn detection under `audio`, and under the section's
/// token ceilings once the door applies them. RED: the session went in the pre-GA shape with no
/// `session.type`.
#[test]
fn the_mint_body_binds_the_session_in_the_ga_shape() {
    let cfg = SessionConfig {
        input_audio_format: Some(crate::codec::ir::media::AudioFormat::G711Ulaw),
        max_output_tokens: Some(crate::codec::ir::config::MaxOutputTokens::Limit(1000)),
        context_window_tokens: Some(16_384),
        ..SessionConfig::default()
    };
    let body: serde_json::Value =
        serde_json::from_slice(&mint_request_body(600, &cfg).expect("serializes")).expect("json");
    let session = &body["session"];
    assert_eq!(session["type"], "realtime", "{body}");
    assert!(session.get("input_audio_format").is_none(), "{body}");
    assert_eq!(
        session["audio"]["input"]["format"]["type"], "audio/pcmu",
        "{body}"
    );
    assert_eq!(session["max_output_tokens"], 1000, "{body}");
    assert_eq!(
        session["truncation"]["token_limits"]["post_instructions"],
        16_384 - 1000,
        "{body}"
    );
}

/// X-2204: a mint answer that states no expiry for its `ek_` secret is refused, never carried to
/// the browser as `expires_at_unix: 0`.
#[test]
fn a_minted_answer_with_no_stated_expiry_is_refused() {
    for body in [
        &br#"{"value":"ek_live"}"#[..],
        &br#"{"value":"ek_live","expires_at":null}"#[..],
        &br#"{"value":"ek_live","expires_at":0}"#[..],
    ] {
        let refused = read_minted(body).expect_err("no stated expiry");
        assert!(refused.contains("no expiry"), "{refused}");
        assert!(
            !refused.contains("ek_live"),
            "the secret is not echoed: {refused}"
        );
    }
}

#[test]
fn a_minted_answer_reads_the_ek_secret_and_its_expiry() {
    let m = read_minted(br#"{"value":"ek_live","expires_at":1700000600}"#).expect("reads");
    assert_eq!(
        m,
        Minted {
            value: cred("ek_live"),
            expires_at_unix: 1_700_000_600
        }
    );
    // An answer stating no expiry no longer reads as `expires_at_unix: 0`: it is refused (X-2204,
    // `a_minted_answer_with_no_stated_expiry_is_refused`).
}

#[test]
fn a_mint_answer_that_is_not_an_ephemeral_secret_is_refused() {
    assert_eq!(
        read_minted(br#"{"value":"sk-real"}"#),
        Err("client-secret response value is not an ek_ ephemeral secret".to_string())
    );
    let err = read_minted(b"not json").expect_err("refused");
    assert!(
        err.starts_with("client-secret response did not parse: "),
        "{err}"
    );
}

#[test]
fn the_browser_is_answered_the_secret_and_its_expiry_only() {
    let body = sealed_bytes(
        &mut minted_answer(&Minted {
            value: cred("ek_1"),
            expires_at_unix: 5,
        })
        .expect("text"),
    );
    let read: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        read,
        serde_json::json!({"value": "ek_1", "expires_at_unix": 5})
    );
}

#[test]
fn the_call_id_is_the_rtc_segment_of_the_location() {
    assert_eq!(
        rtc_call_id_of("/v1/realtime/calls/rtc_abc?x=1"),
        Some("rtc_abc".into())
    );
    assert_eq!(
        rtc_call_id_of("https://p.example/v1/realtime/calls/rtc_z#f"),
        Some("rtc_z".into())
    );
    assert_eq!(rtc_call_id_of("/v1/realtime/calls/abc"), None);
}

/// The browser's answer as 1.6.0-pre built it: the secret, held as text, in `serde_json`'s object.
fn minted_answer_as_built_before(value: &str, expires_at_unix: u64) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "value": value,
        "expires_at_unix": expires_at_unix,
    }))
    .unwrap_or_default()
}

/// THE ANSWER IS BYTE FOR BYTE WHAT IT WAS: the SDK-sealed answer, read off the provider's mint
/// answer, is the bytes the text-holding answer wrote for the same input, every escape included.
#[test]
fn the_sealed_answer_is_the_answer_as_built_before() {
    // Each as the provider's JSON spells it, escapes and all.
    let secrets = [
        r#"ek_1"#,
        r#"ek_68d2b0c0a1e4819092f3b5d6c7e8f901"#,
        r#"ek_q\"uote\\back\/slash"#,
        r#"ek_\u0001\b\f\n\r\t\u001f\u007f"#,
        r#"ek_h\u00e9llo \u2713 \ud83d\ude00 \u2028 é"#,
    ];
    for (i, escaped) in secrets.iter().enumerate() {
        let expires = [1_u64, 5, 1_767_225_600, u64::MAX, 600][i];
        let body = format!(
            r#"{{"value":"{escaped}","expires_at":{expires},"session":{{"type":"realtime"}}}}"#
        );
        let text: String =
            serde_json::from_str(&format!("\"{escaped}\"")).expect("the test's own string");
        let minted = read_minted(body.as_bytes()).expect("reads");
        let mut sealed = minted_answer(&minted).expect("text");
        assert_eq!(
            sealed.len(),
            minted_answer_as_built_before(&text, expires).len()
        );
        assert_eq!(
            sealed_bytes(&mut sealed),
            minted_answer_as_built_before(&text, expires),
            "{escaped}"
        );
    }
}

/// RED-BEFORE-GREEN: the mint answer's decode failure is described by its class and position, the
/// texts the caller was answered before, word for word, and never the value the decoder met.
#[test]
fn a_mint_answer_that_does_not_parse_keeps_its_text_and_quotes_no_value() {
    for (body, want) in [
        (
            &br#"{"value":5}"#[..],
            "client-secret response did not parse: a field is missing or has the wrong type \
             (line 1, column 10; the decoder's text is withheld)",
        ),
        (
            &b"not json"[..],
            "client-secret response did not parse: it is not well-formed JSON (line 1, column 2; \
             the decoder's text is withheld)",
        ),
        (
            &br#"{"value":"ek_live","expires_at":"ek_live"}"#[..],
            "client-secret response did not parse: a field is missing or has the wrong type \
             (line 1, column 41; the decoder's text is withheld)",
        ),
        (
            &br#"{"value":"ek_live""#[..],
            "client-secret response did not parse: it ends before the JSON does (line 1, column \
             18; the decoder's text is withheld)",
        ),
    ] {
        let refused = read_minted(body).expect_err("refused");
        assert_eq!(refused, want);
        assert!(!refused.contains("ek_live"), "{refused}");
    }
}

/// `Debug` of a minted secret names its expiry and nothing of the secret.
#[test]
fn a_minted_secret_debugs_its_expiry_only() {
    let m = read_minted(br#"{"value":"ek_CANARY51","expires_at":9}"#).expect("reads");
    assert_eq!(format!("{m:?}"), "Minted { expires_at_unix: 9, .. }");
}
