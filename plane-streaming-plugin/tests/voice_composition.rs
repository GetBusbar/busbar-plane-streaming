// SPDX-License-Identifier: Apache-2.0
//! Ordinary composition assertions over each actual streaming image. No runner or planted subject.
#[path = "support/voice_abi.rs"]
mod voice_abi;
use busbar_contract::abi::mechanism::call::Outcome;
use busbar_contract::abi::plane::{
    AUDIT_REJECTED, FROM_CALLER, FROM_FAR_END, FROM_KERNEL, PIECE_HAS_STATUS, PIECE_LAST,
    PRINCIPAL_REQUIRED, RECORD_AUDIT, ROUTE_DIRECT, ROUTE_LOCAL, ROUTE_POOL,
};
use busbar_plugin_loader::dispatch::{DispatchConfig, Dispatcher};
use serde_json::{json, Value};
use voice_abi::*;

const USAGE_DONE: &[u8] = br#"{"type":"response.done","response":{"usage":{"input_token_details":{"audio_tokens":10,"text_tokens":3},"output_token_details":{"audio_tokens":20,"text_tokens":4}}}}"#;
const GEMINI_USAGE: &[u8] = br#"{"usageMetadata":{"promptTokenCount":140,"responseTokenCount":80,"totalTokenCount":220,"promptTokensDetails":[{"modality":"TEXT","tokenCount":40},{"modality":"AUDIO","tokenCount":100}],"responseTokensDetails":[{"modality":"AUDIO","tokenCount":80}]}}"#;
const GEMINI_SOCKET: &str =
    "/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";
const MINT_REFUSED: &[u8] = br#"{"error":{"code":"not_granted","message":"this key is not granted the session's model `m-cap`, so no ephemeral secret is minted for it","param":null,"type":"permission_error"}}"#;
fn uplink(ms: usize) -> Vec<u8> {
    serde_json::to_vec(&json!({"type":"input_audio_buffer.append","audio":busbar_contract::media::base64_encode(&vec![0_u8;ms*48])})).unwrap()
}
fn gemini_uplink(ms: usize) -> Vec<u8> {
    serde_json::to_vec(&json!({"realtimeInput":{"audio":{"mimeType":"audio/pcm;rate=16000","data":busbar_contract::media::base64_encode(&vec![0_u8;ms*32])}}})).unwrap()
}
fn downlink(n: usize) -> Vec<u8> {
    serde_json::to_vec(&json!({"type":"response.output_audio.delta","delta":busbar_contract::media::base64_encode(&vec![0_u8;n])})).unwrap()
}

#[test]
fn provider_credentials_stay_on_declared_needs_and_mint_dials_nothing() {
    let mut deliveries = Vec::new();
    for dropped in [false, true] {
        let d = Dispatcher::new(DispatchConfig::default());
        let p = load(&d, "voice-credentials", dropped);
        let served = p.served();
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        let arrived = arrive(&p, 0);
        let mint = piece(&p, &request(7, 0, FROM_CALLER, PIECE_LAST, &[]));
        assert_eq!(arrived.outcome, Outcome::Ready);
        assert_eq!(arrived.route, (ROUTE_LOCAL, String::new()));
        assert!(!mint.to_far);
        assert_eq!(mint.need, 0);
        assert!(mint.verb.is_empty() && mint.target.is_empty());
        assert_eq!(mint.outcome, Outcome::Ready);
        assert!(mint.done);
        assert_eq!(mint.status, 403);
        assert_eq!(mint.emitted, MINT_REFUSED);
        let audio = uplink(100);
        let gemini_audio = gemini_uplink(100);
        let mut rides = Vec::new();
        for (stream, claim, bytes, style) in [
            (20, 5, audio.as_slice(), "bearer"),
            (21, 3, gemini_audio.as_slice(), "x-goog-api-key"),
        ] {
            let initial = piece(&p, &session(stream, claim, FROM_CALLER, 0, &[]));
            assert_eq!(initial.outcome, Outcome::Ready);
            assert_eq!(initial.more, 0);
            if claim == 5 {
                let lock: Value = serde_json::from_slice(&initial.emitted).unwrap();
                assert!(initial.to_far);
                assert_eq!(initial.verb, "GET");
                assert_eq!(initial.target, "/v1/realtime?model=m-cap");
                assert_eq!(lock["type"], "session.update");
                assert_eq!(lock["session"]["type"], "realtime");
                assert_eq!(lock["session"]["max_output_tokens"], 4096);
                assert_eq!(
                    lock["session"]["truncation"]["token_limits"]["post_instructions"],
                    28672
                );
            }
            let ride = piece(&p, &session(stream, claim, FROM_CALLER, 0, bytes));
            let index = usize::try_from(ride.need)
                .unwrap()
                .checked_sub(1)
                .expect("far request names declared need");
            assert_eq!(
                served.need_auths[index].0,
                busbar_contract::abi::host::conn::connector::DIRECTION_OUTBOUND
            );
            assert_eq!(served.need_auths[index].1, style);
            assert_eq!(served.need_transports[index], "ws");
            assert!(served.need_targets[index].is_empty());
            assert!(!ride.verb.is_empty() && !ride.target.is_empty());
            for (name, value) in &ride.fields {
                assert!(![
                    "authorization",
                    "proxy-authorization",
                    "x-goog-api-key",
                    "x-api-key",
                    "api-key",
                    "openai-api-key",
                    "cookie"
                ]
                .contains(&name.to_ascii_lowercase().as_str()));
                assert!(!value.to_ascii_lowercase().starts_with("bearer "));
            }
            rides.push(ride);
            let mut dry = false;
            for _ in 0..64 {
                let a = piece(&p, &session(stream, claim, FROM_KERNEL, 0, &[]));
                assert_eq!(a.outcome, Outcome::Ready);
                assert_eq!(a.more, 0);
                if a.emitted.is_empty() && !a.done {
                    dry = true;
                    break;
                }
                if a.done {
                    dry = true;
                    break;
                }
            }
            assert!(dry, "collection runs dry");
        }
        assert_eq!(rides.len(), 2);
        for (name, style) in [
            ("openai_realtime", "bearer"),
            ("gemini_live", "x-goog-api-key"),
        ] {
            assert!(
                served
                    .dialect_auth
                    .iter()
                    .any(|(index, auth, _)| served.dialects[*index as usize] == name
                        && *auth == style)
            );
        }
        deliveries.push((arrived, mint, rides));
    }
    assert_eq!(deliveries[0], deliveries[1]);
}

#[test]
fn declared_units_fee_and_short_recall_settle_each_turn_once() {
    let mut deliveries = Vec::new();
    for dropped in [false, true] {
        let d = Dispatcher::new(DispatchConfig::default());
        let p = load(&d, "voice-metering", dropped);
        let facts = p.served();
        let classes: Vec<_> = facts
            .billable_classes
            .iter()
            .zip(&facts.billable_families)
            .map(|(c, f)| format!("{c}/{f}"))
            .collect();
        assert_eq!(
            classes,
            [
                "audio_tokens_in/token",
                "audio_tokens_out/token",
                "text_tokens_in/token",
                "text_tokens_out/token",
                "audio_seconds_in/duration",
                "tool_calls/count",
                "image_tokens_in/token",
                "video_tokens_in/token",
                "per_session/count"
            ]
        );
        assert_eq!(facts.fee_units, ["per_session"]);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        let admitted: Vec<_> = (0..5).map(|claim| arrive(&p, claim).units).collect();
        assert!(admitted.iter().all(|n| *n == 0));
        let audio = uplink(2000);
        let a = piece(&p, &session(20, 5, FROM_CALLER, 0, &audio));
        let b = piece(
            &p,
            &Piece {
                attempt_no: 1,
                ..session(20, 5, FROM_KERNEL, 0, &[])
            },
        );
        let c = piece(
            &p,
            &Piece {
                status: 101,
                ..session(20, 5, FROM_FAR_END, PIECE_HAS_STATUS, USAGE_DONE)
            },
        );
        let collect = piece(&p, &session(20, 5, FROM_KERNEL, 0, &[]));
        let short = piece(
            &p,
            &Piece {
                units_cap: 2,
                ..session(20, 5, FROM_CALLER, 0, &audio)
            },
        );
        let recall = piece(&p, &session(20, 5, FROM_CALLER, 0, &audio));
        let end = piece(&p, &session(20, 5, FROM_CALLER, PIECE_LAST, &[]));
        let unanswered = piece(&p, &session(30, 5, FROM_CALLER, 0, &audio));
        let unanswered_end = piece(&p, &session(30, 5, FROM_CALLER, PIECE_LAST, &[]));
        let answers = [
            a,
            b,
            c,
            collect,
            short,
            recall,
            end,
            unanswered,
            unanswered_end,
        ];
        let labels = [
            "caller audio",
            "turn attempt",
            "far usage",
            "collect",
            "short",
            "short re-call",
            "caller end",
            "unanswered audio",
            "unanswered end",
        ];
        let lines: Vec<_> = labels
            .iter()
            .zip(&answers)
            .map(|(name, a)| format!("{name} {:?} units=[{}]", a.outcome, a.units_line()))
            .collect();
        assert_eq!(
            lines,
            [
                "caller audio Ready units=[4:1=2]",
                "turn attempt Ready units=[4:1=2]",
                "far usage Ready units=[0:1=10 1:1=20 2:1=3 3:1=4 4:1=2 8:1=1]",
                "collect Ready units=[0:1=10 1:1=20 2:1=3 3:1=4 4:1=2 8:1=1]",
                "short Failed units=[needed 6]",
                "short re-call Ready units=[0:1=10 1:1=20 2:1=3 3:1=4 4:1=4 8:1=1]",
                "caller end Ready units=[0:1=10 1:1=20 2:1=3 3:1=4 4:1=4 8:1=1]",
                "unanswered audio Ready units=[4:1=2]",
                "unanswered end Ready units=[4:1=2]"
            ]
        );
        deliveries.push((classes, admitted, answers));
    }
    assert_eq!(deliveries[0], deliveries[1]);
}

#[test]
fn session_and_pool_grants_keep_principals_and_refuse_unpublished_claims() {
    let mut deliveries = Vec::new();
    for dropped in [false, true] {
        let d = Dispatcher::new(DispatchConfig::default());
        let p = load(&d, "voice-scope", dropped);
        assert_eq!(p.declared().scope_kinds, ["session", "pool"]);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        let mut arrivals = Vec::new();
        for claim in [0, 1, 2, 3, 4, 5, 9] {
            let a = arrive(&p, claim);
            if claim == 2 {
                assert_eq!(a.outcome, Outcome::Refused);
                assert_eq!(a.principal, PRINCIPAL_REQUIRED);
                assert_eq!(a.refusal, (1, 404));
            } else if claim <= 5 {
                assert_eq!(a.outcome, Outcome::Ready);
                assert_eq!(a.principal, PRINCIPAL_REQUIRED);
                assert_eq!(a.op_class, 0);
            } else {
                assert_eq!(a.outcome, Outcome::Refused);
                assert_eq!(a.refusal, (1, 404));
            }
            arrivals.push(a);
        }
        let refused = refusal(
            &p,
            0,
            0,
            403,
            "the key holds no session grant for this door",
        );
        let body: Value = serde_json::from_str(&refused.body).unwrap();
        assert_eq!(refused.outcome, Outcome::Ready);
        assert_eq!(refused.status, 0);
        assert_eq!(body["error"]["type"], "permission_error");
        assert_eq!(
            body["error"]["message"],
            "the key holds no session grant for this door"
        );
        deliveries.push((arrivals, refused));
    }
    assert_eq!(deliveries[0], deliveries[1]);
}
