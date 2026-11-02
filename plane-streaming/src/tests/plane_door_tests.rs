// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Tests for `plane-streaming/src/plane_door.rs`: the P-item pins the door holds (spec

use super::*;

/// The Statement's version is the crate's, as each sibling plane's literal is pinned to its own.
#[test]
fn the_statement_names_the_crates_version() {
    assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
}

#[test]
fn p_item_wrong_provider_attribution_a_session_rides_the_dialed_provider_not_the_first() {
    use crate::driven::{arrive, GEMINI_SOCKET_PATH, REALTIME_SOCKET_PATH};
    use crate::session_door::PER_SESSION_CLASS;

    let cfg = StreamsCfg::default();

    let served = [
        (
            Door::Gemini,
            RIDES_LIVE_SOCKET,
            GEMINI_SOCKET_PATH,
            GEMINI_LIVE,
        ),
        (
            Door::Sideband,
            RIDES_REALTIME_SOCKET,
            REALTIME_SOCKET_PATH,
            OPENAI_REALTIME,
        ),
        (
            Door::Twilio,
            RIDES_REALTIME_SOCKET,
            REALTIME_SOCKET_PATH,
            TWILIO_MEDIA_STREAMS,
        ),
        (
            Door::Realtime,
            RIDES_REALTIME_SOCKET,
            REALTIME_SOCKET_PATH,
            OPENAI_REALTIME,
        ),
    ];
    for (door, need, path, dialect) in served {
        assert_eq!(door.rides(), need, "{door:?} rides its own provider's need");
        let declared = &NEEDS[need as usize - 1];
        assert_eq!(declared.direction, DIRECTION_OUTBOUND, "{door:?}");
        assert_eq!(declared.egress_class, EGRESS_PROVIDER, "{door:?}");
        let attempt = door.route(None).expect("a session door dials");
        assert_eq!(
            attempt.target, path,
            "{door:?} dials its own provider's socket"
        );
        let arrival = arrive(door as u32).expect("a published door");
        assert_eq!(
            DIALECTS_BY_NAME[arrival.dialect as usize], dialect,
            "{door:?} arrives in its own dialect"
        );
    }
    assert_ne!(
        Door::Gemini.rides(),
        RIDES_REALTIME_SOCKET,
        "a Gemini Live session never rides the first-declared provider's socket"
    );

    assert_eq!(Door::Sdp.rides(), RIDES_REALTIME_CALL);
    assert_eq!(
        NEEDS[RIDES_REALTIME_CALL as usize - 1].direction,
        DIRECTION_OUTBOUND
    );

    let report = br#"{"usageMetadata":{"promptTokenCount":10,"responseTokenCount":20}}"#;
    let text_in = crate::session_unit::CLASSES
        .iter()
        .position(|c| *c == crate::meta::CLASS_TEXT_TOKENS_IN)
        .expect("declared") as u32;
    let text_out = crate::session_unit::CLASSES
        .iter()
        .position(|c| *c == crate::meta::CLASS_TEXT_TOKENS_OUT)
        .expect("declared") as u32;
    let mut gemini = Live::open(Door::Gemini, &cfg).expect("a session door");
    gemini.accept();
    gemini.from_far_end(report);
    assert_eq!(
        gemini.units(),
        [(text_in, 10), (text_out, 20), (PER_SESSION_CLASS, 1)],
        "the Gemini session is metered on the Gemini far end's report"
    );
    let mut openai = Live::open(Door::Sideband, &cfg).expect("a session door");
    openai.accept();
    openai.from_far_end(report);
    assert_eq!(
        openai.units(),
        [(PER_SESSION_CLASS, 1)],
        "an OpenAI Realtime session is never metered on another provider's report"
    );
}

#[test]
fn p_item_refusal_reason_collapse_only_the_kernels_class_picks_the_answer_and_none_reads_internal()
{
    const WORDS: &str = "the kernel's words";
    for status in [400u16, 401, 403, 404, 413, 429, 500, 502, 503, 504] {
        let mut answers: Vec<(RefusalClass, Vec<u8>)> = Vec::new();
        for code in RefusalCode::ALL {
            if reason_of(code.code()).is_none() {
                continue;
            }
            let class = code.class();
            let bytes = refusal_render(status, code.code(), WORDS);
            let body: serde_json::Value =
                serde_json::from_slice(&bytes).expect("the refusal is the dialect's JSON");
            let kind = body["error"]["type"]
                .as_str()
                .expect("the error names its type");
            assert_eq!(
                kind == "server_error",
                status >= 500,
                "{code:?} (class {class:?}) at {status} renders {kind:?}"
            );
            let (message, error_code) = if class == RefusalClass::Unauthenticated {
                (
                    AUTH_FAILURE_MESSAGE,
                    serde_json::Value::from(AUTH_FAILURE_CODE),
                )
            } else {
                (WORDS, serde_json::Value::Null)
            };
            assert_eq!(body["error"]["message"], message, "{code:?} at {status}");
            assert_eq!(body["error"]["code"], error_code, "{code:?} at {status}");
            match answers.iter().find(|(c, _)| *c == class) {
                Some((_, first)) => assert_eq!(
                    *first, bytes,
                    "{code:?} answers differently from the rest of {class:?} at {status}"
                ),
                None => answers.push((class, bytes)),
            }
        }
    }
}

/// (Moved from `plane-streaming-plugin/tests/both_ways.rs`, which reaches only the door.)
#[test]
fn a_refusal_is_rendered_in_the_dialects_error_shape() {
    for (status, kind) in [
        (401, "authentication_error"),
        (403, "permission_error"),
        (404, "not_found_error"),
        (429, "rate_limit_error"),
        (400, "invalid_request_error"),
        (503, "server_error"),
    ] {
        let body: serde_json::Value =
            serde_json::from_slice(&refusal_body(status, "no sessions today")).expect("JSON");
        assert_eq!(body["error"]["type"], kind, "{status}");
        assert_eq!(body["error"]["message"], "no sessions today");
    }
}

/// The vendor's own words for a bad or missing credential (moved from the cdylib crate's
#[test]
fn the_bad_key_answer_is_the_vendors_own_copy() {
    assert_eq!(AUTH_FAILURE_MESSAGE, "Incorrect API key provided.");
    assert_eq!(AUTH_FAILURE_CODE, "invalid_api_key");
}

/// A mint unit's state, as the door holds it from its first piece.
fn held_mint() -> Held {
    Held {
        unit: RequestUnit::open(
            Door::Mint,
            crate::codec::ir::config::SessionConfig::default(),
        )
        .expect("a one-request door"),
        owed: Owed::default(),
        short: None,
    }
}

/// Cancelled request units are forgotten; a full table refuses without evicting live units.
#[test]
fn a_cancelled_request_unit_is_forgotten_and_a_full_table_refuses_rather_than_evicts() {
    let mut units = Units::default();
    let ticket = |unit: u64| Ticket {
        slot: u32::try_from(unit).expect("small") + 1,
        generation: 1,
    };
    for unit in 0..MAX_UNITS as u64 {
        assert!(units.hold(unit, held_mint()), "unit {unit}");
        units.crossed(unit, ticket(unit));
    }
    let late = u64::MAX;
    assert!(!units.hold(late, held_mint()), "a full table refuses");
    assert!(
        units.held.contains_key(&0),
        "the oldest unit in flight is kept"
    );
    assert!(units.cancel(ticket(0), 0), "the cancel names a held unit");
    assert!(
        !units.held.contains_key(&0),
        "the cancelled unit is forgotten"
    );
    assert!(!units.cancel(ticket(0), 0), "and its ticket with it");
    assert!(units.hold(late, held_mint()), "its room is free");
}

/// An open instance with no generation published, as `open` builds one.
fn plane() -> Plane {
    Plane {
        public_url: None,
        generations: Mutex::new(Vec::new()),
        snapshots: Generations::new(),
        units: Mutex::new(Units::default()),
        calls: Mutex::new(PendingCalls::default()),
        brokered: Mutex::new(Brokered::default()),
        services: None,
        sessions: Mutex::new(Sessions::default()),
        wake: None,
        driver: Mutex::new((Ticket::NONE, 0)),
    }
}

/// The plane tail takes a unit's refusal as a refused piece first.
#[test]
fn the_tail_takes_a_units_refusal_as_a_piece_first() {
    assert_eq!(TAIL.flags & TAIL_REFUSED_PIECE, TAIL_REFUSED_PIECE);
}

/// Refusing a unit releases its table entry.
#[test]
fn a_refused_unit_is_forgotten_and_its_room_is_free() {
    let p = plane();
    {
        let mut units = lock(&p.units);
        for unit in 1..=MAX_UNITS as u64 {
            assert!(units.hold(unit, held_mint()), "unit {unit}");
        }
        assert!(
            !units.hold(MAX_UNITS as u64 + 1, held_mint()),
            "the table is full"
        );
    }
    assert!(lock(&p.calls).pend(1, "rtc_1".to_string()));
    p.end_unit(1);
    assert!(
        !lock(&p.units).held.contains_key(&1),
        "the refused unit is forgotten"
    );
    assert_eq!(lock(&p.calls).take(1), None, "its pending call with it");
    assert!(
        lock(&p.units).hold(MAX_UNITS as u64 + 1, held_mint()),
        "its room is free"
    );
    assert!(
        lock(&p.units).held.contains_key(&2),
        "no other unit is touched"
    );
}

/// Cancelling a unit by its key releases its table entry.
#[test]
fn a_unit_the_kernel_cancels_by_its_key_is_forgotten() {
    let mut units = Units::default();
    assert!(units.hold(7, held_mint()));
    assert!(units.hold(8, held_mint()));
    assert!(
        units.cancel(Ticket::NONE, 7),
        "the cancel names a held unit by its key"
    );
    assert!(
        !units.held.contains_key(&7),
        "the cancelled unit is forgotten"
    );
    assert!(units.held.contains_key(&8), "no other unit is touched");
    assert!(
        !units.cancel(Ticket::NONE, 0),
        "a cancel naming no unit forgets none"
    );
    assert!(units.held.contains_key(&8));
}

/// A full table of live units refuses another unit.
#[test]
fn a_full_table_of_live_units_still_refuses() {
    let mut units = Units::default();
    for unit in 0..MAX_UNITS as u64 {
        assert!(units.hold(unit, held_mint()), "unit {unit}");
    }
    assert!(!units.hold(u64::MAX, held_mint()), "a full table refuses");
    assert_eq!(units.held.len(), MAX_UNITS, "no live unit is dropped");
    assert!(!units.held.contains_key(&u64::MAX));
}

/// A full pending-call table refuses the next arrival and retains every existing call.
#[test]
fn a_full_pending_call_table_refuses_the_next_arrival_and_keeps_every_call() {
    let mut calls = PendingCalls::default();
    for unit in 0..MAX_PENDING_CALLS as u64 {
        assert!(calls.pend(unit, format!("rtc_{unit}")), "unit {unit}");
    }
    assert!(
        !calls.pend(u64::MAX, "rtc_late".to_string()),
        "the next arrival is refused"
    );
    assert_eq!(calls.pending.len(), MAX_PENDING_CALLS);
    assert_eq!(
        calls.take(0).as_deref(),
        Some("rtc_0"),
        "the oldest pending call survives"
    );
    assert!(calls.pend(u64::MAX, "rtc_late".to_string()));
    assert!(
        calls.pend(1, "rtc_again".to_string()),
        "an arrival re-asked (a short answer's re-call) keeps its own place"
    );
}

/// Refused and cancelled pending calls release their entries.
#[test]
fn refused_and_cancelled_pending_calls_are_freed() {
    let p = plane();
    assert!(lock(&p.calls).pend(1, "rtc_1".to_string()));
    assert!(lock(&p.calls).pend(2, "rtc_2".to_string()));
    p.end_unit(1);
    assert!(
        !lock(&p.calls).forget(1),
        "the refused unit's call is freed"
    );
    assert!(lock(&p.calls).forget(2), "a cancelled unit's call is freed");
    assert!(lock(&p.calls).pending.is_empty());
}

/// Another caller's offers cannot drop a live call.
#[test]
fn another_callers_offers_never_drop_a_live_call() {
    let mut brokered = Brokered::default();
    assert!(brokered.admits(0));
    brokered.record("rtc_a".to_string(), "tenant-a".to_string(), 0);
    for i in 0..MAX_BROKERED_CALLS {
        assert!(brokered.admits(0), "offer {i} proceeds");
        brokered.record(format!("rtc_b{i}"), "tenant-b".to_string(), 0);
    }
    assert!(
        brokered.owns("rtc_a", "tenant-a", 0),
        "tenant A's live call is kept"
    );
    assert_eq!(
        brokered.per_caller["tenant-b"].len(),
        MAX_CALLS_PER_CALLER,
        "tenant B holds its own allowance"
    );
    let newest = format!("rtc_b{}", MAX_BROKERED_CALLS - 1);
    assert!(
        brokered.owns(&newest, "tenant-b", 0),
        "B's newest call is kept"
    );
    assert!(brokered.admits(0), "the instance still admits");
}

/// A caller at its cap displaces only its own oldest settled row.
#[test]
fn a_caller_at_its_cap_displaces_only_its_own_oldest_row() {
    let mut brokered = Brokered::default();
    brokered.record("rtc_b_first".to_string(), "tenant-b".to_string(), 0);
    for i in 0..MAX_CALLS_PER_CALLER {
        brokered.record(format!("rtc_a{i}"), "tenant-a".to_string(), 0);
    }
    brokered.record("rtc_b_second".to_string(), "tenant-b".to_string(), 0);
    assert!(brokered.admits(0), "the caller at its cap is not refused");
    brokered.record("rtc_a_next".to_string(), "tenant-a".to_string(), 0);
    assert!(
        brokered.owns("rtc_a_next", "tenant-a", 0),
        "the offer proceeds"
    );
    assert!(
        !brokered.owns("rtc_a0", "tenant-a", 0),
        "the caller's own oldest row is gone"
    );
    for i in 1..MAX_CALLS_PER_CALLER {
        assert!(
            brokered.owns(&format!("rtc_a{i}"), "tenant-a", 0),
            "its row {i} stays"
        );
    }
    assert_eq!(brokered.per_caller["tenant-a"].len(), MAX_CALLS_PER_CALLER);
    assert!(
        brokered.owns("rtc_b_first", "tenant-b", 0) && brokered.owns("rtc_b_second", "tenant-b", 0),
        "every other caller's row stays, its oldest included"
    );
    assert_eq!(brokered.owners.len(), MAX_CALLS_PER_CALLER + 2);
}

/// An instance holding [`MAX_BROKERED_CALLS`] rows recorded at the tick clock's reading 0, across
fn full_instance() -> Brokered {
    let mut brokered = Brokered::default();
    for i in 0..MAX_BROKERED_CALLS {
        brokered.record(
            format!("rtc_{i}"),
            format!("caller-{}", i / MAX_CALLS_PER_CALLER),
            0,
        );
    }
    brokered
}

/// The instance refusal clears once its retained rows expire.
#[test]
fn the_instance_429_clears_once_rows_expire() {
    let mut brokered = full_instance();
    assert!(!brokered.admits(0), "a full instance refuses");
    assert_eq!(STATUS_CALLS_FULL, 429);
    let body: serde_json::Value =
        serde_json::from_slice(&refusal_body(STATUS_CALLS_FULL as u16, CALLS_FULL_TEXT))
            .expect("json");
    assert_eq!(body["error"]["type"], "rate_limit_error");
    assert_eq!(body["error"]["message"], CALLS_FULL_TEXT);

    assert!(!brokered.admits(1), "inside the lifetime the rows are live");
    assert!(
        !brokered.admits(REALTIME_CALL_MAX_MS),
        "one millisecond short of the lifetime they are live"
    );
    assert!(
        brokered.admits(1 + REALTIME_CALL_MAX_MS),
        "past the lifetime the 429 clears"
    );
    assert!(brokered.owners.is_empty() && brokered.per_caller.is_empty());
}

/// An expired row frees room for another caller.
#[test]
fn an_expired_row_frees_room_for_another_caller() {
    let mut brokered = full_instance();
    assert!(!brokered.admits(1));
    assert!(brokered.end("rtc_0"), "one call ends");
    brokered.record("rtc_late".to_string(), "caller-late".to_string(), 1);
    assert!(!brokered.admits(1 + REALTIME_CALL_MAX_MS - 1));
    assert!(brokered.admits(1 + REALTIME_CALL_MAX_MS));
    brokered.record(
        "rtc_other".to_string(),
        "tenant-other".to_string(),
        1 + REALTIME_CALL_MAX_MS,
    );
    assert!(brokered.owns("rtc_other", "tenant-other", 1 + REALTIME_CALL_MAX_MS));
    assert!(
        brokered.owns("rtc_late", "caller-late", 1 + REALTIME_CALL_MAX_MS),
        "a row recorded after the clock moved is not expired with the rest"
    );
    assert_eq!(brokered.owners.len(), 2);
}

/// An ended call releases its retained row.
#[test]
fn an_ended_call_frees_its_row() {
    let p = plane();
    for i in 0..2 {
        lock(&p.brokered).record(format!("rtc_{i}"), "tenant-a".to_string(), 0);
        lock(&p.brokered).named(&format!("rtc_{i}"), format!("rtc_up{i}"));
    }
    let call = Some("rtc_up0");
    p.far_end_ended(Door::Sideband, 0, call, true);
    assert!(
        lock(&p.brokered).owns("rtc_0", "tenant-a", 0),
        "not its last piece"
    );
    p.far_end_ended(Door::Sideband, PIECE_LAST | PIECE_CUT, call, true);
    assert!(
        lock(&p.brokered).owns("rtc_0", "tenant-a", 0),
        "a cut is no end"
    );
    p.far_end_ended(Door::Sideband, PIECE_LAST, call, true);
    assert!(
        !lock(&p.brokered).owns("rtc_0", "tenant-a", 0),
        "the ended call's row is freed"
    );
    assert!(
        lock(&p.brokered).owns("rtc_1", "tenant-a", 0),
        "no other call is touched"
    );
    assert_eq!(lock(&p.brokered).per_caller["tenant-a"].len(), 1);
}

/// Refusing a sideband upgrade leaves the live call intact.
#[test]
fn a_refused_sideband_upgrade_frees_no_live_call() {
    let p = plane();
    lock(&p.brokered).record("rtc_0".to_string(), "tenant-a".to_string(), 0);
    lock(&p.brokered).named("rtc_0", "rtc_up0".to_string());
    p.far_end_ended(Door::Sideband, PIECE_LAST, Some("rtc_up0"), false);
    assert!(
        lock(&p.brokered).owns("rtc_0", "tenant-a", 0),
        "the refused upgrade frees nothing"
    );
}

#[test]
fn busbars_call_resolves_for_its_own_caller_once_named() {
    let mut brokered = Brokered::default();
    brokered.record("rtc_ours".to_string(), "tenant-a".to_string(), 0);
    assert!(brokered.holds("rtc_ours"));
    assert_eq!(
        brokered.upstream_of("rtc_ours", "tenant-a", 0),
        None,
        "the provider has not named its call yet"
    );
    brokered.named("rtc_ours", "rtc_provider".to_string());
    assert_eq!(
        brokered.upstream_of("rtc_ours", "tenant-a", 0).as_deref(),
        Some("rtc_provider")
    );
    assert_eq!(
        brokered.upstream_of("rtc_ours", "tenant-b", 0),
        None,
        "another caller's sideband never reaches the call"
    );
    assert!(!brokered.holds("rtc_unknown"));
    assert_eq!(brokered.upstream_of("rtc_unknown", "tenant-a", 0), None);
    brokered.end_upstream("rtc_provider");
    assert!(!brokered.holds("rtc_ours"), "the provider's end frees it");
}

#[test]
fn an_ended_sdp_session_frees_its_call() {
    let p = plane();
    let cfg = StreamsCfg::default();
    lock(&p.brokered).record("rtc_placed".to_string(), "tenant-a".to_string(), 0);
    {
        let mut sessions = lock(&p.sessions);
        let live = sessions
            .get_or_open(9, Door::Sdp, &cfg)
            .expect("the SDP door opens a session");
        live.place("rtc_placed".to_string());
        sessions.close(9);
        p.end_calls(&mut sessions);
    }
    assert!(!lock(&p.brokered).holds("rtc_placed"));
}

/// A far end accepts a session's upgrade with a switch or a success, or, from a transport that
#[test]
fn an_upgrade_is_accepted_by_a_switch_or_a_success_and_by_nothing_else() {
    use busbar_contract::abi::transport::{STATUS_CALLER_FAULT, STATUS_SUCCESS};
    let success = u32::from(STATUS_SUCCESS);
    for code in [101, 200, 204, 299] {
        assert!(upgrade_accepted(PIECE_HAS_STATUS, code, success), "{code}");
    }
    assert!(
        upgrade_accepted(PIECE_HAS_STATUS, 0, success),
        "a class and no number"
    );
    for code in [100, 300, 401, 404, 429, 500, 503] {
        assert!(!upgrade_accepted(PIECE_HAS_STATUS, code, 0), "{code}");
    }
    assert!(!upgrade_accepted(
        PIECE_HAS_STATUS,
        0,
        u32::from(STATUS_CALLER_FAULT)
    ));
    assert!(!upgrade_accepted(0, 101, success), "no status stated");
}

/// A success answering busbar's call that names no provider call is said in one log line (audit
#[test]
fn a_success_naming_no_provider_call_is_said_in_one_log_line() {
    let cap = busbar_test_kit::warn_capture::WarnCapture::default();
    let named = tracing::subscriber::with_default(cap.clone(), || {
        (
            provider_call(None),
            provider_call(Some("/v1/realtime/calls/x")),
            provider_call(Some("/v1/realtime/calls/rtc_ok")),
        )
    });
    assert_eq!(named, (None, None, Some("rtc_ok".to_string())));
    assert_eq!(cap.count("naming no call"), 2, "{:?}", cap.messages());
}
