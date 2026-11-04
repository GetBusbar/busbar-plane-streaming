//! Test-only wiring: the selector and style checks.

/// What a request path matches, decided the same way the boot's overlap check decides it.
mod selectors {
    /// Twilio Media Streams is a streaming DIALECT carried by the WebSocket transport, not a
    /// transport of its own (the design lists five transports and no telephony one).
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

/// Wire rendering of a downlink Twilio audio frame.
mod twilio_render_tests;

/// The bytes a caller's answer body puts on the wire, read as a test's stand-in for the host's
/// reply buffer: a sealed answer through the SDK's test-only driver (`test-seal`).
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

mod voice_twilio_fixtures;
