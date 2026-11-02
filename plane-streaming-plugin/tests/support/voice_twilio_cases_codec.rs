// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors
use super::*;

fn codec_fixture(name: &str, v: &Value) {
    let event = name
        .strip_prefix("from-twilio.")
        .or_else(|| name.strip_prefix("to-twilio."))
        .unwrap()
        .strip_suffix(".json")
        .unwrap();
    assert_eq!(v["event"], event);
    assert!(v.get("type").is_none());
    if event != "connected" {
        assert!(v["streamSid"].as_str().is_some_and(|s| !s.is_empty()));
    }
    let b = bytes(v);
    match (name.starts_with("to-twilio."), event) {
        (_, "connected") => assert!(matches!(
            TwilioEnvelope::decode(&b),
            Ok(TwilioEvent::Connected)
        )),
        (false, "start") => {
            let TwilioEvent::Start(s) = TwilioEnvelope::decode(&b).unwrap() else {
                panic!("start did not decode as Start")
            };
            assert_eq!(s.stream_sid, v["start"]["streamSid"]);
            assert_eq!(s.call_sid, v["start"]["callSid"]);
            assert_g711_ulaw(&s.media_format).unwrap();
        }
        (out, "media") => {
            let TwilioEvent::Media {
                stream_sid,
                payload: audio,
            } = TwilioEnvelope::decode(&b).unwrap()
            else {
                panic!("media did not decode as Media")
            };
            assert_eq!(stream_sid, v["streamSid"].as_str().unwrap());
            assert_eq!(audio, unb64(&payload(v)));
            assert!(!audio.is_empty());
            let again = TwilioEnvelope::encode_media(&stream_sid, &audio);
            let TwilioEvent::Media {
                stream_sid: sid2,
                payload: audio2,
            } = TwilioEnvelope::decode(&again).unwrap()
            else {
                panic!("media did not roundtrip")
            };
            assert_eq!(sid2, stream_sid);
            assert_eq!(audio2, audio);
            if out {
                assert_eq!(serde_json::from_slice::<Value>(&again).unwrap(), *v);
            }
        }
        (_, "mark") => {
            let TwilioEvent::Mark { name, .. } = TwilioEnvelope::decode(&b).unwrap() else {
                panic!("mark did not decode as Mark")
            };
            assert!(!name.is_empty());
        }
        (false, "dtmf") => {
            let TwilioEvent::Dtmf { digit, .. } = TwilioEnvelope::decode(&b).unwrap() else {
                panic!("dtmf did not decode as Dtmf")
            };
            assert!(!digit.is_empty());
        }
        (false, "stop") => assert!(matches!(TwilioEnvelope::decode(&b), Ok(TwilioEvent::Stop))),
        (true, "clear") => assert_eq!(v.as_object().unwrap().len(), 2),
        _ => panic!("unexpected fixture {name}"),
    }
}

#[test]
fn from_twilio_connected() {
    let v = fixture("from-twilio.connected.json");
    codec_fixture("from-twilio.connected.json", &v);
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
    codec_fixture("from-twilio.start.json", &v);
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
    codec_fixture("from-twilio.media.json", &v);
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
    codec_fixture("from-twilio.stop.json", &v);
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
    codec_fixture("from-twilio.dtmf.json", &v);
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
    codec_fixture("from-twilio.mark.json", &v);
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
    codec_fixture("to-twilio.media.json", &v);
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
    let v = fixture("to-twilio.mark.json");
    codec_fixture("to-twilio.mark.json", &v);
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
    let v = fixture("to-twilio.clear.json");
    codec_fixture("to-twilio.clear.json", &v);
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
