// SPDX-License-Identifier: Apache-2.0
//! Streaming lifecycle and governance regression cases through both plugin origins.
#[path = "support/voice_abi.rs"]
pub mod voice_abi;
use busbar_contract::abi::mechanism::call::Outcome;
use busbar_contract::abi::plane::{
    FROM_CALLER, FROM_FAR_END, FROM_KERNEL, PIECE_CUT, PIECE_HAS_STATUS, PIECE_LAST,
};
use busbar_plugin_loader::dispatch::{DispatchConfig, Dispatcher};
use serde_json::{json, Value};
use voice_abi::*;
const USAGE: &[u8] = br#"{"type":"response.done","response":{"usage":{"input_token_details":{"audio_tokens":10,"text_tokens":3},"output_token_details":{"audio_tokens":20,"text_tokens":4}}}}"#;

#[test]
fn observation_v1_barge_in_preemption() {
    for dropped in [false, true] {
        let d = Dispatcher::new(
            DispatchConfig::default(),
            busbar_contract::services::test_caller_binding(),
        );
        let p = load(&d, "observe-barge", dropped);
        let opened = open(&p, SESSION, Some(PUBLIC));
        let initial = piece(&p, &session(100, 5, FROM_CALLER, 0, &[]));
        let audio = serde_json::to_vec(&json!({"type":"response.output_audio.delta","delta":busbar_contract::media::base64_encode(&[0;96])})).unwrap();
        let speech = serde_json::to_vec(
            &json!({"type":"input_audio_buffer.speech_started","audio_start_ms":0,"item_id":"it1"}),
        )
        .unwrap();
        let mut frames = Vec::new();
        let mut ready = opened.outcome == Outcome::Ready && initial.outcome == Outcome::Ready;
        for (index, bytes) in [&audio, &speech].into_iter().enumerate() {
            let a = piece(
                &p,
                &Piece {
                    status: if index == 0 { 101 } else { 0 },
                    ..session(
                        100,
                        5,
                        FROM_FAR_END,
                        if index == 0 { PIECE_HAS_STATUS } else { 0 },
                        bytes,
                    )
                },
            );
            ready &= a.outcome == Outcome::Ready && a.more == 0;
            if !a.emitted.is_empty() {
                frames.push((a.to_far, a.emitted));
            }
            for _ in 0..64 {
                let a = piece(&p, &session(100, 5, FROM_KERNEL, 0, &[]));
                ready &= a.outcome == Outcome::Ready && a.more == 0;
                let quiet = (a.emitted.is_empty() && !a.done) || a.done;
                if !a.emitted.is_empty() {
                    frames.push((a.to_far, a.emitted));
                }
                if quiet {
                    break;
                }
            }
        }
        let matched = ready && frames == [(false, audio), (false, speech)];
        assert!(matched,
            "OBSERVATION V1-barge-in-preemption dropped={dropped} matched={matched}: caller receives audio/speech_started; gateway authors no cancel; frames={frames:?}"
        );
    }
}

#[test]
fn observation_v2_turn_budget_enforcement() {
    const CEILING_TICK_NS: u64 = 1_000_000_000;
    const SESSION_CEILING_REASON: &str = "session_expired";
    for dropped in [false, true] {
        let d = Dispatcher::new(
            DispatchConfig::default(),
            busbar_contract::services::test_caller_binding(),
        );
        let p = load(&d, "observe-ceiling", dropped);
        let opened = open(
            &p,
            br#"{"session":{"model":"m-cap"},"session_max_secs":1}"#,
            Some(PUBLIC),
        );
        let initial = piece(&p, &session(110, 5, FROM_CALLER, 0, &[]));
        let first = tick(&p, 1_000);
        let second = tick(&p, 1_000 + 1_000_000_000);
        let owed = drive(&p);
        let mut frames = Vec::new();
        let mut done = 0;
        let mut ready = opened.outcome == Outcome::Ready && initial.outcome == Outcome::Ready;
        for _ in 0..64 {
            let a = piece(&p, &session(110, 5, FROM_KERNEL, 0, &[]));
            ready &= a.outcome == Outcome::Ready && a.more == 0;
            done += u32::from(a.done);
            let quiet = (a.emitted.is_empty() && !a.done) || a.done;
            if !a.emitted.is_empty() {
                frames.push((a.to_far, serde_json::from_slice::<Value>(&a.emitted).ok()));
            }
            if quiet {
                break;
            }
        }
        let said = matches!(frames.as_slice(),[(false,Some(v))] if v["error"]["code"] == SESSION_CEILING_REASON);
        let matched = ready
            && first == (Outcome::Ready, 1_000 + CEILING_TICK_NS)
            && owed == (Outcome::Ready, vec![110])
            && done == 1
            && said;
        assert!(matched,
            "OBSERVATION V2-turn-budget-enforcement dropped={dropped} matched={matched}: first={first:?} second={second:?} owed={owed:?} ends={done} frames={frames:?}"
        );
    }
}

#[test]
fn observation_v3_metering_lease_settled() {
    for dropped in [false, true] {
        let d = Dispatcher::new(
            DispatchConfig::default(),
            busbar_contract::services::test_caller_binding(),
        );
        let p = load(&d, "observe-settle", dropped);
        let _ = open(&p, SESSION, Some(PUBLIC));
        let _ = piece(&p, &session(120, 5, FROM_CALLER, 0, &[]));
        let audio = serde_json::to_vec(&json!({"type":"input_audio_buffer.append","audio":busbar_contract::media::base64_encode(&vec![0;48_000])})).unwrap();
        let sent = piece(&p, &session(120, 5, FROM_CALLER, 0, &audio));
        let mut ready = sent.outcome == Outcome::Ready;
        for _ in 0..64 {
            let a = piece(&p, &session(120, 5, FROM_KERNEL, 0, &[]));
            ready &= a.outcome == Outcome::Ready && a.more == 0;
            if (a.emitted.is_empty() && !a.done) || a.done {
                break;
            }
        }
        let input = Piece {
            status: 101,
            ..session(120, 5, FROM_FAR_END, PIECE_HAS_STATUS, USAGE)
        };
        let narrow = piece(
            &p,
            &Piece {
                units_cap: 2,
                ..input
            },
        );
        let recall = piece(&p, &input);
        let end = piece(&p, &session(120, 5, FROM_CALLER, PIECE_LAST, &[]));
        let lines = [
            format!(
                "narrow {:?} units=[{}]",
                narrow.outcome,
                narrow.units_line()
            ),
            format!(
                "re-call {:?} units=[{}]",
                recall.outcome,
                recall.units_line()
            ),
            format!(
                "end {:?} done={} units=[{}]",
                end.outcome,
                end.done,
                end.units_line()
            ),
        ];
        let matched = ready
            && lines
                == [
                    "narrow Failed units=[needed 6]",
                    "re-call Ready units=[0:1=10 1:1=20 2:1=3 3:1=4 4:1=1 8:1=1]",
                    "end Ready done=true units=[0:1=10 1:1=20 2:1=3 3:1=4 4:1=1 8:1=1]",
                ];
        assert!(
            matched,
            "OBSERVATION V3-metering-lease-settled dropped={dropped} matched={matched}: {lines:?}"
        );
    }
}

#[test]
fn observation_v4_dialect_downscope() {
    for dropped in [false, true] {
        let d = Dispatcher::new(
            DispatchConfig::default(),
            busbar_contract::services::test_caller_binding(),
        );
        let p = load(&d, "observe-downscope", dropped);
        let _ = open(&p, SESSION, Some(PUBLIC));
        let initial = piece(&p, &session(140, 5, FROM_CALLER, 0, &[]));
        let update = serde_json::to_vec(&json!({"type":"session.update","session":{"instructions":"x","turn_detection":{"type":"semantic_vad","eagerness":"high"},"input_audio_format":"g711_ulaw","output_audio_format":"g711_ulaw"}})).unwrap();
        let a = piece(&p, &session(140, 5, FROM_CALLER, 0, &update));
        let mut ready =
            initial.outcome == Outcome::Ready && a.outcome == Outcome::Ready && a.more == 0;
        let mut frames = Vec::new();
        if !a.emitted.is_empty() {
            frames.push((a.to_far, serde_json::from_slice::<Value>(&a.emitted).ok()));
        }
        for _ in 0..64 {
            let a = piece(&p, &session(140, 5, FROM_KERNEL, 0, &[]));
            ready &= a.outcome == Outcome::Ready && a.more == 0;
            let quiet = (a.emitted.is_empty() && !a.done) || a.done;
            if !a.emitted.is_empty() {
                frames.push((a.to_far, serde_json::from_slice::<Value>(&a.emitted).ok()));
            }
            if quiet {
                break;
            }
        }
        let matched = ready
            && matches!(frames.as_slice(),[(true,Some(v))] if v["session"]["model"] == "m-cap" && v["session"].get("instructions").is_none() && v["session"]["audio"]["input"]["turn_detection"]["type"] == "server_vad" && v["session"].get("turn_detection").is_none());
        assert!(
            matched,
            "OBSERVATION V4-dialect-downscope dropped={dropped} matched={matched}: {frames:?}"
        );
    }
}

#[test]
fn closed_session_refuses_late_frames_and_renders_budget_refusal() {
    for dropped in [false, true] {
        let d = Dispatcher::new(
            DispatchConfig::default(),
            busbar_contract::services::test_caller_binding(),
        );
        let p = load(&d, "observe-close", dropped);
        let _ = open(&p, SESSION, Some(PUBLIC));
        let _ = piece(&p, &session(130, 5, FROM_CALLER, 0, &[]));
        let audio = serde_json::to_vec(&json!({"type":"response.output_audio.delta","delta":busbar_contract::media::base64_encode(&[0;96])})).unwrap();
        let first = piece(
            &p,
            &Piece {
                status: 101,
                ..session(130, 5, FROM_FAR_END, PIECE_HAS_STATUS, &audio)
            },
        );
        let mut ready = first.outcome == Outcome::Ready && first.more == 0;
        let mut done = u32::from(first.done);
        for _ in 0..64 {
            let a = piece(&p, &session(130, 5, FROM_KERNEL, 0, &[]));
            ready &= a.outcome == Outcome::Ready && a.more == 0;
            done += u32::from(a.done);
            if (a.emitted.is_empty() && !a.done) || a.done {
                break;
            }
        }
        let end = piece(&p, &session(130, 5, FROM_CALLER, PIECE_LAST, &[]));
        ready &= end.outcome == Outcome::Ready && end.more == 0;
        done += u32::from(end.done);
        let late = piece(&p, &session(130, 5, FROM_FAR_END, 0, &audio));
        let cut = refusal(&p, 130, 0, 429, "the caller's budget is exhausted");
        let body: Value = serde_json::from_str(&cut.body).unwrap_or_default();
        let matched = ready
            && done == 1
            && (late.outcome, late.emitted.len()) == (Outcome::Refused, 0)
            && cut.outcome == Outcome::Ready
            && body["error"]["type"] == "rate_limit_error";
        assert!(matched,
            "closed-session-and-budget-refusal dropped={dropped} matched={matched}: ends={done} late={late:?} cut={cut:?}"
        );
    }
}

#[test]
fn completed_session_accepts_only_empty_far_terminal_in_either_order() {
    for dropped in [false, true] {
        for far_first in [false, true] {
            for flags in [PIECE_LAST, PIECE_LAST | PIECE_CUT] {
                let d = Dispatcher::new(
                    DispatchConfig::default(),
                    busbar_contract::services::test_caller_binding(),
                );
                let p = load(&d, "terminal-order", dropped);
                assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
                assert_eq!(
                    piece(&p, &session(150, 5, FROM_CALLER, 0, &[])).outcome,
                    Outcome::Ready
                );
                let usage = piece(
                    &p,
                    &Piece {
                        status: 101,
                        ..session(150, 5, FROM_FAR_END, PIECE_HAS_STATUS, USAGE)
                    },
                );
                assert_eq!(usage.outcome, Outcome::Ready);
                let terminal = session(150, 5, FROM_FAR_END, flags, &[]);
                if far_first {
                    let far = piece(&p, &terminal);
                    assert_eq!(far.outcome, Outcome::Ready);
                    assert!(!far.done, "far terminal does not complete the caller reply");
                }
                let near = piece(&p, &session(150, 5, FROM_CALLER, PIECE_LAST, &[]));
                assert_eq!(near.outcome, Outcome::Ready);
                assert!(near.done);
                assert_eq!(
                    near.units, usage.units,
                    "terminal settles cumulative usage once"
                );
                for _ in 0..2 {
                    let far = piece(&p, &terminal);
                    assert_eq!(
                        far.outcome,
                        Outcome::Ready,
                        "dropped={dropped} far_first={far_first} flags={flags}: {far:?}"
                    );
                    assert!(!far.done && !far.to_far && far.more == 0);
                    assert!(
                        far.emitted.is_empty() && far.units.is_empty() && far.fields.is_empty()
                    );
                    assert_eq!(far.records, 0, "cleanup does not write another audit row");
                }
                for late in [
                    session(999, 5, FROM_FAR_END, flags, &[]),
                    session(150, 5, FROM_FAR_END, flags, USAGE),
                    session(150, 5, FROM_FAR_END, 0, USAGE),
                    session(150, 5, FROM_CALLER, PIECE_LAST, &[]),
                    session(150, 5, FROM_KERNEL, 0, &[]),
                    session(150, 3, FROM_FAR_END, flags, &[]),
                ] {
                    assert_eq!(piece(&p, &late).outcome, Outcome::Refused, "{late:?}");
                }
                assert_eq!(drive(&p), (Outcome::Ready, vec![]));
            }
        }
    }
}
