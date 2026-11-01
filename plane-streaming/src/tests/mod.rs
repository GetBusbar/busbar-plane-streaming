//! Test-only wiring: the selector and style checks.

/// What a request path matches, decided the same way the boot's overlap check decides it.
mod selectors {
    use crate::claims::matches_selector;
    use busbar_contract::grammar::Selector;

    /// A one-level prefix claims the segment below it and nothing else. A sibling whose name merely
    /// starts with the prefix's bytes is a different route, and matching it here would hand a
    /// request to a dialect the boot never proved this plane had claimed.
    #[test]
    fn a_one_level_prefix_stops_at_the_segment_boundary() {
        let s = Selector::PrefixOneLevel("/twilio");
        assert!(matches_selector(&s, "/twilio/inbound"));
        assert!(!matches_selector(&s, "/twiliofoo"));
        assert!(!matches_selector(&s, "/twilio"));
        assert!(!matches_selector(&s, "/twilio/inbound/deeper"));
        assert!(!matches_selector(&s, "/other/inbound"));
    }

    /// The test-only claim table the voice conformance harness reads (`claims::DIALECT_CLAIMS`, no
    /// door publishes it) names Twilio Media Streams on the WebSocket transport, one path level
    /// under `/twilio`, under the `webhook-signature` alternative alone, as the door's line does
    /// (the test below holds the line itself).
    #[test]
    fn the_harness_claim_table_names_twilio_on_the_websocket_carrier_under_the_webhook_signature() {
        use crate::claims::{dialect_for, Dialect, DIALECT_CLAIMS, WS_TRANSPORT};

        let twilio: Vec<_> = DIALECT_CLAIMS
            .iter()
            .filter(|c| c.dialect == Dialect::TwilioMediaStreams)
            .collect();
        assert_eq!(
            twilio.len(),
            1,
            "exactly one claim names the Twilio dialect"
        );
        let claim = &twilio[0].claim;
        assert_eq!(
            claim.transport, WS_TRANSPORT,
            "Twilio rides the WebSocket transport"
        );
        assert_eq!(claim.selector, Selector::PrefixOneLevel("/twilio"));
        assert_eq!(claim.scheme_alternatives, &["webhook-signature"]);
        // An arrival one level under `/twilio` names the dialect; the prefix alone and a deeper
        // path do not, and neither does another dialect's path.
        assert_eq!(
            dialect_for("/twilio/inbound"),
            Some(Dialect::TwilioMediaStreams)
        );
        assert_eq!(dialect_for("/twilio"), None);
        assert_eq!(dialect_for("/twilio/a/b"), None);
        assert_eq!(dialect_for("/v1/realtime"), Some(Dialect::OpenaiRealtime));
    }

    /// Twilio Media Streams is a streaming DIALECT carried by the WebSocket transport, not a
    /// transport of its own (the design lists five transports and no telephony one). Its line on
    /// the door's guest list (`plane_door::ROUTES`, the routes the plane publishes) is an upgrade
    /// on the WebSocket transport, one `{call_id}` segment under `/twilio`, and authenticates under
    /// the `webhook-signature` alternative alone: Twilio holds no busbar bearer or API key. (Audit
    /// S83: this was asserted over a test-only claim table no door publishes.)
    #[test]
    fn twilio_media_streams_is_claimed_on_the_websocket_carrier_under_the_webhook_signature() {
        use crate::claims::WS_TRANSPORT;
        use crate::plane_door::{DIALECTS_BY_NAME, ROUTES, SIGNATURE_AUTH};
        use crate::provider::TWILIO_MEDIA_STREAMS;

        let twilio: Vec<_> = ROUTES
            .iter()
            .filter(|r| DIALECTS_BY_NAME[r.dialect as usize] == TWILIO_MEDIA_STREAMS)
            .collect();
        assert_eq!(
            twilio.len(),
            1,
            "exactly one line speaks the Twilio dialect"
        );
        let line = twilio[0];
        assert_eq!(
            line.carrier, WS_TRANSPORT,
            "Twilio rides the WebSocket transport"
        );
        assert!(line.upgrade, "a WebSocket upgrade");
        assert_eq!((line.verb, line.target), ("GET", "/twilio/{call_id}"));
        assert_eq!(line.auth, SIGNATURE_AUTH);
        assert_eq!(
            line.refusal_dialect, line.dialect,
            "refused in its own dialect"
        );
        assert!(!line.ticket, "the carrier takes no busbar ticket");
    }
}

/// What one downlink Twilio audio frame may allocate, and the bytes it must still render.
mod alloc_gate_tests;

/// The bytes a caller's answer body puts on the wire, read as a test's stand-in for the host's reply
/// buffer: a sealed answer through the SDK's test-only driver (`test-seal`).
pub(crate) fn body_bytes(body: crate::driven::Body) -> Vec<u8> {
    match body {
        crate::driven::Body::Public(bytes) => bytes,
        crate::driven::Body::Sealed(mut write) => sealed_bytes(&mut write),
    }
}

/// The bytes a sealed write puts on the wire (`test-seal`).
pub(crate) fn sealed_bytes(write: &mut busbar_contract::abi::sdk::cred::SealedWrite) -> Vec<u8> {
    let mut out = Vec::new();
    let done = busbar_contract::abi::sdk::test_support::chip::drive_sealed::<()>(write, |b| {
        out.extend_from_slice(b);
        std::task::Poll::Ready(Ok(b.len()))
    });
    assert!(done.is_ready(), "the sealed write is taken whole");
    out
}

/// A ticket the host minted, as the SDK hands it to the plane: sealed.
pub(crate) fn sealed_minted(
    ticket: &str,
    expires_at_unix: u64,
) -> busbar_contract::abi::sdk::Minted {
    busbar_contract::abi::sdk::Minted {
        ticket: busbar_contract::abi::sdk::cred::Cred::captured(zeroize::Zeroizing::new(
            ticket.as_bytes().to_vec(),
        )),
        expires_at_unix,
    }
}
