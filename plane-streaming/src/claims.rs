//! The transports the door's routes are carried on ([`WS_TRANSPORT`], [`HTTP_TRANSPORT`]), and a
//! claim table only the tests (and the voice conformance harness) read.
//!
//! WHAT THE SHIPPED PLANE PUBLISHES IS ITS DOOR'S ROUTES (`crate::door::ROUTES`). The table below
//! ([`DIALECT_CLAIMS`]) is not published: no door claims it, and its two one-shot operations
//! (`/v1/audio/*`) are served by no door, so it is compiled for the tests alone. Twilio Media
//! Streams rides the WebSocket carrier and authenticates under the `webhook-signature` alternative
//! alone: Twilio holds no busbar bearer or API key.

#[cfg(test)]
use busbar_contract::grammar::{one_level_under, Claim, Selector};

/// The transport the three WebSocket dialects (`openai-realtime`, `gemini-live`,
/// `twilio-media-streams`) are claimed against.
pub const WS_TRANSPORT: &str = "ws";

/// The transport the two one-shot operations (`transcribe`, `tts`) are claimed against.
pub const HTTP_TRANSPORT: &str = "http";

/// The datagram transport the browser's media rides (row 132): the SDP door's claim, whose session
/// busbar terminates, and the upstream call busbar places for it.
pub const WEBRTC_TRANSPORT: &str = "webrtc";

/// The credential scheme every one of this plane's claims authenticates under.
///
/// One scheme with alternatives, not several schemes, the same discipline `busbar-plane-llm` uses:
/// which alternative a unit narrows to is the authenticate step's answer.
#[cfg(test)]
pub(crate) const SCHEME: &str = "streaming-key";

/// The alternatives a duplex-session unit (`ws`) may narrow to: a bearer token or a vendor API-key
/// header, presented once at session open and cached for the life of the session.
#[cfg(test)]
const WS_SCHEME_ALTS: &[&str] = &["bearer", "api-key"];

/// The one alternative a Twilio Media Streams unit may narrow to: the webhook signature Twilio puts
/// on the WebSocket upgrade (`X-Twilio-Signature`). Twilio holds no busbar credential, so a bearer or
/// API-key alternative here would be either unusable or unauthenticated.
#[cfg(test)]
const TWILIO_SCHEME_ALTS: &[&str] = &["webhook-signature"];

/// The alternatives a one-shot HTTP unit may narrow to — the same two an ordinary API caller uses.
#[cfg(test)]
const HTTP_SCHEME_ALTS: &[&str] = &["bearer", "api-key"];

/// The four dialects this plane's declared claims name.
///
/// This is the open-vocabulary dialect name, kept as a closed Rust enum inside this crate only
/// because every method that switches on it is exhaustive and a fifth dialect is a code change
/// here regardless; nothing about [`busbar_contract::grammar::Claim`] requires a closed set.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// OpenAI Realtime (GA) — WebSocket, PCM16, tool calls, full duplex.
    OpenaiRealtime,
    /// Google Gemini Live (`BidiGenerateContent`) — WebSocket, tool calls.
    GeminiLive,
    /// Twilio Media Streams — WebSocket, G.711 µ-law telephony audio.
    TwilioMediaStreams,
    /// A one-shot speech-to-text request.
    OneShotTranscribe,
    /// A one-shot text-to-speech request.
    OneShotTts,
}

/// One claim with the dialect it names recorded beside it, the same pairing
/// `busbar_plane_llm::claims::LadderClaim` uses for its ladder.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialectClaim {
    /// Which dialect this claim names.
    pub dialect: Dialect,
    /// The claim itself.
    pub claim: Claim,
}

/// Build one claim over a selector, on a transport, under an alternative set.
#[cfg(test)]
const fn claim(
    transport: &'static str,
    selector: Selector,
    alts: &'static [&'static str],
) -> Claim {
    Claim {
        transport,
        selector,
        scheme: Some(SCHEME),
        scheme_alternatives: alts,
        // None of this plane's claims declares an idempotency location: a duplex session has
        // no single request body to key a replay on, and the two one-shot operations are read
        // straight through rather than replay-cached, matching the previous release's behaviour.
        idempotency: None,
    }
}

/// The declared claims, dialect-tagged, in the order the boot's overlap check sees them.
#[cfg(test)]
pub const DIALECT_CLAIMS: &[DialectClaim] = &[
    DialectClaim {
        dialect: Dialect::OpenaiRealtime,
        claim: claim(
            WS_TRANSPORT,
            Selector::PathSuffix("/v1/realtime"),
            WS_SCHEME_ALTS,
        ),
    },
    DialectClaim {
        dialect: Dialect::GeminiLive,
        claim: claim(
            WS_TRANSPORT,
            Selector::PathContains("BidiGenerateContent"),
            WS_SCHEME_ALTS,
        ),
    },
    DialectClaim {
        dialect: Dialect::TwilioMediaStreams,
        claim: claim(
            WS_TRANSPORT,
            Selector::PrefixOneLevel("/twilio"),
            TWILIO_SCHEME_ALTS,
        ),
    },
    DialectClaim {
        dialect: Dialect::OneShotTranscribe,
        claim: claim(
            HTTP_TRANSPORT,
            Selector::PathSuffix("/v1/audio/transcriptions"),
            HTTP_SCHEME_ALTS,
        ),
    },
    DialectClaim {
        dialect: Dialect::OneShotTts,
        claim: claim(
            HTTP_TRANSPORT,
            Selector::PathSuffix("/v1/audio/speech"),
            HTTP_SCHEME_ALTS,
        ),
    },
];

/// Which dialect a request's target names, by walking the claims in declaration order.
///
/// The same walk the kernel runs itself from the declared claims, exposed here so the decode step
/// can name the dialect it is about to read without a second, differently-ordered answer existing
/// anywhere.
#[cfg(test)]
#[must_use]
pub fn dialect_for(path: &str) -> Option<Dialect> {
    DIALECT_CLAIMS
        .iter()
        .find(|c| matches_selector(&c.claim.selector, path))
        .map(|c| c.dialect)
}

/// Whether one selector matches a request path.
///
/// Only the forms this plane's claims actually use are answered.
#[cfg(test)]
pub(crate) fn matches_selector(s: &Selector, path: &str) -> bool {
    match s {
        Selector::PathSuffix(suffix) => path.ends_with(suffix),
        Selector::PathContains(needle) => path.contains(needle),
        // The contract's rule, read rather than restated: a second spelling here could hand a
        // request to a dialect the boot's overlap check never saw this plane claim.
        Selector::PrefixOneLevel(prefix) => one_level_under(prefix, path),
        _ => false,
    }
}
