// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Tests for `crates/busbar-plane-streaming/src/request_unit.rs`.

use super::*;
use crate::broker::{CALLS_PATH, CLIENT_SECRETS_PATH, SAFETY_IDENTIFIER_HEADER};
use crate::driven::MINT_FAILED_STATUS;

fn unit(door: Door, caller_ref: Option<&str>) -> RequestUnit {
    RequestUnit::open(
        door,
        SessionConfig::default(),
        caller_ref.map(str::to_owned),
    )
    .expect("a one-request door")
}

fn piece(from: From, bytes: &[u8], last: bool) -> Piece<'_> {
    Piece {
        from,
        bytes,
        status: None,
        last,
        head: &[],
    }
}

fn far_first<'a>(
    status: u16,
    bytes: &'a [u8],
    last: bool,
    head: &'a [(&'a [u8], &'a [u8])],
) -> Piece<'a> {
    Piece {
        from: From::FarEnd,
        bytes,
        status: Some(status),
        last,
        head,
    }
}

use crate::tests::body_bytes;

fn reply(a: Answer) -> Reply {
    match a {
        Answer::ToCaller(r) => r,
        other => panic!("expected the caller's answer, got {other:?}"),
    }
}

#[test]
fn a_session_door_is_no_request_unit() {
    for door in [Door::Sideband, Door::Gemini, Door::Twilio, Door::Realtime] {
        assert!(RequestUnit::open(door, SessionConfig::default(), None).is_none());
    }
}

#[test]
fn the_mint_attempt_is_whole_and_the_callers_body_is_not_read() {
    let mut u = unit(Door::Mint, Some("ab12"));
    let Answer::Attempt(a) = u.on_piece(piece(From::Kernel(1), &[], false)) else {
        panic!("the ATTEMPT is answered with the mint request");
    };
    assert_eq!((a.verb, a.target), ("POST", CLIENT_SECRETS_PATH));
    assert!(a
        .fields
        .iter()
        .any(|(n, v)| *n == SAFETY_IDENTIFIER_HEADER && v == "ab12"));
    assert!(!a.body.is_empty(), "the mint body goes with the ATTEMPT");
    assert!(matches!(
        u.on_piece(piece(From::Caller, b"{\"ignored\":true}", true)),
        Answer::Nothing
    ));
}

#[test]
fn a_mint_with_no_caller_reference_names_no_caller() {
    let mut u = unit(Door::Mint, None);
    let Answer::Attempt(a) = u.on_piece(piece(From::Kernel(1), &[], false)) else {
        panic!("the ATTEMPT is answered with the mint request");
    };
    assert!(a.fields.iter().all(|(n, _)| *n != SAFETY_IDENTIFIER_HEADER));
}

#[test]
fn the_far_ends_mint_answer_is_gathered_then_answered_once() {
    let mut u = unit(Door::Mint, None);
    let _ = u.on_piece(piece(From::Kernel(1), &[], false));
    assert!(matches!(
        u.on_piece(far_first(200, b"{\"value\":\"ek_1\",", false, &[])),
        Answer::Nothing
    ));
    let r = reply(u.on_piece(piece(From::FarEnd, b"\"expires_at\":7}", true)));
    assert_eq!(r.status, 200);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body_bytes(r.body)).expect("json"),
        serde_json::json!({"value": "ek_1", "expires_at_unix": 7})
    );
    assert!(u.answered());
    assert!(matches!(
        u.on_piece(piece(From::FarEnd, b"late", true)),
        Answer::Nothing
    ));
}

#[test]
fn a_refused_mint_is_the_served_502_text() {
    let mut u = unit(Door::Mint, None);
    let _ = u.on_piece(piece(From::Kernel(1), &[], false));
    let r = reply(u.on_piece(far_first(401, b"nope", true, &[])));
    assert_eq!(r.status, MINT_FAILED_STATUS);
    assert_eq!(
        body_bytes(r.body),
        b"streaming ephemeral-secret mint failed: ephemeral token mint failed: \
          client-secret endpoint returned 401 Unauthorized"
    );
}

#[test]
fn the_sdp_offer_is_relayed_and_the_answer_keeps_its_location() {
    let mut u = unit(Door::Sdp, None);
    let Answer::Attempt(a) = u.on_piece(piece(From::Kernel(1), &[], false)) else {
        panic!("the ATTEMPT is answered with the SDP request head");
    };
    assert_eq!((a.verb, a.target), ("POST", CALLS_PATH));
    assert!(a.body.is_empty(), "the offer follows as the caller's body");
    assert!(matches!(
        u.on_piece(piece(From::Caller, b"v=0\r\n", false)),
        Answer::ToFarEnd(b) if b == b"v=0\r\n"
    ));
    assert!(matches!(
        u.on_piece(piece(From::Caller, b"", true)),
        Answer::Nothing
    ));
    let head: &[(&[u8], &[u8])] = &[(b"location", b"/v1/realtime/calls/rtc_abc")];
    let r = reply(u.on_piece(far_first(201, b"v=0 answer", true, head)));
    assert_eq!(r.status, 201);
    assert_eq!(body_bytes(r.body), b"v=0 answer");
    assert_eq!(r.rtc_call_id.as_deref(), Some("rtc_abc"));
    assert!(r
        .fields
        .iter()
        .any(|(n, v)| *n == FIELD_LOCATION && v == "/v1/realtime/calls/rtc_abc"));
}

#[test]
fn a_new_attempt_forgets_the_previous_far_end() {
    let mut u = unit(Door::Sdp, None);
    let _ = u.on_piece(piece(From::Kernel(1), &[], false));
    let head: &[(&[u8], &[u8])] = &[(b"location", b"/calls/rtc_old")];
    let _ = u.on_piece(far_first(503, b"stale", false, head));
    let _ = u.on_piece(piece(From::Kernel(2), &[], false));
    let r = reply(u.on_piece(far_first(201, b"fresh", true, &[])));
    assert_eq!(r.rtc_call_id, None);
    assert_eq!(
        (r.status, body_bytes(r.body).as_slice()),
        (201, &b"fresh"[..])
    );
}

#[test]
fn a_far_end_answer_with_no_status_is_refused() {
    let mut u = unit(Door::Sdp, None);
    let _ = u.on_piece(piece(From::Kernel(1), &[], false));
    assert!(matches!(
        u.on_piece(piece(From::FarEnd, b"x", true)),
        Answer::Refused
    ));
}

#[test]
fn a_piece_from_the_kernel_that_is_no_attempt_is_refused() {
    let mut u = unit(Door::Mint, None);
    assert!(matches!(
        u.on_piece(piece(From::Kernel(0), &[], false)),
        Answer::Refused
    ));
}

/// RED-BEFORE-GREEN: a unit's `Debug` gives the far end's gathered answer as a length. A mint's
/// answer carries the live `ek_` secret while it is gathered; RED: the derived `Debug` printed
/// the gathered bytes.
#[test]
fn a_units_debug_prints_no_byte_of_a_gathered_mint_answer() {
    const CANARY: &[u8] = b"ek_CANARY0b9e";
    let mut u = unit(Door::Mint, None);
    let _ = u.on_piece(piece(From::Kernel(1), &[], false));
    let _ = u.on_piece(far_first(200, b"{\"value\":\"", false, &[]));
    let _ = u.on_piece(piece(From::FarEnd, CANARY, false));
    let shown = format!("{u:?}");
    let as_list = CANARY
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    assert!(!shown.contains("CANARY"), "{shown}");
    assert!(!shown.contains(&as_list), "{shown}");
    assert!(shown.contains("far: 23 bytes"), "{shown}");
}
