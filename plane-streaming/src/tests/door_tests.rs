// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Tests for `crates/busbar-plane-streaming/src/door.rs`: the P-item pins the door holds (spec
//! DONE item 2, "All P-item behaviours match 1.5.5"), each written against the door as it serves.

use super::*;

/// P-ITEM: WRONG-PROVIDER ATTRIBUTION (spec DONE item 2, "All P-item behaviours match 1.5.5";
/// first fixed red-before-green in busbar e15713578a). A live session is served by the provider
/// its door DIALED: a Gemini Live session's far frames ride the Gemini Live socket's need, at the
/// Gemini Live socket path, its caller arrives in the Gemini Live dialect, and the usage it is
/// metered on is read off the Gemini far end's own report. None of it falls back to the
/// first-declared provider, OpenAI Realtime (its dialect index 0, its socket the first outbound
/// need). Before the fix the paired branch hard-coded the first configured upstream and its lane,
/// so a Gemini session's every turn was billed on the OpenAI lane: the wrong provider's money. An
/// OpenAI Realtime session (the sideband, and the telephony carrier it bridges) still rides OpenAI
/// Realtime's, so the pin does not simply invert the bug.
///
/// The 1.5.5 behaviour this matches is its one surface's (the llm surface; owner correction
/// 2026-09-28): usage is ledgered and metered against the SERVING lane and its provider, never
/// another configured one (v1.5.5 `crates/busbar/src/proxy/usage.rs:57-103`, "`lane` is the
/// SERVING lane (post-failover)"). A bug fixed to match, never a signed diff.
#[test]
fn p_item_wrong_provider_attribution_a_session_rides_the_dialed_provider_not_the_first() {
    use crate::driven::{arrive, GEMINI_SOCKET_PATH, REALTIME_SOCKET_PATH};
    use crate::session_door::PER_SESSION_CLASS;

    let cfg = StreamsCfg::default();
    // (door, the far end's need, its socket path, the dialect the caller arrives in)
    let served = [
        (
            Door::Gemini,
            RIDES_LIVE_SOCKET,
            GEMINI_SOCKET_PATH,
            GEMINI_LIVE,
        ),
        (
            Door::Sideband,
            RIDES_REALTIME_SOCKET,
            REALTIME_SOCKET_PATH,
            OPENAI_REALTIME,
        ),
        (
            Door::Twilio,
            RIDES_REALTIME_SOCKET,
            REALTIME_SOCKET_PATH,
            Dialect::TwilioMediaStreams.name(),
        ),
        (
            Door::Realtime,
            RIDES_REALTIME_SOCKET,
            REALTIME_SOCKET_PATH,
            OPENAI_REALTIME,
        ),
    ];
    for (door, need, path, dialect) in served {
        assert_eq!(door.rides(), need, "{door:?} rides its own provider's need");
        let declared = &NEEDS[need as usize - 1];
        assert_eq!(declared.direction, DIRECTION_OUTBOUND, "{door:?}");
        assert_eq!(declared.egress_class, EGRESS_PROVIDER, "{door:?}");
        let attempt = door
            .route(&cfg.session, None, &[])
            .expect("a session door's dial is a request")
            .expect("a session door dials");
        assert_eq!(
            attempt.target, path,
            "{door:?} dials its own provider's socket"
        );
        let arrival = arrive(door as u32).expect("a published door");
        assert_eq!(
            DIALECTS_BY_NAME[arrival.dialect as usize], dialect,
            "{door:?} arrives in its own dialect"
        );
    }
    assert_ne!(
        Door::Gemini.rides(),
        RIDES_REALTIME_SOCKET,
        "a Gemini Live session never rides the first-declared provider's socket"
    );

    // Metered on the serving provider's own report: a Gemini Live usage frame counts on the
    // Gemini session, and counts nothing on an OpenAI Realtime one (whose provider never said it).
    let report = br#"{"usageMetadata":{"promptTokenCount":10,"responseTokenCount":20}}"#;
    let text_in = crate::session_unit::CLASSES
        .iter()
        .position(|c| *c == crate::meta::CLASS_TEXT_TOKENS_IN)
        .expect("declared") as u32;
    let text_out = crate::session_unit::CLASSES
        .iter()
        .position(|c| *c == crate::meta::CLASS_TEXT_TOKENS_OUT)
        .expect("declared") as u32;
    let mut gemini = Live::open(Door::Gemini, &cfg).expect("a session door");
    gemini.from_far_end(report, 0);
    assert_eq!(
        gemini.units(),
        [(text_in, 10), (text_out, 20), (PER_SESSION_CLASS, 1)],
        "the Gemini session is metered on the Gemini far end's report"
    );
    let mut openai = Live::open(Door::Sideband, &cfg).expect("a session door");
    openai.from_far_end(report, 0);
    assert_eq!(
        openai.units(),
        [(PER_SESSION_CLASS, 1)],
        "an OpenAI Realtime session is never metered on another provider's report"
    );
}

/// P-ITEM: REFUSAL-REASON COLLAPSE (spec DONE item 2, "All P-item behaviours match 1.5.5"; TODO
/// L-ENG9). The streaming plane's refusal table once ended in `_ => "internal"`, so a spent
/// budget, a frozen group, a replayed key, a superseded unit or a deadline told the caller the
/// node had broken. 1.5.5's one surface answered none of its limit reasons as an internal error
/// (v1.5.5 `crates/busbar/src/ingress/mod.rs:237-305`: rate 429 `rate_limit_error`, budget 429,
/// frozen group 403 `permission_error`; none a 500).
///
/// Pinned through the door's own renderer, [`refusal_render`], over every reason a plane can be
/// handed (`reason_of`) at every status the kernel can hand it (its class defaults and a plane's
/// own rows): the reason picks no answer of its own. Every reason of one class renders the same
/// bytes; a refusal reads as the dialect's internal error (`server_error`) exactly when the
/// status the kernel chose for its class is a server status, never because the plane fell through
/// on a reason it did not name; and the message is the kernel's words, except an authentication
/// refusal, which reads in the vendor's own terms. The family is the one classification's
/// (`busbar_contract::abi::plane::RefusalCode::class`), never this plane's own.
#[test]
fn p_item_refusal_reason_collapse_only_the_kernels_class_picks_the_answer_and_none_reads_internal()
{
    const WORDS: &str = "the kernel's words";
    for status in [400u16, 401, 403, 404, 413, 429, 500, 502, 503, 504] {
        let mut answers: Vec<(RefusalClass, Vec<u8>)> = Vec::new();
        for code in RefusalCode::ALL {
            // Two codes are the kernel's own money verdicts and never reach a plane.
            if reason_of(code.code()).is_none() {
                continue;
            }
            let class = code.class();
            let bytes = refusal_render(status, code.code(), WORDS);
            let body: serde_json::Value =
                serde_json::from_slice(&bytes).expect("the refusal is the dialect's JSON");
            let kind = body["error"]["type"]
                .as_str()
                .expect("the error names its type");
            assert_eq!(
                kind == "server_error",
                status >= 500,
                "{code:?} (class {class:?}) at {status} renders {kind:?}"
            );
            let (message, error_code) = if class == RefusalClass::Unauthenticated {
                (
                    AUTH_FAILURE_MESSAGE,
                    serde_json::Value::from(AUTH_FAILURE_CODE),
                )
            } else {
                (WORDS, serde_json::Value::Null)
            };
            assert_eq!(body["error"]["message"], message, "{code:?} at {status}");
            assert_eq!(body["error"]["code"], error_code, "{code:?} at {status}");
            match answers.iter().find(|(c, _)| *c == class) {
                Some((_, first)) => assert_eq!(
                    *first, bytes,
                    "{code:?} answers differently from the rest of {class:?} at {status}"
                ),
                None => answers.push((class, bytes)),
            }
        }
    }
}
