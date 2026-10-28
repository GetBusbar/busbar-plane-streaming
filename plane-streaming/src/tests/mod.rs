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

    /// Twilio Media Streams is a streaming DIALECT carried by the WebSocket transport, not a
    /// transport of its own (the design lists five transports and no telephony one). Its claim is
    /// on the WebSocket transport, one path level under `/twilio`, and authenticates under the
    /// `webhook-signature` alternative alone: Twilio holds no busbar bearer or API key. Without the
    /// claim the dialect's reader and codec are unreachable: no arrival ever names it.
    #[test]
    fn twilio_media_streams_is_claimed_on_the_websocket_carrier_under_the_webhook_signature() {
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
}

/// Style rules this crate holds itself to, checked rather than merely asserted in prose: no
/// section-sign citation and no parity-binding identifier (a two-letter prefix, a hyphen and
/// digits, e.g. a two-letter code followed by a hyphen and a number) anywhere in this crate's own source — the same hard rule
/// `busbar-contract`'s `feature_invariance` test enforces for its own crate
/// (`crates/busbar-contract/tests/feature_invariance.rs`).
mod style {
    use std::path::{Path, PathBuf};

    fn src_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    fn walk(dir: &Path, f: &mut impl FnMut(&Path, &str)) {
        let entries = std::fs::read_dir(dir).expect("the source directory is readable");
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, f);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("a source file is readable");
                f(&path, &text);
            }
        }
    }

    #[test]
    fn no_section_sign_or_parity_binding_identifier_anywhere_in_source() {
        let mut offenders = Vec::new();
        walk(&src_dir(), &mut |path, text| {
            for (n, line) in text.lines().enumerate() {
                if line.contains('\u{00A7}') {
                    offenders.push(format!("{}:{}: section sign", path.display(), n + 1));
                }
                let bytes = line.as_bytes();
                for i in 0..bytes.len().saturating_sub(4) {
                    if bytes[i] == b'P'
                        && bytes[i + 1] == b'B'
                        && bytes[i + 2] == b'-'
                        && bytes[i + 3].is_ascii_digit()
                    {
                        offenders.push(format!("{}:{}: binding identifier", path.display(), n + 1));
                    }
                }
            }
        });
        assert!(
            offenders.is_empty(),
            "the source cites the design by number rather than in words: {offenders:?}"
        );
    }
}

/// The one counting allocator the crate's alloc gates read: busbar-contract's test-only
/// `alloc-counter` (a dev-dependency feature), so this crate holds no `unsafe` and its root keeps
/// the literal `#![forbid(unsafe_code)]`.
#[global_allocator]
static A: busbar_contract::alloc_counter::Counting =
    busbar_contract::alloc_counter::Counting::new();

/// What one downlink Twilio audio frame may allocate, and the bytes it must still render.
mod alloc_gate_tests;
