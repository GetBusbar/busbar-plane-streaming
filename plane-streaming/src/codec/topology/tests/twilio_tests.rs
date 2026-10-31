// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! TWILIO ENVELOPE TESTS (behind `runtime`): the JSON Media Streams envelope decodes/encodes to the
//! raw µ-law bytes the telephony proxy carries; the base64 payload maps to the exact wire bytes; the
//! `start` media-format guard refuses a non-`g711_ulaw` negotiation.

use crate::codec::topology::twilio::{assert_g711_ulaw, TwilioEnvelope, TwilioError, TwilioEvent};

fn frame(v: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&v).unwrap()
}

// ── The captured inbound `media` frame decodes, and decode→encode is byte-stable on the wire shape ──

#[test]
fn media_frame_round_trips_byte_stable() {
    // A canonical OUTBOUND frame is the byte-stable shape (inbound frames carry extra diagnostic fields
    // Twilio does not want echoed back): encode → decode → encode reproduces the exact bytes.
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

// ── The base64 µ-law payload maps to the exact raw bytes TelephonyProxy::run's client_in expects ────

#[test]
fn base64_payload_maps_to_raw_mulaw_bytes() {
    // A full inbound `media` frame as Twilio actually sends it (sequenceNumber, track, chunk, timestamp).
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
    // "////" is the canonical base64 of three 0xFF bytes — the exact raw µ-law bytes handed to client_in.
    assert_eq!(payload, vec![0xFFu8, 0xFF, 0xFF]);

    // And a second known vector, to prove it is genuinely base64-decoding, not passing the string through.
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

// ── The lifecycle events decode without flowing into the session pump ───────────────────────────────

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
///
/// `dtmf` is one of the six messages Twilio Media Streams sends TO the socket, emitted on the
/// inbound track when a DTMF-enabled stream hears a touch-tone. Leaving it unmodelled makes it an
/// `UnknownEvent`, and the plane maps any decode error onto a malformed-frame refusal — so a caller
/// pressing 5 would be handled as a garbled or forged frame. It carries no audio, so it decodes to
/// its own variant and the adapter discards it exactly like `mark`.
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
    // A `dtmf` frame with the inner object missing still decodes — the digit is what Twilio may
    // omit, and refusing the whole frame over a missing digit would reintroduce the same refusal
    // this variant exists to remove.
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

// ── The start media-format guard REFUSES a non-g711_ulaw negotiation (fail closed, not warn) ────────

#[test]
fn start_media_format_guard_refuses_non_g711() {
    // A g711_ulaw / 8000Hz / mono start passes the guard.
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

    // A 16 kHz PCM start is REFUSED — the wrong bytes must never reach the barge-in truncate math.
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

// ── Malformed / unknown frames fail closed ──────────────────────────────────────────────────────────

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
    // A media event whose payload is not valid base64 is refused, not admitted as garbage bytes.
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

/// RED (audit plane-streaming round 52b0f93ac9, S55): a carrier frame that ends the call (a start
/// in another media format, a frame not in Twilio's shape, a payload that is not clean base64) is
/// said in one log line naming why, so an operator can tell why every call from a misconfigured
/// stream ends. RED: the call ended with no line anywhere.
#[test]
fn a_refused_carrier_frame_is_said_in_one_log_line_naming_why() {
    use crate::codec::topology::twilio::{CallerStep, TwilioBridge};
    let cap = busbar_test_kit::warn_capture::WarnCapture::default();
    let steps = tracing::subscriber::with_default(cap.clone(), || {
        let mut bridge = TwilioBridge::default();
        [
            bridge.from_caller(&start_of("MZ1", "audio/l16", 16_000)),
            TwilioBridge::default().from_caller(b"not json"),
        ]
    });
    assert!(steps.iter().all(|s| matches!(s, CallerStep::Refused)));
    assert_eq!(cap.messages().len(), 2, "{:?}", cap.messages());
    assert!(
        cap.contains("16000Hz"),
        "the format is named: {:?}",
        cap.messages()
    );
    assert!(cap.contains("malformed"), "{:?}", cap.messages());
}

/// RED (audit plane-streaming round 52b0f93ac9, S13): the stream a call's `start` binds is the
/// one its media must name; a second `start` (Twilio sends one per stream) binds nothing, so media
/// naming the second stream is not admitted. RED: each start rebound the stream.
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
