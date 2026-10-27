// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Tests for `crates/busbar-plane-streaming/src/door.rs`: the P-item pins the door holds (spec
//! DONE item 2, "All P-item behaviours match 1.5.5"), each written against the door as it serves.

use super::*;

/// P-ITEM: WRONG-PROVIDER ATTRIBUTION (spec DONE item 2, "All P-item behaviours match 1.5.5";
/// first fixed red-before-green in busbar e15713578a). A live session is served by the provider
/// its door DIALED: a Gemini Live session's far frames ride the Gemini Live socket's need, at the
/// Gemini Live socket path, its caller arrives in the Gemini Live dialect, and the usage it is
/// metered on is read off the Gemini far end's own report. None of it falls back to the
/// first-declared provider, OpenAI Realtime (its dialect index 0, its socket the first outbound
/// need). Before the fix the paired branch hard-coded the first configured upstream and its lane,
/// so a Gemini session's every turn was billed on the OpenAI lane: the wrong provider's money. An
/// OpenAI Realtime session (the sideband, and the telephony carrier it bridges) still rides OpenAI
/// Realtime's, so the pin does not simply invert the bug.
///
/// The 1.5.5 behaviour this matches is its one surface's (the llm surface; owner correction
/// 2026-09-28): usage is ledgered and metered against the SERVING lane and its provider, never
/// another configured one (v1.5.5 `crates/busbar/src/proxy/usage.rs:57-103`, "`lane` is the
/// SERVING lane (post-failover)"). A bug fixed to match, never a signed diff.
#[test]
fn p_item_wrong_provider_attribution_a_session_rides_the_dialed_provider_not_the_first() {
    use crate::driven::{arrive, GEMINI_SOCKET_PATH, REALTIME_SOCKET_PATH};
    use crate::session_door::PER_SESSION_CLASS;

    let cfg = StreamsCfg::default();
    // (door, the far end's need, its socket path, the dialect the caller arrives in)
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
        let attempt = door
            .route(&cfg.session, None, &[])
            .expect("a session door dials");
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

    // Metered on the serving provider's own report: a Gemini Live usage frame counts on the
    // Gemini session, and counts nothing on an OpenAI Realtime one (whose provider never said it).
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
    gemini.from_far_end(report);
    assert_eq!(
        gemini.units(),
        [(text_in, 10), (text_out, 20), (PER_SESSION_CLASS, 1)],
        "the Gemini session is metered on the Gemini far end's report"
    );
    let mut openai = Live::open(Door::Sideband, &cfg).expect("a session door");
    openai.from_far_end(report);
    assert_eq!(
        openai.units(),
        [(PER_SESSION_CLASS, 1)],
        "an OpenAI Realtime session is never metered on another provider's report"
    );
}

/// P-ITEM: REFUSAL-REASON COLLAPSE (spec DONE item 2, "All P-item behaviours match 1.5.5"; TODO
/// L-ENG9). The streaming plane's refusal table once ended in `_ => "internal"`, so a spent
/// budget, a frozen group, a replayed key, a superseded unit or a deadline told the caller the
/// node had broken. 1.5.5's one surface answered none of its limit reasons as an internal error
/// (v1.5.5 `crates/busbar/src/ingress/mod.rs:237-305`: rate 429 `rate_limit_error`, budget 429,
/// frozen group 403 `permission_error`; none a 500).
///
/// Pinned through the door's own renderer, [`refusal_render`], over every reason a plane can be
/// handed (`reason_of`) at every status the kernel can hand it (its class defaults and a plane's
/// own rows): the reason picks no answer of its own. Every reason of one class renders the same
/// bytes; a refusal reads as the dialect's internal error (`server_error`) exactly when the
/// status the kernel chose for its class is a server status, never because the plane fell through
/// on a reason it did not name; and the message is the kernel's words, except an authentication
/// refusal, which reads in the vendor's own terms. The family is the one classification's
/// (`busbar_contract::abi::plane::RefusalCode::class`), never this plane's own.
#[test]
fn p_item_refusal_reason_collapse_only_the_kernels_class_picks_the_answer_and_none_reads_internal()
{
    const WORDS: &str = "the kernel's words";
    for status in [400u16, 401, 403, 404, 413, 429, 500, 502, 503, 504] {
        let mut answers: Vec<(RefusalClass, Vec<u8>)> = Vec::new();
        for code in RefusalCode::ALL {
            // Two codes are the kernel's own money verdicts and never reach a plane.
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
/// A refused open (a hook's veto, an admission's refusal) is answered in the dialects' error shape:
/// the kernel's status read as the error's type, its words as the message.
#[test]
fn a_refusal_is_rendered_in_the_dialects_error_shape() {
    for (status, kind) in [
        (401, "authentication_error"),
        (403, "permission_error"),
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
/// `both_ways.rs`, whose `an_authentication_refusal_reads_in_the_dialects_vendor_terms` drives the
/// door against these same words).
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
            None,
        )
        .expect("a one-request door"),
        owed: Owed::default(),
        short: None,
    }
}

/// RED-BEFORE-GREEN (THE DESIGN: admission bounds live work and nothing evicts it): a request unit
/// whose op the kernel cancels is forgotten at once, and a table at its bound refuses a new unit
/// rather than drop one in flight. RED: the cancel slot ended only sessions, so a cancelled mint
/// or SDP unit held its state until 4096 newer units pushed it out, and past the bound the oldest
/// unit was evicted live, its gathered far-end body lost.
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
        sessions: Mutex::new(Sessions::default()),
        wake: None,
        driver: Mutex::new((Ticket::NONE, 0)),
    }
}

/// RED-BEFORE-GREEN (the contract's `TAIL_REFUSED_PIECE`, `busbar_contract::abi::plane`): a request
/// unit's hop holds state from its first piece, so the tail asks for a unit's refusal as a piece
/// first. RED: the tail stated no flags, so a unit the walk refused after its first piece (its
/// member's credential gone, or no member left) was never told to the door, and its state stayed.
#[test]
fn the_tail_takes_a_units_refusal_as_a_piece_first() {
    assert_eq!(TAIL.flags & TAIL_REFUSED_PIECE, TAIL_REFUSED_PIECE);
}

/// RED-BEFORE-GREEN (units leak): a refused unit, whether told by the refused piece or by
/// `refusal`, is forgotten with its pending sideband call, and its room is free. RED: nothing on
/// the refusal path forgot a held unit, so each refused mint or SDP unit kept a slot for the
/// instance's life, and the 4096th refused every caller of every tenant.
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

/// RED-BEFORE-GREEN (units leak): the kernel's own cancel of a unit names it by its key
/// (`PlaneCancelIn::unit`), for a unit whose ticket no piece may ever have carried; that unit is
/// forgotten. RED: the cancel found units only by ticket, so a unit cancelled by key kept its slot.
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

/// GREEN (THE DESIGN: admission bounds live work, nothing evicts it): 4096 live units still refuse
/// the next unit, and none of them is dropped.
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

/// RED-BEFORE-GREEN (`BUSBAR-1.6.0.md`, "Admission bounds live work; nothing evicts it"): at
/// [`MAX_PENDING_CALLS`] the next sideband arrival is refused, and every pending call survives.
/// RED: the oldest pending call was evicted to make room, so a flood of arrivals (none of which
/// need open a session) dropped real callers' calls, and their sessions were refused.
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

/// RED-BEFORE-GREEN: a pending call is freed when its unit is refused or cancelled. RED: entries
/// were freed only when a session opened, so refused and cancelled arrivals held the table.
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

/// RED-BEFORE-GREEN (`BUSBAR-1.6.0.md` l.199: "An active work handle is never evicted ... Retention
/// bounds only settled handles"): 4096 SDP offers from tenant B never drop tenant A's live call; B
/// keeps at most its own allowance, its newest calls. RED: the brokered list dropped its oldest
/// entry past 4096, so B's offers pushed A's live call out, and A's sideband to it was refused.
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

/// RED-BEFORE-GREEN (lead ruling: no permanent refusal): a caller holding
/// [`MAX_CALLS_PER_CALLER`] live calls is not refused; its next offer proceeds and displaces that
/// caller's own oldest row, and no other row. RED: a caller had no bound of its own, so its
/// newest call was kept beside all the others and its oldest row stayed (and past 4096 rows the
/// oldest row of the whole list went, whichever caller's it was).
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
/// as many callers as hold them at their cap.
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

/// RED-BEFORE-GREEN (lead ruling: only settled handles are bounded): an instance full of unexpired
/// rows answers a new SDP offer [`STATUS_CALLS_FULL`] in the dialects' error shape, and the 429
/// clears once the rows are older than [`REALTIME_CALL_MAX_MS`]: their calls have certainly
/// ended. A row's age counts from the first tick after it was recorded, so a clock that had not
/// moved since never ages it early. RED: a row never expired, so the 429 stood for as long as the
/// instance lived.
#[test]
fn the_instance_429_clears_once_rows_expire() {
    let mut brokered = full_instance();
    assert!(!brokered.admits(0), "a full instance refuses");
    let reply = calls_full_reply();
    assert_eq!(u32::from(reply.status), STATUS_CALLS_FULL);
    assert_eq!(reply.rtc_call_id, None, "a refused offer brokers no call");
    let body: serde_json::Value =
        serde_json::from_slice(&crate::tests::body_bytes(reply.body)).expect("json");
    assert_eq!(body["error"]["type"], "rate_limit_error");
    assert_eq!(body["error"]["message"], CALLS_FULL_TEXT);
    // The clock moves past the reading the rows were recorded at: their age counts from here.
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

/// RED-BEFORE-GREEN (lead ruling): an expired row frees room for another caller, and only the
/// expired rows go: a call recorded once the clock had moved is still live. RED: a row never
/// expired, so the instance's room never came back.
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

/// RED-BEFORE-GREEN: a brokered call the far end ends, as its sideband sees it (an accepted
/// upgrade whose answer ends whole: its last piece, not cut), has ended, and its row is freed. A
/// far end whose connection was cut, or a piece that is not its last, frees nothing. RED: the row
/// was freed only when the sideband itself closed, never on the far end's end.
#[test]
fn an_ended_call_frees_its_row() {
    let p = plane();
    for i in 0..2 {
        lock(&p.brokered).record(format!("rtc_{i}"), "tenant-a".to_string(), 0);
    }
    let call = Some("rtc_0");
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

/// RED-BEFORE-GREEN (review round 4): a sideband whose upgrade the far end refused (a status that
/// is no switch and no success, on its last piece, not cut) never reached the call, so it says
/// nothing of the call's end: the live call's row stays. RED: any whole last far-end piece freed
/// the row, so one refused upgrade dropped a live call, and its caller's next sideband was refused.
#[test]
fn a_refused_sideband_upgrade_frees_no_live_call() {
    let p = plane();
    lock(&p.brokered).record("rtc_0".to_string(), "tenant-a".to_string(), 0);
    p.far_end_ended(Door::Sideband, PIECE_LAST, Some("rtc_0"), false);
    assert!(
        lock(&p.brokered).owns("rtc_0", "tenant-a", 0),
        "the refused upgrade frees nothing"
    );
}
