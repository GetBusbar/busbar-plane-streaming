// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors
//! Ordinary tests of the real Twilio plane door, through each loader origin.
use busbar_contract::abi::host::conn::connector::DIRECTION_INBOUND;
use busbar_contract::abi::mechanism::call::Outcome;
use busbar_contract::abi::plane::{
    FROM_CALLER, FROM_FAR_END, FROM_KERNEL, PIECE_HAS_STATUS, PRINCIPAL_REQUIRED, ROUTE_DIRECT,
};
use busbar_contract::media::{base64_decode, base64_encode};
use busbar_plugin_loader::dispatch::kinds::plane::Plane;
use busbar_plugin_loader::dispatch::{DispatchConfig, Dispatcher, Plugin};
use serde_json::{json, Value};
#[path = "support/voice_abi.rs"]
pub mod voice_abi;
use voice_abi::{arrive, load, open, piece, session, Answer, Piece, MODEL, PUBLIC, SESSION};
#[path = "support/voice_twilio_cases_codec.rs"]
mod codec_cases;
#[path = "support/voice_twilio_cases_media.rs"]
mod media_cases;
#[path = "support/voice_twilio_cases_meter.rs"]
mod meter_cases;
const CLAIM: u32 = 4;
const OTHER_SID: &str = "MZYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYY";

fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testing/voice-conformance/fixtures/twilio")
        .join(name);
    serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .expect("verbatim published Twilio fixture parses")
}
fn bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}
fn sid() -> String {
    fixture("from-twilio.start.json")["start"]["streamSid"]
        .as_str()
        .unwrap()
        .to_string()
}
fn payload(v: &Value) -> String {
    v["media"]["payload"].as_str().unwrap().to_string()
}
fn unb64(s: &str) -> Vec<u8> {
    base64_decode(s).expect("fixture base64").to_vec()
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn far_delta(b64: &str) -> Value {
    json!({"event_id": "evt_far_01", "type": "response.output_audio.delta", "response_id": "resp_001", "item_id": "item_asst_001", "output_index": 0, "content_index": 0, "delta": b64})
}
fn far_done() -> Value {
    json!({"event_id": "evt_far_02", "type": "response.output_audio.done", "response_id": "resp_001", "item_id": "item_asst_001", "output_index": 0, "content_index": 0})
}
fn far_speech_started() -> Value {
    json!({"event_id": "evt_far_03", "type": "input_audio_buffer.speech_started", "audio_start_ms": 210, "item_id": "item_user_002"})
}
fn media_frame(sid: &str, n: usize, audio: &[u8]) -> Value {
    json!({"event": "media", "sequenceNumber": (n + 2).to_string(), "media": {"track": "inbound", "chunk": (n + 1).to_string(), "timestamp": (n * 20).to_string(), "payload": base64_encode(audio)}, "streamSid": sid})
}
fn ready(a: &Answer) {
    assert_eq!(a.outcome, Outcome::Ready);
    assert_eq!(a.more, 0);
}
fn quiet(a: Answer) {
    ready(&a);
    assert!(a.emitted.is_empty(), "unexpected frame: {:?}", a.emitted);
    assert!(!a.done);
}
fn lock_of(v: &Value) -> String {
    let format = |flat: &str, side: &str| match v[flat]
        .as_str()
        .or_else(|| v["audio"][side]["format"]["type"].as_str())
    {
        Some("g711_ulaw" | "audio/pcmu") => "mulaw".to_string(),
        Some(other) => other.to_string(),
        None => "unset".to_string(),
    };
    let known = [
        "model",
        "type",
        "audio",
        "input_audio_format",
        "output_audio_format",
    ];
    let more: Vec<_> = v
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .filter(|k| !known.contains(k))
        .collect();
    format!(
        "far-lock:in={},out={},more={more:?}",
        format("input_audio_format", "input"),
        format("output_audio_format", "output")
    )
}
fn described(a: &Answer) -> String {
    let v: Value = serde_json::from_slice(&a.emitted).unwrap_or(Value::Null);
    if a.to_far {
        return if v["type"] == "input_audio_buffer.append" {
            format!("far-audio:{}", hex(&unb64(v["audio"].as_str().unwrap())))
        } else if v["type"] == "session.update" {
            lock_of(&v["session"])
        } else if v.is_null() {
            format!("far-bytes:{}", String::from_utf8_lossy(&a.emitted))
        } else {
            format!("far-frame:{v}")
        };
    }
    let sid = v["streamSid"].as_str().unwrap_or_default();
    match v["event"].as_str() {
        Some("media") => format!(
            "caller-media:{sid}:{}",
            v["media"]["payload"].as_str().unwrap_or_default()
        ),
        Some("mark") => format!(
            "caller-mark:{sid}:{}",
            if v["mark"]["name"].as_str().is_some_and(|s| !s.is_empty()) {
                "named"
            } else {
                "unnamed"
            }
        ),
        Some("clear") => format!("caller-clear:{sid}"),
        _ => format!("caller-other:{}", String::from_utf8_lossy(&a.emitted)),
    }
}
fn emitted(a: Answer, expected: &str) {
    ready(&a);
    assert!(!a.done);
    assert!(!a.emitted.is_empty());
    assert_eq!(described(&a), expected);
}
fn locked(a: Answer) {
    emitted(
        a,
        "far-lock:in=mulaw,out=mulaw,more=[\"max_output_tokens\", \"truncation\"]",
    );
}
fn stopped(a: Answer) {
    ready(&a);
    assert!(a.done);
    assert!(a.emitted.is_empty());
}

// Twilio does not specify caller-side output for response.output_audio.done. Preserve exactly
// that freedom while asserting that every actual answer is READY and no far-bound frame leaks.
fn audio_done(p: &Plugin<Plane>, stream: u64, first_far: bool) {
    let b = bytes(&far_done());
    let a = piece(
        p,
        &Piece {
            status: if first_far { 101 } else { 0 },
            ..session(
                stream,
                CLAIM,
                FROM_FAR_END,
                if first_far { PIECE_HAS_STATUS } else { 0 },
                &b,
            )
        },
    );
    ready(&a);
    assert!(!a.done);
    assert!(a.emitted.is_empty() || !a.to_far);
    for _ in 0..64 {
        let a = piece(p, &session(stream, CLAIM, FROM_KERNEL, 0, &[]));
        ready(&a);
        assert!(!a.done);
        assert!(a.emitted.is_empty() || !a.to_far);
        if a.emitted.is_empty() {
            return;
        }
    }
    panic!("audio.done caller output never ran dry");
}

#[test]
fn twilio_route_signature_principal_and_start_binding() {
    let dispatcher = Dispatcher::new(
        DispatchConfig::default(),
        busbar_contract::services::test_caller_binding(),
    );
    for dropped in [false, true] {
        let p = load(&dispatcher, "twilio-route", dropped);
        let opened = open(&p, SESSION, Some(PUBLIC));
        assert_eq!(opened.outcome, Outcome::Ready);
        assert!(opened
            .claims
            .iter()
            .any(|c| c.line == "GET /twilio/{call_id} ws" && c.refusal_dialect == 2));
        let served = p.served();
        assert!(served.need_auths.iter().zip(&served.need_transports).any(
            |((direction, auth), transport)| *direction == DIRECTION_INBOUND
                && *auth == "webhook-signature"
                && *transport == "ws"
        ));
        let a = arrive(&p, CLAIM);
        assert_eq!(a.outcome, Outcome::Ready);
        assert_eq!(a.principal, PRINCIPAL_REQUIRED);
        assert_eq!(a.route, (ROUTE_DIRECT, MODEL.to_string()));
        assert_eq!(served.dialects[a.dialect as usize], "twilio_media_streams");
        let start = bytes(&fixture("from-twilio.start.json"));
        let connected = bytes(&json!({"event":"connected","protocol":"Call","version":"1.0.0"}));
        quiet(piece(&p, &session(40, CLAIM, FROM_CALLER, 0, &[])));
        quiet(piece(&p, &session(40, CLAIM, FROM_CALLER, 0, &connected)));
        quiet(piece(&p, &session(40, CLAIM, FROM_KERNEL, 0, &[])));
        locked(piece(&p, &session(40, CLAIM, FROM_CALLER, 0, &start)));
        quiet(piece(&p, &session(40, CLAIM, FROM_KERNEL, 0, &[])));
        let audio = vec![7_u8; 160];
        let media = bytes(&media_frame(&sid(), 1, &audio));
        quiet(piece(&p, &session(41, CLAIM, FROM_CALLER, 0, &[])));
        quiet(piece(&p, &session(41, CLAIM, FROM_CALLER, 0, &media)));
        quiet(piece(&p, &session(41, CLAIM, FROM_KERNEL, 0, &[])));
        quiet(piece(&p, &session(42, CLAIM, FROM_CALLER, 0, &[])));
        locked(piece(&p, &session(42, CLAIM, FROM_CALLER, 0, &start)));
        quiet(piece(&p, &session(42, CLAIM, FROM_KERNEL, 0, &[])));
        emitted(
            piece(&p, &session(42, CLAIM, FROM_CALLER, 0, &media)),
            &format!("far-audio:{}", hex(&audio)),
        );
        quiet(piece(&p, &session(42, CLAIM, FROM_KERNEL, 0, &[])));
        quiet(piece(
            &p,
            &session(
                42,
                CLAIM,
                FROM_CALLER,
                0,
                &bytes(&media_frame(OTHER_SID, 2, &audio)),
            ),
        ));
        quiet(piece(&p, &session(42, CLAIM, FROM_KERNEL, 0, &[])));
        quiet(piece(&p, &session(43, CLAIM, FROM_CALLER, 0, &[])));
        locked(piece(&p, &session(43, CLAIM, FROM_CALLER, 0, &start)));
        quiet(piece(&p, &session(43, CLAIM, FROM_KERNEL, 0, &[])));
        let typed =
            bytes(&json!({"type":"input_audio_buffer.append","audio":base64_encode(&audio)}));
        stopped(piece(&p, &session(43, CLAIM, FROM_CALLER, 0, &typed)));
    }
}
