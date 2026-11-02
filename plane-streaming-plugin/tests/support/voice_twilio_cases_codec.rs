// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors
use super::*;

#[test]
fn from_twilio_connected() {
    let v = fixture("from-twilio.connected.json");
    let dispatcher = Dispatcher::new(DispatchConfig::default());
    for dropped in [false, true] {
        let p = load(&dispatcher, "twilio-fixture-connected", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        quiet(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &[])));
        quiet(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &bytes(&v))));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
    }
}
#[test]
fn from_twilio_start() {
    let v = fixture("from-twilio.start.json");
    let dispatcher = Dispatcher::new(DispatchConfig::default());
    for dropped in [false, true] {
        let p = load(&dispatcher, "twilio-fixture-start", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        quiet(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &[])));
        locked(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &bytes(&v))));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
    }
}
#[test]
fn from_twilio_media() {
    let v = fixture("from-twilio.media.json");
    let dispatcher = Dispatcher::new(DispatchConfig::default());
    for dropped in [false, true] {
        let p = load(&dispatcher, "twilio-fixture-in-media", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        quiet(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &[])));
        locked(piece(
            &p,
            &session(
                77,
                CLAIM,
                FROM_CALLER,
                0,
                &bytes(&fixture("from-twilio.start.json")),
            ),
        ));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        emitted(
            piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &bytes(&v))),
            &format!("far-audio:{}", hex(&unb64(&payload(&v)))),
        );
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
    }
}
#[test]
fn from_twilio_stop() {
    let v = fixture("from-twilio.stop.json");
    let dispatcher = Dispatcher::new(DispatchConfig::default());
    for dropped in [false, true] {
        let p = load(&dispatcher, "twilio-fixture-stop", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        quiet(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &[])));
        locked(piece(
            &p,
            &session(
                77,
                CLAIM,
                FROM_CALLER,
                0,
                &bytes(&fixture("from-twilio.start.json")),
            ),
        ));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        stopped(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &bytes(&v))));
    }
}
#[test]
fn from_twilio_dtmf() {
    let v = fixture("from-twilio.dtmf.json");
    let dispatcher = Dispatcher::new(DispatchConfig::default());
    for dropped in [false, true] {
        let p = load(&dispatcher, "twilio-fixture-dtmf", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        quiet(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &[])));
        locked(piece(
            &p,
            &session(
                77,
                CLAIM,
                FROM_CALLER,
                0,
                &bytes(&fixture("from-twilio.start.json")),
            ),
        ));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        quiet(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &bytes(&v))));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
    }
}
#[test]
fn from_twilio_mark() {
    let v = fixture("from-twilio.mark.json");
    let dispatcher = Dispatcher::new(DispatchConfig::default());
    for dropped in [false, true] {
        let p = load(&dispatcher, "twilio-fixture-mark", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        quiet(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &[])));
        locked(piece(
            &p,
            &session(
                77,
                CLAIM,
                FROM_CALLER,
                0,
                &bytes(&fixture("from-twilio.start.json")),
            ),
        ));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        quiet(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &bytes(&v))));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
    }
}
#[test]
fn to_twilio_media() {
    let v = fixture("to-twilio.media.json");
    let dispatcher = Dispatcher::new(DispatchConfig::default());
    for dropped in [false, true] {
        let p = load(&dispatcher, "twilio-fixture-out-media", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        quiet(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &[])));
        locked(piece(
            &p,
            &session(
                77,
                CLAIM,
                FROM_CALLER,
                0,
                &bytes(&fixture("from-twilio.start.json")),
            ),
        ));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        let b = bytes(&far_delta(&payload(&v)));
        emitted(
            piece(
                &p,
                &Piece {
                    status: 101,
                    ..session(77, CLAIM, FROM_FAR_END, PIECE_HAS_STATUS, &b)
                },
            ),
            &format!("caller-media:{}:{}", sid(), payload(&v)),
        );
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
    }
}
#[test]
fn to_twilio_mark() {
    let dispatcher = Dispatcher::new(DispatchConfig::default());
    for dropped in [false, true] {
        let p = load(&dispatcher, "twilio-fixture-out-mark", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        quiet(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &[])));
        locked(piece(
            &p,
            &session(
                77,
                CLAIM,
                FROM_CALLER,
                0,
                &bytes(&fixture("from-twilio.start.json")),
            ),
        ));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        let reply = payload(&fixture("to-twilio.media.json"));
        let b = bytes(&far_delta(&reply));
        emitted(
            piece(
                &p,
                &Piece {
                    status: 101,
                    ..session(77, CLAIM, FROM_FAR_END, PIECE_HAS_STATUS, &b)
                },
            ),
            &format!("caller-media:{}:{reply}", sid()),
        );
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        audio_done(&p, 77, false);
    }
}
#[test]
fn to_twilio_clear() {
    let dispatcher = Dispatcher::new(DispatchConfig::default());
    for dropped in [false, true] {
        let p = load(&dispatcher, "twilio-fixture-out-clear", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        quiet(piece(&p, &session(77, CLAIM, FROM_CALLER, 0, &[])));
        locked(piece(
            &p,
            &session(
                77,
                CLAIM,
                FROM_CALLER,
                0,
                &bytes(&fixture("from-twilio.start.json")),
            ),
        ));
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        let reply = payload(&fixture("to-twilio.media.json"));
        let b = bytes(&far_delta(&reply));
        emitted(
            piece(
                &p,
                &Piece {
                    status: 101,
                    ..session(77, CLAIM, FROM_FAR_END, PIECE_HAS_STATUS, &b)
                },
            ),
            &format!("caller-media:{}:{reply}", sid()),
        );
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
        emitted(
            piece(
                &p,
                &session(77, CLAIM, FROM_FAR_END, 0, &bytes(&far_speech_started())),
            ),
            &format!("caller-clear:{}", sid()),
        );
        quiet(piece(&p, &session(77, CLAIM, FROM_KERNEL, 0, &[])));
    }
}
