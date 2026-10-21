//! The transports the door's routes are carried on ([`WS_TRANSPORT`], [`HTTP_TRANSPORT`]), and the
//! claim table the crate's tests read through the plane's test-only meta declaration.
//!
//! WHAT THE SHIPPED PLANE PUBLISHES IS ITS DOOR'S FIVE ROUTES (`crate::door::ROUTES`: the secret
//! mint, the SDP offer, the sideband socket, the Gemini Live socket and the telephony socket), with
//! the dialects its Statement tail names. The table below ([`DIALECT_CLAIMS`]) is not published:
//! no door claims it, and its two one-shot operations (`/v1/audio/*`) are served by no door. It is
//! kept as the tests' fixture of a claim-shaped declaration over the plane's dialects.
//!
//! Two carriers bring those dialects in: the WebSocket carrier (both duplex JSON dialects AND the
//! Twilio telephony dialect) and the request/response carrier (the two one-shot operations). The
//! browser media leg is unclaimed (below).
//!
//! * **Twilio Media Streams rides the WebSocket carrier.** Its wire is one JSON event per WebSocket
//!   message (`connected`/`start`/`media`/`mark`/`dtmf`/`stop`): the carrier frames the messages and
//!   the event envelope is this plane's dialect ([`crate::codec::topology::twilio`]). The design
//!   lists five transports and no telephony one. Twilio
//!   cannot present a busbar bearer or API key, so its claim authenticates under the
//!   `webhook-signature` alternative alone: the `X-Twilio-Signature` HMAC over the request URL,
//!   verified by the auth kind's inbound verify, never by this plane.
//! * **`webrtc`** — no codec surface for the RTP media plane exists anywhere in this crate's closure
//!   (a plane cannot claim a transport it cannot decode frames from without lying about what it
//!   reads). It is unclaimed: a claim is declared independently, and the boot's own overlap check
//!   is what proves the claims stay disjoint when it is added.

use busbar_contract::grammar::{Claim, Selector};

#[cfg(test)]
use busbar_contract::grammar::one_level_under;

/// The transport the three WebSocket dialects (`openai-realtime`, `gemini-live`,
/// `twilio-media-streams`) are claimed against.
pub const WS_TRANSPORT: &str = "ws";

/// The transport the two one-shot operations (`transcribe`, `tts`) are claimed against.
pub const HTTP_TRANSPORT: &str = "http";

/// The credential scheme every one of this plane's claims authenticates under.
///
/// One scheme with alternatives, not several schemes, the same discipline `busbar-plane-llm` uses:
/// which alternative a unit narrows to is the authenticate step's answer.
pub(crate) const SCHEME: &str = "streaming-key";

/// The alternatives a duplex-session unit (`ws`) may narrow to: a bearer token or a vendor API-key
/// header, presented once at session open and cached for the life of the session.
const WS_SCHEME_ALTS: &[&str] = &["bearer", "api-key"];

/// The one alternative a Twilio Media Streams unit may narrow to: the webhook signature Twilio puts
/// on the WebSocket upgrade (`X-Twilio-Signature`). Twilio holds no busbar credential, so a bearer or
/// API-key alternative here would be either unusable or unauthenticated.
const TWILIO_SCHEME_ALTS: &[&str] = &["webhook-signature"];

/// The alternatives a one-shot HTTP unit may narrow to — the same two an ordinary API caller uses.
const HTTP_SCHEME_ALTS: &[&str] = &["bearer", "api-key"];

/// The four dialects this plane's declared claims name.
///
/// This is the open-vocabulary dialect name, kept as a closed Rust enum inside this crate only
/// because every method that switches on it is exhaustive and a fifth dialect is a code change
/// here regardless; nothing about [`busbar_contract::grammar::Claim`] requires a closed set.
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

impl Dialect {
    /// The dialect's own name: the spelling the door's Statement tail names it by
    /// (`crate::door::DIALECTS_BY_NAME`), one spelling for every dialect.
    #[cfg(test)]
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Dialect::OpenaiRealtime => crate::provider::OPENAI_REALTIME,
            Dialect::GeminiLive => crate::provider::GEMINI_LIVE,
            Dialect::TwilioMediaStreams => crate::provider::TWILIO_MEDIA_STREAMS,
            Dialect::OneShotTranscribe => "transcribe",
            Dialect::OneShotTts => "tts",
        }
    }
}

/// One claim with the dialect it names recorded beside it, the same pairing
/// `busbar_plane_llm::claims::LadderClaim` uses for its ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialectClaim {
    /// Which dialect this claim names.
    pub dialect: Dialect,
    /// The claim itself.
    pub claim: Claim,
}

/// Build one claim over a selector, on a transport, under an alternative set.
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

/// The claims, one field narrower, as [`busbar_contract::plane::PlaneMeta::CLAIMS`] wants them.
///
/// Written out rather than derived because a declaration is an associated constant, and a constant
/// cannot run a loop over another constant.
pub const CLAIMS: &[Claim] = &[
    DIALECT_CLAIMS[0].claim,
    DIALECT_CLAIMS[1].claim,
    DIALECT_CLAIMS[2].claim,
    DIALECT_CLAIMS[3].claim,
    DIALECT_CLAIMS[4].claim,
];

/// The two lists cannot drift: one is the other with a field dropped, and a constant cannot loop
/// over a constant, so the equality is asserted at compile time instead of trusted.
const _: () = assert!(CLAIMS.len() == DIALECT_CLAIMS.len());

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
