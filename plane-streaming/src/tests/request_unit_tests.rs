// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Tests for `crates/busbar-plane-streaming/src/request_unit.rs`: the mint door, answered by a
//! ticket busbar mints (row 132).

use super::*;

/// A piece from `from`, `last` or not. The mint reads no body: `_bytes` names what the caller
/// sent, for the reader.
fn piece(from: From, _bytes: &[u8], last: bool) -> Piece {
    Piece { from, last }
}

fn locked() -> SessionConfig {
    SessionConfig {
        instructions: Some("locked".into()),
        ..SessionConfig::default()
    }
}

#[test]
fn the_mint_door_asks_for_a_ticket_locking_its_session_once() {
    let mut unit = RequestUnit::open(Door::Mint, locked()).expect("a request door");
    // Nothing before the caller's last piece; an ATTEMPT never comes to a unit that dials nothing.
    assert!(matches!(
        unit.on_piece(piece(From::Caller, b"{", false)),
        Answer::Nothing
    ));
    assert!(matches!(
        unit.on_piece(piece(From::Kernel, b"", false)),
        Answer::Nothing
    ));
    let Answer::Mint { ttl_secs, record } = unit.on_piece(piece(From::Caller, b"}", true)) else {
        panic!("the caller's last piece asks for the ticket");
    };
    assert_eq!(ttl_secs, 600, "a ten-minute ticket");
    assert_eq!(crate::ticket::locked_by(&record).expect("reads"), locked());
    // Asked once.
    assert!(matches!(
        unit.on_piece(piece(From::Caller, b"", true)),
        Answer::Nothing
    ));
    assert!(!unit.answered());
}

/// The mint door's answer: the OpenAI-compatible `{value, expires_at_unix}` shape, the ticket
/// sealed through the SDK (R7).
#[test]
fn a_minted_ticket_is_answered_in_the_shape_sdks_read() {
    let mut unit = RequestUnit::open(Door::Mint, locked()).expect("a request door");
    let _ = unit.on_piece(piece(From::Caller, b"", true));
    let minted = crate::tests::sealed_minted("ek_abc", 1_700_000_600);
    let Answer::ToCaller(reply) = unit.minted(Ok(minted)) else {
        panic!("answered");
    };
    assert_eq!(reply.status, 200);
    assert_eq!(
        reply.fields,
        vec![("content-type", "application/json".to_string())]
    );
    assert!(
        matches!(reply.body, crate::driven::Body::Sealed(_)),
        "the ticket goes out sealed"
    );
    assert_eq!(
        reply.body.public(),
        None,
        "a sealed answer has no public bytes to read"
    );
    let v: serde_json::Value =
        serde_json::from_slice(&crate::tests::body_bytes(reply.body)).expect("json");
    assert_eq!(
        v,
        serde_json::json!({"value": "ek_abc", "expires_at_unix": 1_700_000_600})
    );
    assert!(unit.answered());
    assert!(matches!(
        unit.on_piece(piece(From::Caller, b"", true)),
        Answer::Nothing
    ));
}

/// RED: a mint the host refused is the served plane's 502, naming why and nothing secret.
#[test]
fn a_refused_mint_keeps_the_served_text() {
    let mut unit = RequestUnit::open(Door::Mint, locked()).expect("a request door");
    let _ = unit.on_piece(piece(From::Caller, b"", true));
    let Answer::ToCaller(reply) = unit.minted(Err("no principal".into())) else {
        panic!("answered");
    };
    assert_eq!(reply.status, 502);
    assert_eq!(
        reply.body.public(),
        Some(
            &b"streaming ephemeral-secret mint failed: ephemeral token mint failed: no principal"[..]
        ),
        "a refusal's text is public"
    );
    assert_eq!(
        String::from_utf8(crate::tests::body_bytes(reply.body)).expect("utf-8"),
        "streaming ephemeral-secret mint failed: ephemeral token mint failed: no principal"
    );
}

#[test]
fn a_session_door_opens_no_request_unit_and_a_far_end_never_answers_the_mint() {
    for door in [
        Door::Sdp,
        Door::Sideband,
        Door::Gemini,
        Door::Twilio,
        Door::Realtime,
    ] {
        assert!(RequestUnit::open(door, locked()).is_none(), "{door:?}");
    }
    let mut unit = RequestUnit::open(Door::Mint, locked()).expect("a request door");
    assert!(matches!(
        unit.on_piece(piece(From::FarEnd, b"x", true)),
        Answer::Refused
    ));
}
