// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! TWILIO ENVELOPE TESTS (behind `runtime`): the JSON Media Streams envelope decodes/encodes to the

use crate::codec::topology::twilio::{assert_g711_ulaw, TwilioEnvelope, TwilioError, TwilioEvent};

fn frame(v: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&v).unwrap()
}

#[test]
fn media_frame_round_trips_byte_stable() {
    let raw = vec![0u8, 0, 0];
    let canonical = TwilioEnvelope::encode_media("MZ1", &raw);
    assert_eq!(
        canonical,
        br#"{"event":"media","streamSid":"MZ1","media":{"payload":"AAAA"}}"#.to_vec(),
        "the outbound media envelope has a fixed, deterministic shape"
    );

    let decoded = TwilioEnvelope::decode(&canonical).expect("the canonical media frame decodes");
    let TwilioEvent::Media {
        stream_sid,
        payload,
    } = decoded
    else {
        panic!("expected a media event, got {decoded:?}");
    };
    assert_eq!(stream_sid, "MZ1");
    assert_eq!(payload, raw);

    let re_encoded = TwilioEnvelope::encode_media(&stream_sid, &payload);
    assert_eq!(re_encoded, canonical, "decode→encode is byte-stable");
}

#[test]
fn base64_payload_maps_to_raw_mulaw_bytes() {
    let inbound = frame(serde_json::json!({
        "event": "media",
        "sequenceNumber": "3",
        "media": { "track": "inbound", "chunk": "3", "timestamp": "20", "payload": "////" },
        "streamSid": "MZ1",
    }));
    let TwilioEvent::Media { payload, .. } =
        TwilioEnvelope::decode(&inbound).expect("inbound media decodes")
    else {
        panic!("expected a media event");
    };

    assert_eq!(payload, vec![0xFFu8, 0xFF, 0xFF]);

    let zeros = frame(serde_json::json!({
        "event": "media",
        "media": { "payload": "AAAA" },
        "streamSid": "MZ1",
    }));
    let TwilioEvent::Media { payload, .. } =
        TwilioEnvelope::decode(&zeros).expect("zeros media decodes")
    else {
        panic!("expected a media event");
    };
    assert_eq!(payload, vec![0u8, 0, 0]);
}

#[test]
fn lifecycle_events_decode() {
    assert_eq!(
        TwilioEnvelope::decode(&frame(serde_json::json!({"event": "connected"}))).unwrap(),
        TwilioEvent::Connected
    );
    assert_eq!(
        TwilioEnvelope::decode(&frame(
            serde_json::json!({"event": "stop", "streamSid": "MZ1"})
        ))
        .unwrap(),
        TwilioEvent::Stop
    );
    let mark = TwilioEnvelope::decode(&frame(serde_json::json!({
        "event": "mark", "streamSid": "MZ1", "mark": { "name": "playback-1" }
    })))
    .unwrap();
    assert_eq!(
        mark,
        TwilioEvent::Mark {
            stream_sid: "MZ1".into(),
            name: "playback-1".into()
        }
    );
}

/// A CALLER PRESSING A KEY IS A LIFECYCLE EVENT, NOT A HOSTILE FRAME.
#[test]
fn a_dtmf_keypress_decodes_as_a_lifecycle_event() {
    let dtmf = TwilioEnvelope::decode(&frame(serde_json::json!({
        "event": "dtmf",
        "streamSid": "MZ1",
        "sequenceNumber": "5",
        "dtmf": { "track": "inbound_track", "digit": "5" }
    })))
    .unwrap();
    assert_eq!(
        dtmf,
        TwilioEvent::Dtmf {
            stream_sid: "MZ1".into(),
            digit: "5".into()
        }
    );

    assert_eq!(
        TwilioEnvelope::decode(&frame(
            serde_json::json!({"event": "dtmf", "streamSid": "MZ1"})
        ))
        .unwrap(),
        TwilioEvent::Dtmf {
            stream_sid: "MZ1".into(),
            digit: String::new()
        }
    );
}

#[test]
fn start_media_format_guard_refuses_non_g711() {
    let good = frame(serde_json::json!({
        "event": "start",
        "start": {
            "streamSid": "MZ1",
            "callSid": "CA1",
            "mediaFormat": { "encoding": "audio/x-mulaw", "sampleRate": 8000, "channels": 1 }
        },
        "streamSid": "MZ1",
    }));
    let TwilioEvent::Start(start) = TwilioEnvelope::decode(&good).unwrap() else {
        panic!("expected a start event");
    };
    assert_eq!(start.stream_sid, "MZ1");
    assert_eq!(start.call_sid, "CA1");
    assert!(assert_g711_ulaw(&start.media_format).is_ok());

    let bad = frame(serde_json::json!({
        "event": "start",
        "start": {
            "streamSid": "MZ1",
            "callSid": "CA1",
            "mediaFormat": { "encoding": "audio/l16", "sampleRate": 16000, "channels": 1 }
        },
        "streamSid": "MZ1",
    }));
    let TwilioEvent::Start(start) = TwilioEnvelope::decode(&bad).unwrap() else {
        panic!("expected a start event");
    };
    assert!(
        matches!(
            assert_g711_ulaw(&start.media_format),
            Err(TwilioError::FormatMismatch { .. })
        ),
        "a non-g711 mediaFormat is refused outright"
    );
}

#[test]
fn malformed_and_unknown_frames_fail_closed() {
    assert_eq!(
        TwilioEnvelope::decode(b"not json"),
        Err(TwilioError::Malformed)
    );
    assert_eq!(
        TwilioEnvelope::decode(&frame(serde_json::json!({"event": "teleport"}))),
        Err(TwilioError::UnknownEvent("teleport".into()))
    );

    assert_eq!(
        TwilioEnvelope::decode(&frame(serde_json::json!({
            "event": "media", "streamSid": "MZ1", "media": { "payload": "*not*base64*" }
        }))),
        Err(TwilioError::BadPayload)
    );
}

fn start_of(sid: &str, encoding: &str, rate: u32) -> Vec<u8> {
    frame(serde_json::json!({
        "event": "start",
        "start": {
            "streamSid": sid,
            "callSid": "CA1",
            "mediaFormat": { "encoding": encoding, "sampleRate": rate, "channels": 1 }
        },
        "streamSid": sid,
    }))
}

fn media_of(sid: &str) -> Vec<u8> {
    frame(serde_json::json!({
        "event": "media",
        "streamSid": sid,
        "media": { "payload": busbar_contract::media::base64_encode(&[0xff; 160]) },
    }))
}

/// A refused carrier frame is logged once with its reason.
#[test]
fn a_refused_carrier_frame_is_said_in_one_log_line_naming_why() {
    use crate::codec::topology::twilio::{CallerStep, TwilioBridge};
    struct Log(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Log {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = std::sync::Arc::clone(&output);
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || Log(std::sync::Arc::clone(&writer)))
        .finish();
    let steps = tracing::subscriber::with_default(subscriber, || {
        let mut bridge = TwilioBridge::default();
        [
            bridge.from_caller(&start_of("MZ1", "audio/l16", 16_000)),
            TwilioBridge::default().from_caller(b"not json"),
        ]
    });
    assert!(steps.iter().all(|s| matches!(s, CallerStep::Refused)));
    let lines = String::from_utf8(output.lock().unwrap().clone()).expect("UTF-8 diagnostics");
    assert_eq!(lines.lines().count(), 2, "{lines}");
    assert!(lines.contains("16000Hz"), "the format is named: {lines}");
    assert!(lines.contains("malformed"), "{lines}");
}

/// A second start frame cannot rebind the call's stream.
#[test]
fn a_second_start_does_not_rebind_the_calls_stream() {
    use crate::codec::topology::twilio::{CallerStep, TwilioBridge};
    let mut bridge = TwilioBridge::default();
    assert!(matches!(
        bridge.from_caller(&start_of("MZa", "audio/x-mulaw", 8000)),
        CallerStep::Nothing
    ));
    assert!(matches!(
        bridge.from_caller(&start_of("MZb", "audio/x-mulaw", 8000)),
        CallerStep::Nothing
    ));
    assert!(
        matches!(bridge.from_caller(&media_of("MZb")), CallerStep::Nothing),
        "media for a stream the call did not start is not admitted"
    );
    assert!(
        matches!(bridge.from_caller(&media_of("MZa")), CallerStep::Audio(_)),
        "the stream the call started still is"
    );
}
