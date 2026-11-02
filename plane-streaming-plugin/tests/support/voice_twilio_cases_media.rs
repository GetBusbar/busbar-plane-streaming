// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors
use super::*;

#[test]
fn published_transcript_in_exact_order() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testing/voice-conformance/fixtures/twilio/transcript.jsonl");
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<Value> = text
        .lines()
        .filter(|s| !s.trim().is_empty())
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(lines.len(), 11);
    assert_eq!(
        lines
            .iter()
            .map(|v| (
                v["dir"].as_str().unwrap(),
                v["event"]["event"].as_str().unwrap_or_default()
            ))
            .collect::<Vec<_>>(),
        [
            ("meta", ""),
            ("client", "connected"),
            ("client", "start"),
            ("client", "media"),
            ("client", "media"),
            ("client", "media"),
            ("client", "dtmf"),
            ("server", "media"),
            ("server", "mark"),
            ("client", "mark"),
            ("client", "stop"),
        ]
    );
    let sid = lines[2]["event"]["start"]["streamSid"].as_str().unwrap();
    let dispatcher = Dispatcher::new(DispatchConfig::default());
    for dropped in [false, true] {
        let p = load(&dispatcher, "twilio-transcript", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        quiet(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &[])));
        quiet(piece(
            &p,
            &session(77, CLAIM, FROM_CALLER, 0, &bytes(&lines[1]["event"])),
        ));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        locked(piece(
            &p,
            &session(77, CLAIM, FROM_CALLER, 0, &bytes(&lines[2]["event"])),
        ));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        for index in [3, 4, 5] {
            let v = &lines[index]["event"];
            emitted(
                piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &bytes(v))),
                &format!("far-audio:{}", hex(&unb64(&payload(v)))),
            );
            quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        }
        quiet(piece(
            &p,
            &session(77, CLAIM, FROM_CALLER, 0, &bytes(&lines[6]["event"])),
        ));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        let reply = payload(&lines[7]["event"]);
        let delta = bytes(&far_delta(&reply));
        emitted(
            piece(
                &p,
                &Piece {
                    status: 101,
                    ..session(77, CLAIM, FROM_FAR_END, PIECE_HAS_STATUS, &delta)
                },
            ),
            &format!("caller-media:{sid}:{reply}"),
        );
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        audio_done(&p, 77, false);
        quiet(piece(
            &p,
            &session(77, CLAIM, FROM_CALLER, 0, &bytes(&lines[9]["event"])),
        ));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        stopped(piece(
            &p,
            &session(77, CLAIM, FROM_CALLER, 0, &bytes(&lines[10]["event"])),
        ));
    }
}

#[test]
fn two_calls_preserve_media_sid_barge_in_and_one_terminal() {
    let dispatcher = Dispatcher::new(DispatchConfig::default());
    for dropped in [false, true] {
        let p = load(&dispatcher, "twilio-two-calls", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        for (stream, sid) in [(50, sid()), (51, OTHER_SID.to_string())] {
            let mut start = fixture("from-twilio.start.json");
            start["start"]["streamSid"] = json!(sid);
            start["streamSid"] = json!(sid);
            quiet(piece(&p, &session(stream, CLAIM, FROM_CALLER, 0, &[])));
            quiet(piece(
                &p,
                &session(
                    stream,
                    CLAIM,
                    FROM_CALLER,
                    0,
                    &bytes(&fixture("from-twilio.connected.json")),
                ),
            ));
            quiet(piece(&p, &session(stream, CLAIM, FROM_KERNEL, 0, &[])));
            locked(piece(
                &p,
                &session(stream, CLAIM, FROM_CALLER, 0, &bytes(&start)),
            ));
            quiet(piece(&p, &session(stream, CLAIM, FROM_KERNEL, 0, &[])));
            for n in 1..=3_u8 {
                let audio: Vec<u8> = (0..160)
                    .map(|i| n.wrapping_mul(31).wrapping_add(i))
                    .collect();
                emitted(
                    piece(
                        &p,
                        &session(
                            stream,
                            CLAIM,
                            FROM_CALLER,
                            0,
                            &bytes(&media_frame(&sid, usize::from(n), &audio)),
                        ),
                    ),
                    &format!("far-audio:{}", hex(&audio)),
                );
                quiet(piece(&p, &session(stream, CLAIM, FROM_KERNEL, 0, &[])));
            }
            let mut dtmf = fixture("from-twilio.dtmf.json");
            dtmf["streamSid"] = json!(sid);
            quiet(piece(
                &p,
                &session(stream, CLAIM, FROM_CALLER, 0, &bytes(&dtmf)),
            ));
            quiet(piece(&p, &session(stream, CLAIM, FROM_KERNEL, 0, &[])));
            let reply = payload(&fixture("to-twilio.media.json"));
            let delta = bytes(&far_delta(&reply));
            emitted(
                piece(
                    &p,
                    &Piece {
                        status: 101,
                        ..session(stream, CLAIM, FROM_FAR_END, PIECE_HAS_STATUS, &delta)
                    },
                ),
                &format!("caller-media:{sid}:{reply}"),
            );
            quiet(piece(&p, &session(stream, CLAIM, FROM_KERNEL, 0, &[])));
            audio_done(&p, stream, false);
            let mut mark = fixture("from-twilio.mark.json");
            mark["streamSid"] = json!(sid);
            quiet(piece(
                &p,
                &session(stream, CLAIM, FROM_CALLER, 0, &bytes(&mark)),
            ));
            quiet(piece(&p, &session(stream, CLAIM, FROM_KERNEL, 0, &[])));
            emitted(
                piece(&p, &session(stream, CLAIM, FROM_FAR_END, 0, &delta)),
                &format!("caller-media:{sid}:{reply}"),
            );
            quiet(piece(&p, &session(stream, CLAIM, FROM_KERNEL, 0, &[])));
            emitted(
                piece(
                    &p,
                    &session(
                        stream,
                        CLAIM,
                        FROM_FAR_END,
                        0,
                        &bytes(&far_speech_started()),
                    ),
                ),
                &format!("caller-clear:{sid}"),
            );
            quiet(piece(&p, &session(stream, CLAIM, FROM_KERNEL, 0, &[])));
            let mut stop = fixture("from-twilio.stop.json");
            stop["streamSid"] = json!(sid);
            stopped(piece(
                &p,
                &session(stream, CLAIM, FROM_CALLER, 0, &bytes(&stop)),
            ));
        }
        // The first call stays terminal after another call has run: neither media, another stop,
        // nor collection can resurrect it or produce any frame.
        let audio: Vec<u8> = (0..160).map(|i| 31_u8.wrapping_add(i)).collect();
        let media = bytes(&media_frame(&sid(), 1, &audio));
        let stop = bytes(&fixture("from-twilio.stop.json"));
        for (from, input) in [
            (FROM_CALLER, media.as_slice()),
            (FROM_CALLER, stop.as_slice()),
            (FROM_KERNEL, &[][..]),
        ] {
            let a = piece(&p, &session(50, CLAIM, from, 0, input));
            assert_eq!(a.outcome, Outcome::Refused);
            assert!(a.emitted.is_empty());
        }
    }
}

#[test]
fn twilio_tool_calls_are_opaque_and_never_answered() {
    let frames = [
        json!({"event_id": "evt_far_10", "type": "response.output_item.added", "response_id": "resp_002", "output_index": 0, "item": {"id": "item_fc_001", "type": "function_call", "status": "in_progress", "call_id": "call_tw_1", "name": "lookup", "arguments": ""}}),
        json!({"event_id": "evt_far_11", "type": "response.function_call_arguments.delta", "response_id": "resp_002", "item_id": "item_fc_001", "output_index": 0, "call_id": "call_tw_1", "delta": "{\"q\":1}"}),
        json!({"event_id": "evt_far_12", "type": "response.function_call_arguments.done", "response_id": "resp_002", "item_id": "item_fc_001", "output_index": 0, "call_id": "call_tw_1", "name": "lookup", "arguments": "{\"q\":1}"}),
    ];
    let dispatcher = Dispatcher::new(DispatchConfig::default());
    for dropped in [false, true] {
        let p = load(&dispatcher, "twilio-tools", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        quiet(piece(&p, &session(52, CLAIM, FROM_CALLER, 0, &[])));
        locked(piece(
            &p,
            &session(
                52,
                CLAIM,
                FROM_CALLER,
                0,
                &bytes(&fixture("from-twilio.start.json")),
            ),
        ));
        quiet(piece(&p, &session(52, CLAIM, FROM_KERNEL, 0, &[])));
        for (index, v) in frames.iter().enumerate() {
            let raw = bytes(v);
            let a = piece(
                &p,
                &Piece {
                    status: if index == 0 { 101 } else { 0 },
                    ..session(
                        52,
                        CLAIM,
                        FROM_FAR_END,
                        if index == 0 { PIECE_HAS_STATUS } else { 0 },
                        &raw,
                    )
                },
            );
            ready(&a);
            assert!(!a.to_far);
            assert!(!a.done);
            assert_eq!(a.emitted, raw);
            quiet(piece(&p, &session(52, CLAIM, FROM_KERNEL, 0, &[])));
        }
        stopped(piece(
            &p,
            &session(
                52,
                CLAIM,
                FROM_CALLER,
                0,
                &bytes(&fixture("from-twilio.stop.json")),
            ),
        ));
    }
}
