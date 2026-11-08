// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors
use super::*;
const PER_SESSION_CLASS: u32 = 8;
const USAGE_DONE: &[u8] = br#"{"type":"response.done","response":{"usage":{"input_token_details":{"audio_tokens":10,"text_tokens":3},"output_token_details":{"audio_tokens":20,"text_tokens":4}}}}"#;
fn usage_units(secs: u64) -> String {
    format!("0:1=10 1:1=20 2:1=3 3:1=4 4:1={secs} {PER_SESSION_CLASS}:1=1")
}
fn units_of(what: &str, a: &Answer) -> String {
    format!("{what} {:?} units=[{}]", a.outcome, a.units_line())
}
fn chunk(n: usize) -> Vec<u8> {
    vec![u8::try_from(n % 251).unwrap_or(0); 160]
}
fn caller_media(p: &Plugin<Plane>, stream: u64, sid: &str, frames: usize) -> (Answer, usize) {
    assert!(frames > 0);
    let first = bytes(&media_frame(sid, 0, &chunk(0)));
    let mut last = piece(p, &session(stream, CLAIM, FROM_CALLER, 0, &first));
    let mut bad = usize::from(last.outcome != Outcome::Ready);
    for n in 1..frames {
        let b = bytes(&media_frame(sid, n, &chunk(n)));
        last = piece(p, &session(stream, CLAIM, FROM_CALLER, 0, &b));
        bad += usize::from(last.outcome != Outcome::Ready);
    }
    (last, bad)
}

#[test]
fn twilio_cumulative_units_short_recall_and_stop_settlement() {
    let expected = vec![
        "start Ready units=[]".to_string(),
        "caller audio 2s Ready units=[4:1=2]".to_string(),
        "frames not READY 0".to_string(),
        "turn attempt Ready units=[4:1=2]".to_string(),
        format!("far usage Ready units=[{}]", usage_units(2)),
        format!("collect Ready units=[{}]", usage_units(2)),
        format!("second turn 0.98s Ready units=[{}]", usage_units(3)),
        "short Failed units=[needed 6]".to_string(),
        format!("short re-call Ready units=[{}]", usage_units(3)),
        format!("stop mid-turn Ready units=[{}] done=true", usage_units(3)),
        "second stop Refused units=[] done=false".to_string(),
        "unanswered start Ready units=[]".to_string(),
        "unanswered audio 1s Ready units=[4:1=1]".to_string(),
        "unanswered stop Ready units=[4:1=1] done=true".to_string(),
        "settled start Ready units=[]".to_string(),
        format!("settled stop Ready units=[{}] done=true", usage_units(2)),
    ];
    let dispatcher = Dispatcher::new(
        DispatchConfig::default(),
        busbar_contract::services::test_caller_binding(),
    );
    for dropped in [false, true] {
        let p = load(&dispatcher, "twilio-meter", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        let sid = sid();
        let start = bytes(&fixture("from-twilio.start.json"));
        let stop = bytes(&fixture("from-twilio.stop.json"));
        let mut lines = Vec::new();
        lines.push(units_of(
            "start",
            &piece(&p, &session(60, CLAIM, FROM_CALLER, 0, &start)),
        ));
        let (a, bad) = caller_media(&p, 60, &sid, 100);
        lines.push(units_of("caller audio 2s", &a));
        lines.push(format!("frames not READY {bad}"));
        let turn = Piece {
            attempt_no: 1,
            ..session(60, CLAIM, FROM_KERNEL, 0, &[])
        };
        lines.push(units_of("turn attempt", &piece(&p, &turn)));
        let far = Piece {
            status: 101,
            ..session(60, CLAIM, FROM_FAR_END, PIECE_HAS_STATUS, USAGE_DONE)
        };
        lines.push(units_of("far usage", &piece(&p, &far)));
        lines.push(units_of(
            "collect",
            &piece(&p, &session(60, CLAIM, FROM_KERNEL, 0, &[])),
        ));
        let (a, _) = caller_media(&p, 60, &sid, 49);
        lines.push(units_of("second turn 0.98s", &a));
        let short = bytes(&media_frame(&sid, 49, &chunk(49)));
        let x = Piece {
            units_cap: 2,
            ..session(60, CLAIM, FROM_CALLER, 0, &short)
        };
        lines.push(units_of("short", &piece(&p, &x)));
        lines.push(units_of(
            "short re-call",
            &piece(&p, &session(60, CLAIM, FROM_CALLER, 0, &short)),
        ));
        let end = piece(&p, &session(60, CLAIM, FROM_CALLER, 0, &stop));
        lines.push(format!(
            "{} done={}",
            units_of("stop mid-turn", &end),
            end.done
        ));
        let again = piece(&p, &session(60, CLAIM, FROM_CALLER, 0, &stop));
        lines.push(format!(
            "{} done={}",
            units_of("second stop", &again),
            again.done
        ));
        lines.push(units_of(
            "unanswered start",
            &piece(&p, &session(61, CLAIM, FROM_CALLER, 0, &start)),
        ));
        let (a, _) = caller_media(&p, 61, &sid, 50);
        lines.push(units_of("unanswered audio 1s", &a));
        let end = piece(&p, &session(61, CLAIM, FROM_CALLER, 0, &stop));
        lines.push(format!(
            "{} done={}",
            units_of("unanswered stop", &end),
            end.done
        ));
        lines.push(units_of(
            "settled start",
            &piece(&p, &session(62, CLAIM, FROM_CALLER, 0, &start)),
        ));
        let _ = caller_media(&p, 62, &sid, 100);
        let turn = Piece {
            attempt_no: 1,
            ..session(62, CLAIM, FROM_KERNEL, 0, &[])
        };
        let _ = piece(&p, &turn);
        let far = Piece {
            status: 101,
            ..session(62, CLAIM, FROM_FAR_END, PIECE_HAS_STATUS, USAGE_DONE)
        };
        let _ = piece(&p, &far);
        let end = piece(&p, &session(62, CLAIM, FROM_CALLER, 0, &stop));
        lines.push(format!(
            "{} done={}",
            units_of("settled stop", &end),
            end.done
        ));
        assert_eq!(lines, expected, "actual origin dropped={dropped}");
    }
}
