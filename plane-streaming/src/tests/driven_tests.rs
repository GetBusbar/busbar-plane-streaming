// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Tests for `crates/busbar-plane-streaming/src/driven.rs`.

use super::*;

#[test]
fn every_published_claim_arrives_at_its_door_in_its_dialect() {
    let doors: Vec<_> = (0..6)
        .map(|c| arrive(c).map(|a| (a.door, a.dialect, a.op_class)))
        .collect();
    assert_eq!(
        doors,
        [
            Some((Door::Mint, 0, 0)),
            Some((Door::Sdp, 0, 0)),
            Some((Door::Sideband, 0, 0)),
            Some((Door::Gemini, 1, 0)),
            Some((Door::Twilio, 2, 0)),
            Some((Door::Realtime, 0, 0)),
        ]
    );
    assert_eq!(
        arrive(6),
        None,
        "a claim the plane never published arrives nowhere (the metadata path is no claim)"
    );
    assert_eq!(crate::door::ROUTES.len(), 6, "one door per published claim");
}

#[test]
fn the_mint_attempt_names_the_caller_and_carries_the_locked_session() {
    let cfg = SessionConfig {
        instructions: Some("locked".into()),
        ..SessionConfig::default()
    };
    let a = mint_attempt(&cfg, Some("vk-1")).expect("serializes");
    assert_eq!((a.verb, a.target), ("POST", "/v1/realtime/client_secrets"));
    assert_eq!(
        a.fields,
        vec![
            ("content-type", "application/json".to_string()),
            ("OpenAI-Safety-Identifier", "vk-1".to_string()),
        ]
    );
    let body: serde_json::Value = serde_json::from_slice(&a.body).expect("json");
    assert_eq!(body["expires_after"]["seconds"], 600);
    assert_eq!(body["session"]["instructions"], "locked");
    assert!(
        a.fields
            .iter()
            .all(|(n, _)| !n.eq_ignore_ascii_case("authorization")),
        "the plane never writes the credential; the kernel adds it"
    );
    let anonymous = mint_attempt(&cfg, None).expect("serializes");
    assert_eq!(
        anonymous.fields,
        vec![("content-type", "application/json".to_string())],
        "no reference lent, no caller named"
    );
}

#[test]
fn a_minted_secret_is_answered_and_a_failure_keeps_the_served_text() {
    let ok = mint_reply(200, br#"{"value":"ek_1","expires_at":9}"#);
    assert_eq!(ok.status, 200);
    let v: serde_json::Value = serde_json::from_slice(&ok.body).expect("json");
    assert_eq!(
        v,
        serde_json::json!({"value": "ek_1", "expires_at_unix": 9})
    );

    let refused = mint_reply(400, b"{}");
    assert_eq!(refused.status, 502);
    assert_eq!(
        String::from_utf8(refused.body).expect("utf-8"),
        "streaming ephemeral-secret mint failed: ephemeral token mint failed: client-secret \
         endpoint returned 400 Bad Request"
    );
    let not_ek = mint_reply(200, br#"{"value":"sk-real"}"#);
    assert_eq!(not_ek.status, 502);
    assert!(!String::from_utf8(not_ek.body)
        .expect("utf-8")
        .contains("sk-real"));
}

#[test]
fn the_sdp_offer_is_relayed_and_its_answer_names_the_call() {
    let a = sdp_attempt(b"v=0\r\n");
    assert_eq!((a.verb, a.target), ("POST", "/v1/realtime/calls"));
    assert_eq!(
        a.fields,
        vec![("content-type", "application/sdp".to_string())]
    );
    assert_eq!(a.body, b"v=0\r\n");

    let r = sdp_reply(
        201,
        Some("/v1/realtime/calls/rtc_abc"),
        Some("application/sdp"),
        b"v=0 answer",
    );
    assert_eq!(r.status, 201);
    assert_eq!(r.rtc_call_id.as_deref(), Some("rtc_abc"));
    assert_eq!(
        r.fields,
        vec![
            ("content-type", "application/sdp".to_string()),
            ("location", "/v1/realtime/calls/rtc_abc".to_string()),
        ]
    );
    assert_eq!(sdp_reply(400, None, None, b"no").rtc_call_id, None);
}

/// RED-BEFORE-GREEN (the caller is answered what the far end answered): an SDP offer the far end
/// refuses is answered with the far end's error body under the far end's own `Content-Type`, never
/// labelled an SDP answer; a far end that states no type gets none invented. RED: every answer
/// said `application/sdp`, so a JSON error body was handed over declared to be an SDP answer.
#[test]
fn a_failed_sdp_offers_error_body_keeps_the_far_ends_content_type() {
    let error = br#"{"error":{"message":"bad offer","type":"invalid_request_error"}}"#;
    let r = sdp_reply(400, None, Some("application/json"), error);
    assert_eq!(r.status, 400);
    assert_eq!(r.body, error.to_vec());
    assert_eq!(
        r.fields,
        vec![("content-type", "application/json".to_string())]
    );
    assert!(sdp_reply(502, None, None, b"upstream down")
        .fields
        .is_empty());
    assert_eq!(
        sdp_reply(201, None, Some("text/plain"), b"v=0").fields,
        vec![("content-type", "application/sdp".to_string())],
        "a success is an SDP answer"
    );
}

/// The sideband door's `{call_id}` is read off the arrival's target, and only an id made of the
/// provider's characters is taken: it is dialled as a query value.
#[test]
fn a_sideband_arrival_names_its_call() {
    assert_eq!(
        sideband_call_id("/v1/realtime/sideband/rtc_abc123"),
        Some("rtc_abc123")
    );
    assert_eq!(
        sideband_call_id("/v1/realtime/sideband/rtc_abc?x=1"),
        Some("rtc_abc")
    );
    for bad in [
        "/v1/realtime/sideband/",
        "/v1/realtime/sideband/rtc_a&model=x",
        "/v1/realtime/sideband/rtc%20a",
        "/v1/realtime/calls/rtc_abc",
    ] {
        assert_eq!(sideband_call_id(bad), None, "{bad}");
    }
}

#[test]
fn a_status_prints_with_its_reason_phrase_when_it_has_one() {
    assert_eq!(status_text(502), "502 Bad Gateway");
    assert_eq!(status_text(299), "299");
}

#[test]
fn each_door_routes_and_meters_as_the_plane_answers() {
    let cfg = SessionConfig::default();
    let sock = Door::Gemini
        .route(&cfg, None, b"")
        .expect("a session dials");
    assert_eq!(sock.verb, "GET");
    assert!(
        !sock.target.contains("key="),
        "the credential is never in the target"
    );
    assert_eq!(
        Door::Sdp.route(&cfg, None, b"v=0").expect("routes"),
        sdp_attempt(b"v=0")
    );
    let mut units = crate::session_unit::CumulativeUnits::default();
    units.0[1] = 20;
    units.0[4] = 2;
    assert_eq!(units.reported(), vec![(1, 20), (4, 2)]);
    assert_eq!(SESSION_OPEN_ACTION, crate::meta::OP_SESSION_OPEN.as_str());
}

/// RED-BEFORE-GREEN (busbar row 159; ARCHITECT ruling 2026-10-23 on `BUSBAR-1.6.0.md` Part 2 #45,
/// "OpenAI Realtime over WS and WebRTC"): plain server-to-server OpenAI Realtime over WS is its own
/// door, mirroring the provider's surface, a WS upgrade on `GET /v1/realtime` (the caller's
/// `?model=` rides its query); the sideband is call-attached only. The plain door is a keyed
/// session door in the OpenAI Realtime dialect that dials the provider's realtime socket bare,
/// attached to no call. RED: no `GET /v1/realtime` line was published.
#[test]
fn plain_openai_realtime_over_ws_is_its_own_session_door() {
    let i = crate::door::ROUTES
        .iter()
        .position(|r| r.verb == "GET" && r.target == "/v1/realtime")
        .expect("GET /v1/realtime is a published door");
    let line = crate::door::ROUTES[i];
    assert!(line.upgrade, "the plain socket is an upgrade line");
    assert_eq!(line.carrier, crate::claims::WS_TRANSPORT);
    assert_eq!(line.auth, crate::door::KEY_AUTH, "a keyed door");
    let a = arrive(i as u32).expect("the plain socket arrives at a door");
    assert!(a.door.is_session(), "it opens a live session");
    assert_eq!(a.dialect, 0, "in the OpenAI Realtime dialect");
    assert_eq!(a.door.rides(), crate::door::RIDES_REALTIME_SOCKET);
    let dial = a
        .door
        .route(&SessionConfig::default(), None, b"")
        .expect("a session door dials");
    assert_eq!(
        (dial.verb, dial.target),
        ("GET", REALTIME_SOCKET_PATH),
        "the plain session dials the provider's realtime socket, attached to no call"
    );
}

/// `Debug` of a minted secret, and of the reply that carries it to the caller, prints no byte of the
/// live `ek_` key, as text or as the body's byte list: a `{:?}` in a log line holds no credential.
#[test]
fn debug_of_a_minted_secret_and_its_reply_prints_no_key() {
    const CANARY: &str = "ek_CANARY7c1f9e";
    let minted =
        crate::broker::read_minted(format!(r#"{{"value":"{CANARY}","expires_at":9}}"#).as_bytes())
            .unwrap();
    let reply = mint_reply(
        200,
        format!(r#"{{"value":"{CANARY}","expires_at":9}}"#).as_bytes(),
    );
    assert_eq!(reply.status, 200);
    let as_bytes = CANARY
        .bytes()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    for out in [format!("{minted:?}"), format!("{reply:?}")] {
        assert!(!out.contains(CANARY), "ek_ key in Debug: {out}");
        assert!(!out.contains(&as_bytes), "ek_ key bytes in Debug: {out}");
    }
}
