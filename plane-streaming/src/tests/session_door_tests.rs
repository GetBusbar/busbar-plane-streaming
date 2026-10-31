// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Tests for `crates/busbar-plane-streaming/src/session_door.rs`: a live session on the door,
//! one frame per answer, each side's frames in order, the session's end answered on the caller's
//! side only, and the ceiling read on the tick clock.

use std::num::NonZeroU32;

use super::*;

fn json(v: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&v).expect("json")
}

fn text(f: &[u8]) -> String {
    String::from_utf8_lossy(f).into_owned()
}

/// `ms` milliseconds of 24 kHz PCM16 uplink audio (48 bytes a millisecond).
fn uplink(ms: usize) -> Vec<u8> {
    json(serde_json::json!({"type":"input_audio_buffer.append",
        "audio": busbar_contract::media::base64_encode(&vec![0u8; ms * 48])}))
}

fn usage_done() -> Vec<u8> {
    json(
        serde_json::json!({"type":"response.done","response":{"usage":{
        "input_token_details":{"audio_tokens":10,"text_tokens":3},
        "output_token_details":{"audio_tokens":20,"text_tokens":4}}}}),
    )
}

fn audio_delta(item: &str) -> Vec<u8> {
    json(serde_json::json!({"type":"response.output_audio.delta",
        "delta": busbar_contract::media::base64_encode(&[0u8; 96]),
        "item_id": item, "output_index":0, "content_index":0}))
}

fn sideband() -> Live {
    Live::open(Door::Sideband, &StreamsCfg::default()).expect("a session door")
}

/// The class index of audio seconds in, in the tail's order.
fn seconds() -> u32 {
    crate::session_unit::CLASSES
        .iter()
        .position(|c| *c == crate::meta::CLASS_AUDIO_SECONDS_IN)
        .expect("declared") as u32
}

#[test]
fn a_one_request_door_opens_no_session() {
    assert!(Live::open(Door::Mint, &StreamsCfg::default()).is_none());
}

/// The SDP door's session (row 132) owes the far end busbar's own call before anything else, and
/// a session told to end ends once what it owes is collected.
#[test]
fn the_sdp_sessions_call_is_owed_the_far_end_first() {
    let mut s = Live::open(Door::Sdp, &StreamsCfg::default()).expect("a session door");
    assert!(!s.ready(), "nothing owed at the open");
    s.open_far(b"--form".to_vec());
    assert!(s.ready());
    assert_eq!(
        s.next(true),
        Emit {
            frame: Some((Side::FarEnd, b"--form".to_vec())),
            done: false
        }
    );
    s.end();
    assert_eq!(
        s.next(true),
        Emit {
            frame: None,
            done: true
        }
    );
}

#[test]
fn a_caller_frame_is_one_turn_toward_the_far_end() {
    let mut s = sideband();
    s.from_caller(&uplink(1000), false);
    let emit = s.next(true);
    let Some((Side::FarEnd, frame)) = emit.frame else {
        panic!("a frame toward the far end: {emit:?}");
    };
    assert!(text(&frame).contains("input_audio_buffer.append"));
    assert!(!emit.done);
    assert_eq!(
        s.next(true),
        Emit::default(),
        "one frame, nothing more owed"
    );
    assert!(!s.ready());
}

#[test]
fn the_far_ends_usage_reaches_the_caller_and_the_sessions_units() {
    let mut s = sideband();
    s.from_caller(&uplink(2000), false);
    let _ = s.next(true);
    s.accept();
    s.from_far_end(&usage_done());
    assert_eq!(
        s.units(),
        vec![
            (0, 10),
            (1, 20),
            (2, 3),
            (3, 4),
            (seconds(), 2),
            (PER_SESSION_CLASS, 1)
        ],
        "the closed turn's tokens, its admitted audio, and the fee the answered session incurred"
    );
    let emit = s.next(false);
    assert!(
        matches!(&emit.frame, Some((Side::Caller, f)) if text(f).contains("response.done"))
            || emit.frame.is_none(),
        "{emit:?}"
    );
}

/// A barge-in is the caller's to act on (Law 11): the far-side answer carries the event to the
/// caller as the far end sent it, and nothing is owed the far end.
#[test]
fn a_far_side_answer_carries_only_a_frame_for_the_caller() {
    let mut s = sideband();
    s.from_far_end(&audio_delta("it1"));
    while s.next(false).frame.is_some() {}
    let barge = json(
        serde_json::json!({"type":"input_audio_buffer.speech_started",
        "audio_start_ms":420,"item_id":"it1"}),
    );
    s.from_far_end(&barge);
    let far = s.next(false);
    assert!(
        matches!(&far.frame, Some((Side::Caller, f)) if *f == barge),
        "{far:?}"
    );
    assert_eq!(s.next(false), Emit::default(), "no caller frame left");
    assert!(!s.ready(), "nothing is owed the far end");
}

/// The caller's last piece settles the open turn once: the audio it admitted is in the session's
/// units (as it was, running, in every answer before), and only the caller side answers the end.
#[test]
fn the_callers_last_piece_settles_the_open_turn_and_ends_the_session() {
    let mut s = sideband();
    s.from_caller(&uplink(3000), false);
    let _ = s.next(true);
    assert_eq!(s.units(), vec![(seconds(), 3)], "the open turn, running");
    s.from_caller(&[], true);
    assert_eq!(s.units(), vec![(seconds(), 3)], "settled once, not twice");
    assert!(
        !s.next(false).done,
        "a far-side answer never ends the session"
    );
    assert!(s.next(true).done);
}

#[test]
fn the_telephony_door_reads_the_carriers_envelope() {
    let mut s = Live::open(Door::Twilio, &StreamsCfg::default()).expect("a session door");
    s.from_caller(
        &json(
            serde_json::json!({"event":"start","start":{"streamSid":"MZ1","callSid":"CA1",
            "mediaFormat":{"encoding":"audio/x-mulaw","sampleRate":8000,"channels":1}}}),
        ),
        false,
    );
    let told = s.next(true);
    assert!(
        matches!(&told.frame, Some((Side::FarEnd, f)) if text(f).contains("session.update")),
        "{told:?}"
    );
    assert_eq!(s.next(true), Emit::default());
    s.from_caller(
        &json(serde_json::json!({"event":"media","streamSid":"MZ1",
            "media":{"payload": busbar_contract::media::base64_encode(&[0x7f; 160])}})),
        false,
    );
    let emit = s.next(true);
    assert!(
        matches!(&emit.frame, Some((Side::FarEnd, f)) if text(f).contains("input_audio_buffer.append")),
        "{emit:?}"
    );
    s.from_caller(&json(serde_json::json!({"event":"stop"})), false);
    assert!(s.next(true).done, "a stop ends the call");
}

/// The ceiling is read on the tick clock: the session's clock starts at its first tick, and a
/// session past its ceiling tells the caller why and then ends.
#[test]
fn a_session_past_its_ceiling_is_told_why_then_ends() {
    let cfg = StreamsCfg {
        session_max_secs: NonZeroU32::new(5),
        ..StreamsCfg::default()
    };
    let mut s = Live::open(Door::Sideband, &cfg).expect("a session door");
    assert!(!s.tick(1_000), "the clock starts");
    assert!(!s.tick(1_000 + 4 * NS_PER_SEC), "inside its ceiling");
    assert!(s.tick(1_000 + 5 * NS_PER_SEC), "at its ceiling");
    let told = s.next(true);
    assert!(
        matches!(&told.frame, Some((Side::Caller, f)) if text(f).contains("session_max_secs")),
        "{told:?}"
    );
    assert!(s.next(true).done);
    assert!(
        !s.tick(1_000 + 9 * NS_PER_SEC),
        "an ended session is told once"
    );
}

#[test]
fn a_session_with_no_ceiling_runs_on() {
    let mut s = sideband();
    assert!(!s.tick(0));
    assert!(!s.tick(u64::MAX / 2));
    assert!(!s.ready());
}

#[test]
fn a_cancel_on_either_sides_ticket_ends_the_session() {
    let t = |slot| Ticket {
        slot,
        generation: 1,
    };
    let mut all = Sessions::default();
    let cfg = StreamsCfg::default();
    for stream in [7, 8] {
        assert!(all.get_or_open(stream, Door::Sideband, &cfg).is_some());
    }
    all.crossed(7, t(1));
    all.crossed(7, t(2));
    all.crossed(8, t(3));
    assert!(!all.cancel(t(9)), "a ticket no session crossed on");
    assert!(all.cancel(t(2)), "the far side's ticket");
    assert!(all.get(7).is_none());
    assert!(!all.cancel(t(1)), "its other ticket is forgotten with it");
    assert_eq!(all.len(), 1);
    assert!(all.get_or_open(9, Door::Mint, &cfg).is_none());
}

#[test]
fn drive_names_the_sessions_that_owe_output() {
    let mut all = Sessions::default();
    let cfg = StreamsCfg::default();
    for stream in [3, 4, 5] {
        let _ = all.get_or_open(stream, Door::Sideband, &cfg);
    }
    assert!(all.ready(8).is_empty());
    all.get(4).expect("open").from_caller(&[], true);
    all.get(5).expect("open").from_caller(&uplink(10), false);
    assert_eq!(all.ready(8), vec![4, 5]);
    assert_eq!(all.ready(1), vec![4]);
    assert_eq!(all.ready_count(), 2);
}

/// THE SESSION'S FEE UNIT (ARCHITECT Q-L5-FEE (A); Q17-6): `per_session` is reported `1` once the
/// far end accepts the session, never before, and never for a far-end piece alone (a refused
/// upgrade's body; audit S1). A session whose far end never answered (its
/// open failed) reports no fee unit however much its caller sent, so the kernel refunds the fee.
#[test]
fn the_fee_unit_is_incurred_when_the_far_end_first_answers() {
    let mut s = sideband();
    s.from_caller(&uplink(1000), false);
    let _ = s.next(true);
    assert!(
        !s.units().iter().any(|(c, _)| *c == PER_SESSION_CLASS),
        "no far answer, no fee unit: {:?}",
        s.units()
    );
    s.from_caller(&[], true);
    assert!(
        !s.units().iter().any(|(c, _)| *c == PER_SESSION_CLASS),
        "an open that never reached the far end ends with no fee unit"
    );
    let mut s = sideband();
    s.from_far_end(&audio_delta("it1"));
    assert!(
        !s.units().iter().any(|(c, _)| *c == PER_SESSION_CLASS),
        "a far-end piece is not the far end accepting the session: {:?}",
        s.units()
    );
    s.accept();
    assert_eq!(
        s.units()
            .iter()
            .filter(|(c, _)| *c == PER_SESSION_CLASS)
            .collect::<Vec<_>>(),
        vec![&(PER_SESSION_CLASS, 1)],
        "the far end's acceptance incurs it once"
    );
    s.from_far_end(&audio_delta("it2"));
    assert!(s.units().contains(&(PER_SESSION_CLASS, 1)), "still one");
}

/// THE SESSION-OPEN REWRITE (ARCHITECT Q-L5B-PROJECT; the retired streams crate's projection,
/// verbatim): a hook's rewrite is a PATCH over the locked params (a key it names wins, a key it
/// does not keep the plane's value, `null` clears); output that is not JSON, not an object, or not
/// a session config once merged is refused, never opened as if no rewrite had been made.
#[test]
fn a_session_open_rewrite_patches_the_locked_params_or_refuses() {
    let locked = SessionConfig {
        instructions: Some("be brief".to_string()),
        voice: Some("alloy".to_string()),
        ..SessionConfig::default()
    };
    let patched =
        committed_session_config(&locked, br#"{"voice":"marin"}"#).expect("a readable patch");
    assert_eq!(
        patched.voice.as_deref(),
        Some("marin"),
        "the named key wins"
    );
    assert_eq!(
        patched.instructions.as_deref(),
        Some("be brief"),
        "an unnamed key keeps the locked value"
    );
    let cleared = committed_session_config(&locked, br#"{"instructions":null}"#).expect("clear");
    assert_eq!(cleared.instructions, None, "naming null clears");
    for bad in [&b"not json"[..], b"7", br#"{"voice":7}"#] {
        assert!(
            committed_session_config(&locked, bad).is_err(),
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
}

/// A session opened after its open's hooks rewrote the params opens under the rewritten params,
/// once; a stream with none opens under what its door locks.
#[test]
fn a_rewritten_session_open_locks_the_rewritten_params_once() {
    let cfg = StreamsCfg::default();
    assert_eq!(Live::locked(Door::Sideband, &cfg), cfg.session);
    // The telephony lock is the section's session with µ-law both ways, and nothing else.
    let telephony = Live::locked(Door::Twilio, &cfg);
    assert_eq!(telephony.input_audio_format, Some(AudioFormat::G711Ulaw));
    assert_eq!(telephony.output_audio_format, Some(AudioFormat::G711Ulaw));
    assert_eq!(
        SessionConfig {
            input_audio_format: cfg.session.input_audio_format,
            output_audio_format: cfg.session.output_audio_format,
            ..telephony
        },
        cfg.session
    );
    let mut sessions = Sessions::default();
    let rewritten = SessionConfig {
        voice: Some("marin".to_string()),
        ..cfg.session.clone()
    };
    sessions.rewrite(7, rewritten);
    assert!(sessions.get_or_open(7, Door::Sideband, &cfg).is_some());
    sessions.close(7);
    assert!(
        sessions.rewritten.is_empty(),
        "consumed at the open and gone with the session"
    );
}

/// RED-BEFORE-GREEN (Part 2 #45, Twilio Media Streams "with a µ-law codec"; the telephony lock):
/// the telephony door's far end is told the session it is locked to once the carrier states its
/// stream: µ-law both ways, over the operator's own session (here its instructions). RED: nothing
/// was ever sent, so the far end read the carrier's µ-law as its default PCM16.
#[test]
fn the_telephony_door_tells_the_far_end_its_locked_session_at_the_carriers_start() {
    let cfg = StreamsCfg {
        session: crate::codec::ir::config::SessionConfig {
            instructions: Some("be brief".into()),
            ..StreamsCfg::default().session
        },
        ..StreamsCfg::default()
    };
    let mut s = Live::open(Door::Twilio, &cfg).expect("a session door");
    s.from_caller(
        &json(
            serde_json::json!({"event":"start","start":{"streamSid":"MZ1","callSid":"CA1",
            "mediaFormat":{"encoding":"audio/x-mulaw","sampleRate":8000,"channels":1}}}),
        ),
        false,
    );
    let emit = s.next(true);
    let Some((Side::FarEnd, frame)) = emit.frame else {
        panic!("the locked session goes to the far end: {emit:?}");
    };
    let v: serde_json::Value = serde_json::from_slice(&frame).expect("json");
    assert_eq!(v["type"], "session.update", "{v}");
    assert_eq!(v["session"]["type"], "realtime", "{v}");
    assert_eq!(
        v["session"]["audio"]["input"]["format"]["type"], "audio/pcmu",
        "{v}"
    );
    assert_eq!(
        v["session"]["audio"]["output"]["format"]["type"], "audio/pcmu",
        "{v}"
    );
    assert_eq!(v["session"]["instructions"], "be brief", "{v}");
}

/// RED-BEFORE-GREEN (the GA `/v1/realtime` socket the doors dial with no `OpenAI-Beta` header;
/// OpenAI's beta-to-GA migration: "set `session.type`, move output audio configuration under
/// `session.audio.output`"): the telephony lock is told in the GA `session` shape, with no
/// pre-GA top-level key. RED: the lock was `{input_audio_format: g711_ulaw, ...}` with no
/// `session.type`, which the GA socket does not read, so the far end read µ-law as 24 kHz PCM.
#[test]
fn the_telephony_lock_is_told_in_the_ga_session_shape() {
    let mut s = Live::open(Door::Twilio, &StreamsCfg::default()).expect("a session door");
    s.from_caller(
        &json(
            serde_json::json!({"event":"start","start":{"streamSid":"MZ1","callSid":"CA1",
            "mediaFormat":{"encoding":"audio/x-mulaw","sampleRate":8000,"channels":1}}}),
        ),
        false,
    );
    let Some((Side::FarEnd, frame)) = s.next(true).frame else {
        panic!("the locked session goes to the far end");
    };
    let v: serde_json::Value = serde_json::from_slice(&frame).expect("json");
    let session = v["session"].as_object().expect("a session object");
    for beta in [
        "input_audio_format",
        "output_audio_format",
        "turn_detection",
        "modalities",
        "voice",
    ] {
        assert!(!session.contains_key(beta), "no pre-GA `{beta}`: {v}");
    }
    assert_eq!(v["session"]["type"], "realtime", "{v}");
    assert_eq!(
        v["session"]["audio"]["input"]["turn_detection"]["type"], "server_vad",
        "the section's turn detection, under audio.input: {v}"
    );
}

/// The class index of tool calls, in the tail's order.
fn tool_calls() -> u32 {
    crate::session_unit::CLASSES
        .iter()
        .position(|c| *c == crate::meta::CLASS_TOOL_CALLS)
        .expect("declared") as u32
}

/// RED-BEFORE-GREEN (MONEY, THE DESIGN section 7: "`cut-stream` cuts it, and a cut bills what
/// streamed"; units are reported "as they happen"): the kernel bills a session from the last units
/// it reported, however it ends, and a cancel (a `cut-stream` budget cut, a drain) reports none of
/// its own. So every answer carries the OPEN turn's running counts. A caller that streamed 2.5 s
/// while the far end opened one tool call, cut before any usage report, bills 3 `audio_seconds_in`
/// and 1 `tool_calls` with its fee. RED: the answers reported closed turns only, the last
/// checkpoint before the cancel was the fee alone, and the 2.5 s and the call were never billed.
#[test]
fn a_session_the_kernel_cancels_mid_turn_has_already_reported_the_open_turn() {
    let t = Ticket {
        slot: 1,
        generation: 1,
    };
    let mut all = Sessions::default();
    let cfg = StreamsCfg::default();
    let live = all.get_or_open(7, Door::Sideband, &cfg).expect("open");
    live.from_caller(&uplink(2500), false);
    live.accept();
    live.from_far_end(&json(
        serde_json::json!({"type":"response.output_item.added",
            "item":{"type":"function_call","call_id":"c1","name":"weather"}}),
    ));
    let last_checkpoint = live.units();
    all.crossed(7, t);
    assert!(all.cancel(t), "the kernel's cancel ends the session");
    assert_eq!(
        last_checkpoint,
        vec![(seconds(), 3), (tool_calls(), 1), (PER_SESSION_CLASS, 1)],
        "the last answer before the cancel bills what streamed"
    );
}

/// The open turn's running counts never fall when the turn closes: a usage report moves them into
/// the closed total, so the kernel's checkpoint is monotone across the close.
#[test]
fn a_turns_close_moves_its_running_counts_and_the_total_never_falls() {
    let mut s = sideband();
    s.from_caller(&uplink(1500), false);
    let running = s.units();
    assert_eq!(running, vec![(seconds(), 2)]);
    s.from_far_end(&usage_done());
    let closed = s.units();
    assert!(closed.contains(&(seconds(), 2)), "{closed:?}");
    s.from_caller(&uplink(400), false);
    assert!(
        s.units().contains(&(seconds(), 2)),
        "1 900 ms is still 2 s: {:?}",
        s.units()
    );
    s.from_caller(&uplink(200), false);
    assert!(s.units().contains(&(seconds(), 3)), "{:?}", s.units());
}

/// RED-BEFORE-GREEN (`streams.max_output_tokens` and `streams.context_window_tokens`, the section's
/// two token ceilings): each is stated to the far end in the dialect's own key when the session is
/// locked, the output cap lowered to the session's own where that is lower. RED: both keys were
/// parsed, defaulted and never read, so an operator's cap was no cap.
#[test]
fn the_sections_token_ceilings_reach_the_far_end() {
    let cfg = StreamsCfg {
        max_output_tokens: 1000,
        context_window_tokens: 16_384,
        ..StreamsCfg::default()
    };
    let mut s = Live::open(Door::Twilio, &cfg).expect("a session door");
    s.from_caller(
        &json(
            serde_json::json!({"event":"start","start":{"streamSid":"MZ1","callSid":"CA1",
            "mediaFormat":{"encoding":"audio/x-mulaw","sampleRate":8000,"channels":1}}}),
        ),
        false,
    );
    let Some((Side::FarEnd, frame)) = s.next(true).frame else {
        panic!("the locked session goes to the far end");
    };
    let v: serde_json::Value = serde_json::from_slice(&frame).expect("json");
    assert_eq!(v["session"]["max_output_tokens"], 1000, "{v}");
    assert_eq!(v["session"]["truncation"]["type"], "retention_ratio", "{v}");
    assert_eq!(
        v["session"]["truncation"]["token_limits"]["post_instructions"],
        16_384 - 1000,
        "the context left once the response's own output is room: {v}"
    );

    let lower = StreamsCfg {
        session: crate::codec::ir::config::SessionConfig {
            max_output_tokens: Some(crate::codec::ir::config::MaxOutputTokens::Limit(200)),
            ..cfg.session.clone()
        },
        ..cfg.clone()
    };
    let mut g = Live::open(Door::Gemini, &lower).expect("a session door");
    g.from_caller(
        &json(serde_json::json!({"setup":{"model":"models/gemini-live",
            "generationConfig":{"temperature":0.3}}})),
        false,
    );
    let Some((Side::FarEnd, frame)) = g.next(true).frame else {
        panic!("the caller's setup goes to the far end");
    };
    let v: serde_json::Value = serde_json::from_slice(&frame).expect("json");
    assert_eq!(
        v["setup"]["generationConfig"]["maxOutputTokens"], 200,
        "the session's own lower cap stands: {v}"
    );
    assert_eq!(v["setup"]["generationConfig"]["temperature"], 0.3, "{v}");
    assert_eq!(
        v["setup"]["contextWindowCompression"]["triggerTokens"], 16_384,
        "{v}"
    );
}

/// RED-BEFORE-GREEN (lead ruling: a caller's own `max_output_tokens` never bypasses the configured
/// output cap), through a live session: a `response.create` reaches the far end asking for the
/// lower of the caller's value and `streams.max_output_tokens`, and for the cap where it names
/// none. RED: the caller's 5000 went out as written, past the section's 1000.
#[test]
fn a_callers_response_create_cannot_lift_the_output_ceiling() {
    let cfg = StreamsCfg {
        max_output_tokens: 1000,
        ..StreamsCfg::default()
    };
    for (asked, sent) in [(Some(5000), 1000), (Some(200), 200), (None, 1000)] {
        let mut s = Live::open(Door::Sideband, &cfg).expect("a session door");
        let response = match asked {
            Some(n) => serde_json::json!({"max_output_tokens": n}),
            None => serde_json::json!({}),
        };
        s.from_caller(
            &json(serde_json::json!({"type":"response.create","response":response})),
            false,
        );
        let Some((Side::FarEnd, frame)) = s.next(true).frame else {
            panic!("the response request goes to the far end");
        };
        let v: serde_json::Value = serde_json::from_slice(&frame).expect("json");
        assert_eq!(v["response"]["max_output_tokens"], sent, "{asked:?}: {v}");
    }
}

/// RED on dev: a stream whose session ended (the caller's `stop`, a cancel) is never opened again.
/// The caller's next piece on it (a second Twilio `stop`) is refused rather than answered by a
/// fresh session that ends, reports `done` and settles a second time.
#[test]
fn an_ended_stream_is_never_opened_again() {
    let t = Ticket {
        slot: 1,
        generation: 1,
    };
    let mut all = Sessions::default();
    let cfg = StreamsCfg::default();
    assert!(all.get_or_open(7, Door::Sideband, &cfg).is_some());
    all.close(7);
    assert!(all.get(7).is_none());
    assert!(
        all.get_or_open(7, Door::Sideband, &cfg).is_none(),
        "an ended call"
    );
    assert!(all.served(7), "its refusal is still the session's");
    assert!(all.get_or_open(8, Door::Sideband, &cfg).is_some());
    all.crossed(8, t);
    assert!(all.cancel(t));
    assert!(
        all.get_or_open(8, Door::Sideband, &cfg).is_none(),
        "a cancelled call"
    );
    assert_eq!(all.len(), 0);
}

/// RED-BEFORE-GREEN (busbar row 159; ARCHITECT ruling 2026-10-23 on `BUSBAR-1.6.0.md` Part 2 #45:
/// OpenAI's own WS surface takes the model on the socket's query): a plain session (`GET
/// /v1/realtime`) and a telephony session dial the provider's realtime socket naming the session's
/// model (`?model=`), escaped so it carries no other parameter; a sideband names only its call
/// (`?call_id=`), and the Gemini Live socket names none. RED: every OpenAI Realtime session that was
/// attached to no call dialled the bare socket, which the provider refuses without a model.
#[test]
fn a_plain_or_telephony_session_dials_the_socket_naming_the_sessions_model() {
    let named = |model: &str| StreamsCfg {
        session: SessionConfig {
            model: Some(model.to_string()),
            ..SessionConfig::default()
        },
        ..StreamsCfg::default()
    };
    let cfg = named("gpt-realtime");
    for door in [Door::Realtime, Door::Twilio] {
        let live = Live::open(door, &cfg).expect("a session door");
        assert_eq!(
            live.dial_target("/v1/realtime"),
            "/v1/realtime?model=gpt-realtime",
            "{door:?} dials the socket naming the session's model"
        );
    }
    let mut attached = Live::open(Door::Sideband, &cfg).expect("a session door");
    attached.attach("rtc_abc".to_string());
    assert_eq!(
        attached.dial_target("/v1/realtime"),
        "/v1/realtime?call_id=rtc_abc",
        "a sideband names only the call it attaches to"
    );
    let gemini = Live::open(Door::Gemini, &cfg).expect("a session door");
    assert_eq!(
        gemini.dial_target(crate::driven::GEMINI_SOCKET_PATH),
        crate::driven::GEMINI_SOCKET_PATH,
        "the Gemini Live socket names no model on its path"
    );
    let odd = Live::open(Door::Realtime, &named("a b&call_id=x")).expect("a session door");
    assert_eq!(
        odd.dial_target("/v1/realtime"),
        "/v1/realtime?model=a+b%26call_id%3Dx",
        "the model is escaped: it carries no second parameter"
    );
}
