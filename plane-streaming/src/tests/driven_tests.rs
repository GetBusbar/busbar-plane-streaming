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
    assert_eq!(
        crate::plane_door::ROUTES.len(),
        6,
        "one door per published claim"
    );
}

/// Busbar terminates the browser's media (row 132): the mint door dials nothing, and the SDP door
/// is a session whose far end is busbar's own call, one form, naming the caller for the provider's
/// attribution of the session. RED: the mint dialled the provider's client-secret endpoint.
#[test]
fn the_mint_dials_nothing_and_busbars_call_names_the_caller() {
    assert_eq!(Door::Mint.route(Some("vk-1")), None);
    assert!(!Door::Mint.is_session());
    assert!(Door::Sdp.is_session());
    let call = Door::Sdp.route(Some("vk-1")).expect("a call");
    assert_eq!((call.verb, call.target), ("POST", "/v1/realtime/calls"));
    assert_eq!(
        call.fields,
        vec![
            (
                "content-type",
                "multipart/form-data; boundary=busbar-call-form".to_string()
            ),
            ("OpenAI-Safety-Identifier", "vk-1".to_string()),
        ]
    );
    assert!(
        call.fields
            .iter()
            .all(|(n, _)| !n.eq_ignore_ascii_case("authorization")),
        "the plane never writes the credential; the kernel adds it"
    );
    assert_eq!(Door::Sdp.rides(), crate::plane_door::RIDES_REALTIME_CALL);
    let anonymous = Door::Sdp.route(None).expect("a call");
    assert_eq!(
        anonymous.fields,
        vec![(
            "content-type",
            "multipart/form-data; boundary=busbar-call-form".to_string()
        )],
        "no reference lent, no caller named"
    );
    assert_eq!(
        Door::Sdp.route(Some("")).expect("a call").fields,
        anonymous.fields,
        "an empty reference names no caller"
    );
}

/// The mint door's answer is the ticket and its expiry in the OpenAI-compatible shape; a failed
/// mint keeps the served plane's text.
#[test]
fn a_minted_ticket_is_answered_and_a_failure_keeps_the_served_text() {
    let ok = minted(&crate::tests::sealed_minted("ek_1", 9).ticket, 9);
    assert_eq!(ok.status, 200);
    let v: serde_json::Value =
        serde_json::from_slice(&crate::tests::body_bytes(ok.body)).expect("json");
    assert_eq!(
        v,
        serde_json::json!({"value": "ek_1", "expires_at_unix": 9})
    );
    let refused = mint_failed("the host minted no ticket");
    assert_eq!(refused.status, 502);
    assert_eq!(
        String::from_utf8(crate::tests::body_bytes(refused.body)).expect("utf-8"),
        "streaming ephemeral-secret mint failed: ephemeral token mint failed: the host minted no \
         ticket"
    );
}

/// The sideband door's `{call_id}` is read off the arrival's target, and only an id made of the
/// call ids' characters is taken: nothing that could carry another parameter into a dialled query.
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
    let busbars = crate::ticket::call_id(&[0x5a; 16]);
    assert_eq!(
        sideband_call_id(&format!("/v1/realtime/sideband/{busbars}")),
        Some(busbars.as_str()),
        "busbar's own call ids are taken"
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
fn each_door_routes_and_meters_as_the_plane_answers() {
    let sock = Door::Gemini.route(None).expect("a session dials");
    assert_eq!(sock.verb, "GET");
    assert!(
        !sock.target.contains("key="),
        "the credential is never in the target"
    );
    assert_eq!(
        Door::Sdp.route(Some("vk-1")),
        Some(call_attempt(Some("vk-1")))
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
    let i = crate::plane_door::ROUTES
        .iter()
        .position(|r| r.verb == "GET" && r.target == "/v1/realtime")
        .expect("GET /v1/realtime is a published door");
    let line = crate::plane_door::ROUTES[i];
    assert!(line.upgrade, "the plain socket is an upgrade line");
    assert_eq!(line.carrier, crate::claims::WS_TRANSPORT);
    assert_eq!(line.auth, crate::plane_door::KEY_AUTH, "a keyed door");
    let a = arrive(i as u32).expect("the plain socket arrives at a door");
    assert!(a.door.is_session(), "it opens a live session");
    assert_eq!(a.dialect, 0, "in the OpenAI Realtime dialect");
    assert_eq!(a.door.rides(), crate::plane_door::RIDES_REALTIME_SOCKET);
    let dial = a.door.route(None).expect("a session door dials");
    assert_eq!(
        (dial.verb, dial.target),
        ("GET", REALTIME_SOCKET_PATH),
        "the plain session dials the provider's realtime socket, attached to no call"
    );
}

/// `Debug` of the reply that carries a minted ticket to the caller prints no byte of the live `ek_`
/// ticket, as text or as the body's byte list: a `{:?}` in a log line holds no credential.
#[test]
fn debug_of_a_minted_tickets_reply_prints_no_ticket() {
    const CANARY: &str = "ek_CANARY7c1f9e";
    let reply = minted(&crate::tests::sealed_minted(CANARY, 9).ticket, 9);
    assert_eq!(reply.status, 200);
    let as_bytes = CANARY
        .bytes()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let out = format!("{reply:?}");
    assert!(!out.contains(CANARY), "ek_ ticket in Debug: {out}");
    assert!(!out.contains(&as_bytes), "ek_ ticket bytes in Debug: {out}");
}
