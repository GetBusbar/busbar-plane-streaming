// SPDX-License-Identifier: Apache-2.0
//! Ordinary composition assertions over each actual streaming image. No runner or planted subject.
#[path = "support/voice_abi.rs"]
pub mod voice_abi;
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

#[test]
fn gemini_claim_arrival_and_handshake_keep_their_dialect() {
    const SETUP: &[u8] = br#"{"setup":{"model":"models/gemini-2.0-flash-exp","generationConfig":{"responseModalities":["AUDIO"]}}}"#;
    const COMPLETE: &[u8] = br#"{"setupComplete":{}}"#;
    let mut deliveries = Vec::new();
    for dropped in [false, true] {
        let d = Dispatcher::new(DispatchConfig::default());
        let p = load(&d, "voice-gemini", dropped);
        let opened = open(&p, SESSION, Some(PUBLIC));
        assert_eq!(opened.outcome, Outcome::Ready);
        assert!(opened
            .claims
            .iter()
            .any(|c| c.line == "GET /v1/realtime/gemini/{call_id} ws" && c.refusal_dialect == 1));
        assert!(opened
            .claims
            .iter()
            .any(|c| c.line == "GET /v1/realtime/sideband/{call_id} ws" && c.refusal_dialect == 0));
        assert_eq!(opened.audience, "https://gw.example.com/v1/realtime");
        let arrived = arrive(&p, 3);
        assert_eq!(arrived.outcome, Outcome::Ready);
        assert_eq!(arrived.principal, PRINCIPAL_REQUIRED);
        assert_eq!(arrived.route, (ROUTE_DIRECT, MODEL.to_string()));
        assert_eq!(p.served().dialects[arrived.dialect as usize], "gemini_live");
        let initial = piece(&p, &session(40, 3, FROM_CALLER, 0, &[]));
        assert_eq!(initial.outcome, Outcome::Ready);
        assert_eq!(initial.more, 0);
        let mut answers = vec![initial];
        for input in [
            session(40, 3, FROM_CALLER, 0, SETUP),
            Piece {
                status: 101,
                ..session(40, 3, FROM_FAR_END, PIECE_HAS_STATUS, COMPLETE)
            },
        ] {
            let a = piece(&p, &input);
            assert_eq!(a.outcome, Outcome::Ready);
            assert_eq!(a.more, 0);
            answers.push(a);
            let mut dry = false;
            for _ in 0..64 {
                let a = piece(&p, &session(40, 3, FROM_KERNEL, 0, &[]));
                assert_eq!(a.outcome, Outcome::Ready);
                assert_eq!(a.more, 0);
                let quiet = a.emitted.is_empty() && !a.done;
                let done = a.done;
                answers.push(a);
                if quiet || done {
                    dry = true;
                    break;
                }
            }
            assert!(dry, "Gemini handshake collection runs dry");
        }
        let far: Vec<_> = answers
            .iter()
            .filter(|a| a.to_far && !a.emitted.is_empty())
            .collect();
        let near: Vec<_> = answers
            .iter()
            .filter(|a| !a.to_far && !a.emitted.is_empty())
            .collect();
        assert_eq!(far.len(), 1);
        assert_eq!(far[0].need, 5);
        assert_eq!(far[0].verb, "GET");
        assert_eq!(far[0].target, GEMINI_SOCKET);
        let setup: Value = serde_json::from_slice(&far[0].emitted).unwrap();
        assert!(
            setup["setup"].is_object(),
            "the far frame is a Gemini setup"
        );
        assert_eq!(near.len(), 1);
        assert_eq!(
            serde_json::from_slice::<Value>(&near[0].emitted).unwrap(),
            serde_json::from_slice::<Value>(COMPLETE).unwrap()
        );
        deliveries.push((opened, arrived, answers));
    }
    assert_eq!(deliveries[0], deliveries[1]);
}

#[test]
fn provider_frames_use_actual_declared_socket_needs_and_report_usage() {
    let mut deliveries = Vec::new();
    for dropped in [false, true] {
        let d = Dispatcher::new(DispatchConfig::default());
        let p = load(&d, "voice-dial", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        let audio = uplink(100);
        let gemini_audio = gemini_uplink(100);
        let mut dialect_answers = Vec::new();
        for (stream, claim, bytes, usage, need, socket, want) in [
            (
                50,
                5,
                audio.as_slice(),
                USAGE_DONE,
                4,
                "/v1/realtime",
                vec![(0, 10), (1, 20), (2, 3), (3, 4)],
            ),
            (
                51,
                3,
                gemini_audio.as_slice(),
                GEMINI_USAGE,
                5,
                GEMINI_SOCKET,
                vec![(0, 100), (1, 80), (2, 40)],
            ),
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
            let mut answers = Vec::new();
            for input in [
                session(stream, claim, FROM_CALLER, 0, bytes),
                Piece {
                    status: 101,
                    ..session(stream, claim, FROM_FAR_END, PIECE_HAS_STATUS, usage)
                },
            ] {
                let a = piece(&p, &input);
                assert_eq!(a.outcome, Outcome::Ready);
                assert_eq!(a.more, 0);
                answers.push(a);
                let mut dry = false;
                for _ in 0..64 {
                    let a = piece(&p, &session(stream, claim, FROM_KERNEL, 0, &[]));
                    assert_eq!(a.outcome, Outcome::Ready);
                    assert_eq!(a.more, 0);
                    let quiet = a.emitted.is_empty() && !a.done;
                    let done = a.done;
                    answers.push(a);
                    if quiet || done {
                        dry = true;
                        break;
                    }
                }
                assert!(dry, "provider collection runs dry");
            }
            let far: Vec<_> = answers
                .iter()
                .filter(|a| a.to_far && !a.emitted.is_empty())
                .collect();
            assert!(!far.is_empty());
            assert!(far
                .iter()
                .all(|a| a.need == need && a.verb == "GET" && dials_socket(&a.target, socket)));
            let units = &answers.last().unwrap().units;
            let got: Vec<_> = units
                .iter()
                .filter(|u| u.0 < 4)
                .map(|u| (u.0, u.2))
                .collect();
            assert!(!want.is_empty());
            assert_eq!(got, want);
            assert!(
                units.iter().any(|u| u.0 == 8 && u.2 == 1),
                "one answered-session fee"
            );
            dialect_answers.push(answers);
        }
        deliveries.push(dialect_answers);
    }
    assert_eq!(deliveries[0], deliveries[1]);
}

#[test]
fn all_policy_refusal_reasons_render_4xx_without_opening_or_dialing() {
    use busbar_contract::abi::plane::{class_of, reason_code, reason_of, RefusalClass};
    use busbar_contract::caps::ReasonCode;
    let classes = [
        RefusalClass::Unreadable,
        RefusalClass::Rejected,
        RefusalClass::Unauthenticated,
        RefusalClass::Forbidden,
        RefusalClass::TooLarge,
        RefusalClass::Throttled,
        RefusalClass::Busy,
        RefusalClass::QuotaExhausted,
        RefusalClass::NotFound,
    ];
    let mut deliveries = Vec::new();
    for dropped in [false, true] {
        let d = Dispatcher::new(DispatchConfig::default());
        let p = load(&d, "voice-refusal", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        let mut rendered = Vec::new();
        for (status, words, kind) in [
            (429, "the caller's budget is exhausted", "rate_limit_error"),
            (403, "the key holds no session grant", "permission_error"),
            (401, "no credential was presented", "authentication_error"),
        ] {
            let r = refusal(&p, 60, 0, status, words);
            assert_eq!(r.outcome, Outcome::Ready);
            assert_eq!(r.status, 0);
            assert_eq!(r.records, 0);
            assert_eq!(
                r.fields,
                [("content-type".to_string(), "application/json".to_string())]
            );
            assert_eq!(
                serde_json::from_str::<Value>(&r.body).unwrap(),
                json!({"error":{"message":words,"type":kind,"param":null,"code":null}})
            );
            rendered.push(r);
        }
        let mut seen_classes = Vec::new();
        let mut policy = Vec::new();
        for reason in ReasonCode::ALL
            .iter()
            .filter(|r| classes.contains(&class_of(**r)))
            .filter(|r| reason_of(reason_code(**r)).is_some())
        {
            let class = class_of(*reason);
            let status = busbar_kernel::plane_driver::refusal_status(*reason);
            let status16 = u16::try_from(status).unwrap();
            let r = refusal_for(
                &p,
                62,
                0,
                reason_code(*reason),
                status,
                "the request was refused",
            );
            assert!((400..500).contains(&status16));
            let kind = match status {
                401 => "authentication_error",
                403 => "permission_error",
                404 => "not_found_error",
                429 => "rate_limit_error",
                400..=499 => "invalid_request_error",
                _ => "server_error",
            };
            assert_ne!(kind, "server_error");
            let expected = if class == RefusalClass::Unauthenticated {
                json!({"error":{"message":"Incorrect API key provided.","type":kind,"param":null,"code":"invalid_api_key"}})
            } else {
                json!({"error":{"message":"the request was refused","type":kind,"param":null,"code":null}})
            };
            assert_eq!(r.outcome, Outcome::Ready);
            assert_eq!(r.status, 0);
            assert_eq!(r.records, 0);
            assert_eq!(
                r.fields,
                [("content-type".to_string(), "application/json".to_string())]
            );
            let body: Value = serde_json::from_str(&r.body).unwrap();
            assert_ne!(body["error"]["type"], "server_error");
            assert_eq!(body, expected);
            seen_classes.push(class);
            policy.push((reason_code(*reason), status, r));
        }
        for class in classes {
            assert!(
                seen_classes.contains(&class),
                "every original policy class is exercised"
            );
        }
        let audio = downlink(96);
        let far = piece(&p, &session(60, 5, FROM_FAR_END, 0, &audio));
        let collect = piece(&p, &session(60, 5, FROM_KERNEL, 0, &[]));
        assert_eq!((far.outcome, far.emitted.len()), (Outcome::Refused, 0));
        assert_eq!(
            (collect.outcome, collect.emitted.len()),
            (Outcome::Refused, 0)
        );
        let driven = drive(&p);
        assert_eq!(driven, (Outcome::Ready, Vec::new()));
        let initial = piece(&p, &session(61, 5, FROM_CALLER, 0, &[]));
        assert_eq!(initial.outcome, Outcome::Ready);
        assert_eq!(initial.more, 0);
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
        let caller_audio = uplink(100);
        let control = piece(&p, &session(61, 5, FROM_CALLER, 0, &caller_audio));
        assert_eq!(control.outcome, Outcome::Ready);
        assert!(control.to_far);
        assert_eq!(control.need, 4);
        let mut dry = false;
        for _ in 0..64 {
            let a = piece(&p, &session(61, 5, FROM_KERNEL, 0, &[]));
            assert_eq!(a.outcome, Outcome::Ready);
            assert_eq!(a.more, 0);
            if (a.emitted.is_empty() && !a.done) || a.done {
                dry = true;
                break;
            }
        }
        assert!(dry);
        deliveries.push((rendered, policy, far, collect, driven, control));
    }
    assert_eq!(deliveries[0], deliveries[1]);
}

#[test]
fn routes_and_attempts_preserve_fresh_dials_without_early_output() {
    let mut deliveries = Vec::new();
    for dropped in [false, true] {
        let d = Dispatcher::new(DispatchConfig::default());
        let p = load(&d, "voice-route", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        let mut arrivals = Vec::new();
        for claim in [0, 1, 3, 4, 5] {
            let a = arrive(&p, claim);
            assert_eq!(a.outcome, Outcome::Ready);
            assert_eq!(
                a.route,
                if claim == 0 {
                    (ROUTE_LOCAL, String::new())
                } else {
                    (ROUTE_DIRECT, MODEL.to_string())
                }
            );
            arrivals.push(a);
        }
        let initial = piece(&p, &session(70, 5, FROM_CALLER, 0, &[]));
        assert_eq!(initial.outcome, Outcome::Ready);
        assert_eq!(initial.more, 0);
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
        let audio = uplink(100);
        let caller = piece(&p, &session(70, 5, FROM_CALLER, 0, &audio));
        assert_eq!(caller.outcome, Outcome::Ready);
        assert_eq!(caller.more, 0);
        let mut turn = vec![caller];
        let mut dry = false;
        for _ in 0..64 {
            let a = piece(&p, &session(70, 5, FROM_KERNEL, 0, &[]));
            assert_eq!(a.outcome, Outcome::Ready);
            assert_eq!(a.more, 0);
            let quiet = (a.emitted.is_empty() && !a.done) || a.done;
            turn.push(a);
            if quiet {
                dry = true;
                break;
            }
        }
        assert!(dry);
        assert!(turn.iter().any(|a| !a.emitted.is_empty()
            && a.to_far
            && a.verb == "GET"
            && dials_socket(&a.target, "/v1/realtime")
            && a.need == 4));
        let mut attempts = Vec::new();
        for attempt_no in [1, 2] {
            let a = piece(
                &p,
                &Piece {
                    attempt_no,
                    ..session(70, 5, FROM_KERNEL, 0, &[])
                },
            );
            assert_eq!(a.outcome, Outcome::Ready);
            assert!(a.emitted.is_empty());
            assert!(!a.to_far && !a.done);
            attempts.push(a);
        }
        let unrouted = load(&d, "voice-no-route", dropped);
        assert_eq!(open(&unrouted, b"{}", Some(PUBLIC)).outcome, Outcome::Ready);
        let no_route = arrive(&unrouted, 1);
        assert_eq!(no_route.outcome, Outcome::Ready);
        assert_eq!(no_route.route, (ROUTE_POOL, String::new()));
        deliveries.push((arrivals, turn, attempts, no_route));
    }
    assert_eq!(deliveries[0], deliveries[1]);
}

#[test]
fn audit_rows_name_the_pool_once_and_never_create_private_records() {
    const SESSION_OPEN_ACTION: &str = "streaming.session.open";
    const AUDIT_KIND: &str = "streaming_session";
    let mut deliveries = Vec::new();
    for dropped in [false, true] {
        let d = Dispatcher::new(DispatchConfig::default());
        let p = load(&d, "voice-audit", dropped);
        assert_eq!(p.served().audit_kind, AUDIT_KIND);
        assert_eq!(p.served().op_classes, [SESSION_OPEN_ACTION]);
        assert!(p.declared().record_kinds.is_empty());
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        for claim in [0, 1, 3, 4] {
            let a = arrive(&p, claim);
            assert_eq!((a.outcome, a.op_class), (Outcome::Ready, 0));
        }
        let audio = uplink(100);
        let mut rows = Vec::new();
        for stream in [80, 81] {
            let initial = piece(&p, &session(stream, 5, FROM_CALLER, 0, &[]));
            assert_eq!(initial.outcome, Outcome::Ready);
            assert_eq!(initial.more, 0);
            assert!(!initial.emitted.is_empty());
            {
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
            rows.extend(initial.rows);
            for input in [
                session(stream, 5, FROM_CALLER, 0, &audio),
                Piece {
                    status: 101,
                    ..session(stream, 5, FROM_FAR_END, PIECE_HAS_STATUS, USAGE_DONE)
                },
                session(stream, 5, FROM_CALLER, PIECE_LAST, &[]),
            ] {
                let first = piece(&p, &input);
                assert_eq!((first.outcome, first.more), (Outcome::Ready, 0));
                let done = first.done;
                rows.extend(first.rows);
                if done {
                    continue;
                }
                let mut dry = false;
                for _ in 0..64 {
                    let a = piece(&p, &session(stream, 5, FROM_KERNEL, 0, &[]));
                    assert_eq!((a.outcome, a.more), (Outcome::Ready, 0));
                    let quiet = (a.emitted.is_empty() && !a.done) || a.done;
                    rows.extend(a.rows);
                    if quiet {
                        dry = true;
                        break;
                    }
                }
                assert!(dry);
            }
        }
        assert!(rows.is_empty(), "sessions author no records");
        let mint = piece(&p, &request(82, 0, FROM_CALLER, PIECE_LAST, &[]));
        assert_eq!(mint.outcome, Outcome::Ready);
        rows.extend(mint.rows);
        let refused = refusal(&p, 83, 0, 403, "refused");
        assert_eq!(refused.outcome, Outcome::Ready);
        rows.extend(refused.rows);
        assert_eq!(
            rows,
            [Row {
                op: RECORD_AUDIT,
                kind: AUDIT_REJECTED,
                key: "realtime_ticket.mint".to_string(),
                value: "pool:m-cap".to_string()
            }]
        );
        deliveries.push(rows);
    }
    assert_eq!(deliveries[0], deliveries[1]);
}

#[test]
fn session_exit_settles_once_and_refuses_every_late_side() {
    let mut deliveries = Vec::new();
    for dropped in [false, true] {
        let d = Dispatcher::new(DispatchConfig::default());
        let p = load(&d, "voice-exit", dropped);
        assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
        let initial = piece(&p, &session(90, 5, FROM_CALLER, 0, &[]));
        assert_eq!(initial.outcome, Outcome::Ready);
        assert_eq!(initial.more, 0);
        assert!(!initial.emitted.is_empty());
        {
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
        let audio = uplink(1000);
        let mut ends = u32::from(initial.done);
        let mut answers = Vec::new();
        for input in [
            session(90, 5, FROM_CALLER, 0, &audio),
            Piece {
                status: 101,
                ..session(90, 5, FROM_FAR_END, PIECE_HAS_STATUS, USAGE_DONE)
            },
            session(90, 5, FROM_CALLER, 0, &audio),
        ] {
            let a = piece(&p, &input);
            assert_eq!((a.outcome, a.more), (Outcome::Ready, 0));
            ends += u32::from(a.done);
            answers.push(a);
            let mut dry = false;
            for _ in 0..64 {
                let a = piece(&p, &session(90, 5, FROM_KERNEL, 0, &[]));
                assert_eq!((a.outcome, a.more), (Outcome::Ready, 0));
                let quiet = (a.emitted.is_empty() && !a.done) || a.done;
                ends += u32::from(a.done);
                answers.push(a);
                if quiet {
                    dry = true;
                    break;
                }
            }
            assert!(dry);
        }
        let end = piece(&p, &session(90, 5, FROM_CALLER, PIECE_LAST, &[]));
        assert_eq!(end.outcome, Outcome::Ready);
        assert!(end.done);
        assert_eq!(end.units_line(), "0:1=10 1:1=20 2:1=3 3:1=4 4:1=2 8:1=1");
        ends += u32::from(end.done);
        let collect = piece(&p, &session(90, 5, FROM_KERNEL, 0, &[]));
        let far = piece(&p, &session(90, 5, FROM_FAR_END, 0, &downlink(96)));
        for a in [&collect, &far] {
            assert_eq!((a.outcome, a.emitted.len()), (Outcome::Refused, 0));
        }
        assert_eq!(drive(&p), (Outcome::Ready, Vec::new()));
        let initial = piece(&p, &session(91, 5, FROM_CALLER, 0, &[]));
        assert_eq!(initial.outcome, Outcome::Ready);
        assert_eq!(initial.more, 0);
        assert!(!initial.emitted.is_empty());
        {
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
        ends += u32::from(initial.done);
        let interrupted = piece(&p, &session(91, 5, FROM_CALLER, PIECE_LAST, &[]));
        assert_eq!(interrupted.outcome, Outcome::Ready);
        assert!(interrupted.done && interrupted.units.is_empty());
        ends += u32::from(interrupted.done);
        assert_eq!(ends, 2);
        deliveries.push((answers, end, collect, far, interrupted));
    }
    assert_eq!(deliveries[0], deliveries[1]);
}

#[test]
fn tool_calls_and_caller_results_relay_unchanged_without_authored_answers() {
    let mut deliveries = Vec::new();
    for dropped in [false, true] {
        let mut dialects = Vec::new();
        for claim in [5, 3] {
            let d = Dispatcher::new(DispatchConfig::default());
            let p = load(&d, "voice-tool", dropped);
            assert_eq!(open(&p, SESSION, Some(PUBLIC)).outcome, Outcome::Ready);
            let initial = piece(&p, &session(95, claim, FROM_CALLER, 0, &[]));
            assert_eq!(initial.outcome, Outcome::Ready);
            assert_eq!(initial.more, 0);
            if claim == 5 {
                assert!(!initial.emitted.is_empty());
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
            let (announce, usage, reply, tokens) = if claim == 5 {
                (
                    vec![
                        json!({"type":"response.output_item.added","item":{"type":"function_call","call_id":"call_x","name":"lookup"}}),
                        json!({"type":"response.function_call_arguments.delta","call_id":"call_x","delta":"{\"q\":1}"}),
                        json!({"type":"response.function_call_arguments.done","call_id":"call_x","name":"lookup","arguments":"{\"q\":1}"}),
                    ],
                    USAGE_DONE,
                    json!({"type":"conversation.item.create","item":{"type":"function_call_output","call_id":"call_x","output":"{\"ok\":true}"}}),
                    vec![(0, 10), (1, 20), (2, 3), (3, 4)],
                )
            } else {
                (
                    vec![
                        json!({"toolCall":{"functionCalls":[{"id":"call_x","name":"lookup","args":{"q":1}}]}}),
                    ],
                    GEMINI_USAGE,
                    json!({"toolResponse":{"functionResponses":[{"id":"call_x","name":"lookup","response":{"ok":true}}]}}),
                    vec![(0, 100), (1, 80), (2, 40)],
                )
            };
            let announce: Vec<Vec<u8>> = announce
                .iter()
                .map(|v| serde_json::to_vec(v).unwrap())
                .collect();
            let reply = serde_json::to_vec(&reply).unwrap();
            let mut phases = Vec::new();
            for (index, bytes) in announce
                .iter()
                .map(Vec::as_slice)
                .chain([usage, reply.as_slice()])
                .enumerate()
            {
                let caller = index == announce.len() + 1;
                let input = Piece {
                    status: if index == 0 { 101 } else { 0 },
                    ..session(
                        95,
                        claim,
                        if caller { FROM_CALLER } else { FROM_FAR_END },
                        if index == 0 { PIECE_HAS_STATUS } else { 0 },
                        bytes,
                    )
                };
                let a = piece(&p, &input);
                assert_eq!((a.outcome, a.more), (Outcome::Ready, 0));
                let mut unit_reports = vec![a.units];
                let mut frames = Vec::new();
                if !a.emitted.is_empty() {
                    frames.push((a.to_far, a.emitted));
                }
                let mut dry = false;
                for _ in 0..64 {
                    let a = piece(&p, &session(95, claim, FROM_KERNEL, 0, &[]));
                    assert_eq!((a.outcome, a.more), (Outcome::Ready, 0));
                    let quiet = (a.emitted.is_empty() && !a.done) || a.done;
                    unit_reports.push(a.units);
                    if !a.emitted.is_empty() {
                        frames.push((a.to_far, a.emitted));
                    }
                    if quiet {
                        dry = true;
                        break;
                    }
                }
                assert!(dry);
                assert_eq!(
                    frames,
                    [(caller, bytes.to_vec())],
                    "each phase relays exactly its one frame"
                );
                if index == announce.len() {
                    let last_units = unit_reports.last().unwrap();
                    let got: Vec<_> = last_units
                        .iter()
                        .filter(|u| u.0 < 4)
                        .map(|u| (u.0, u.2))
                        .collect();
                    assert_eq!(got, tokens);
                    assert!(last_units.contains(&(5, 1, 1)), "one tool call");
                }
                phases.push(frames);
            }
            dialects.push(phases);
        }
        deliveries.push(dialects);
    }
    assert_eq!(deliveries[0], deliveries[1]);
}
