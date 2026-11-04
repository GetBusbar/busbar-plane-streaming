// SPDX-License-Identifier: Apache-2.0
//! Published Twilio fixture and route predicates over the owning private implementation.

use crate::codec::topology::twilio::{assert_g711_ulaw, TwilioEnvelope, TwilioEvent};
use busbar_contract::media::base64_decode;
use serde_json::Value;
fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testing/voice-conformance/fixtures/twilio")
        .join(name);
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
fn bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}
fn payload(v: &Value) -> String {
    v["media"]["payload"].as_str().unwrap().to_string()
}
fn unb64(s: &str) -> Vec<u8> {
    base64_decode(s).unwrap().to_vec()
}
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
    codec_fixture(
        "from-twilio.connected.json",
        &fixture("from-twilio.connected.json"),
    );
}
#[test]
fn from_twilio_start() {
    codec_fixture("from-twilio.start.json", &fixture("from-twilio.start.json"));
}
#[test]
fn from_twilio_media() {
    codec_fixture("from-twilio.media.json", &fixture("from-twilio.media.json"));
}
#[test]
fn from_twilio_stop() {
    codec_fixture("from-twilio.stop.json", &fixture("from-twilio.stop.json"));
}
#[test]
fn from_twilio_dtmf() {
    codec_fixture("from-twilio.dtmf.json", &fixture("from-twilio.dtmf.json"));
}
#[test]
fn from_twilio_mark() {
    codec_fixture("from-twilio.mark.json", &fixture("from-twilio.mark.json"));
}
#[test]
fn to_twilio_media() {
    codec_fixture("to-twilio.media.json", &fixture("to-twilio.media.json"));
}
#[test]
fn to_twilio_mark() {
    codec_fixture("to-twilio.mark.json", &fixture("to-twilio.mark.json"));
}
#[test]
fn to_twilio_clear() {
    codec_fixture("to-twilio.clear.json", &fixture("to-twilio.clear.json"));
}
#[test]
fn published_transcript_inbound_codec_reads() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testing/voice-conformance/fixtures/twilio/transcript.jsonl");
    let text = std::fs::read_to_string(path).unwrap();
    let lines: Vec<Value> = text
        .lines()
        .filter(|s| !s.trim().is_empty())
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(lines.len(), 11);
    for index in [1, 2, 3, 4, 5, 6, 9, 10] {
        TwilioEnvelope::decode(&bytes(&lines[index]["event"])).unwrap();
    }
}
