// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE `streams:` SECTION — the voice plane's config grammar, and the one place its values are read.
//!
//! ## The section IS the plane
//!
//! `streams:` is the fourth plane noun beside `pools:` / `tools:` / `agents:`, and — like them —
//! there is no `plane:`/`bind:`/`target:` selector: writing a `streams:` block IS declaring the voice
//! plane's configuration. It is a SINGULAR typed section, not a named-definition map: a deployment has
//! ONE live-voice posture, so the section is one object (the locked session defaults + the three
//! session ceilings), never a map of registrations. That is why it is not in `NamedMapSection`.
//!
//! ## The VAD/session grammar is REUSED, not restated
//!
//! Its media/VAD/session shape IS the GA `session` object ([`SessionConfig`], which already carries
//! `turn_detection: Option<IrVad>` with the `server_vad` knobs threshold / prefix_padding_ms /
//! silence_duration_ms / create_response / interrupt_response). The plane adds only the three
//! plane-imposed ceilings — session wall-clock, context window, per-response output tokens — as the
//! sole NEW scalars. No second copy of the VAD grammar exists to drift from the wire one.
//!
//! The session wall-clock ceiling is optional and has no default: absent, a session runs as long
//! as its sockets stay up; present, it is a positive number of seconds and the plane closes a
//! session at it.
//!
//! ## Config-schema tracked set
//!
//! Like `tools:`/`agents:`, this file is fingerprinted by `cargo xtask gate config-schema` (a
//! `SOURCES` entry). Both the `streams` key of `DeployCfg`'s neutral declared-section carrier and
//! this per-key grammar are covered by the additive-only gate, so a deployment's live-voice
//! ceilings cannot be retyped or removed without the gate flagging it.

use crate::codec::ir::config::{IrVad, SessionConfig};
use serde::{Deserialize, Deserializer, Serialize};

/// Context-window ceiling default — 32768 tokens.
fn default_context_window_tokens() -> u32 {
    32_768
}
/// Per-response output-token ceiling default — 4096 tokens.
fn default_max_output_tokens() -> u32 {
    4096
}

/// The locked session defaults an absent `streams.session:` opens with.
///
/// An operator who writes no `turn_detection` gets server VAD with OpenAI's published knobs
/// (threshold 0.5, 300 ms of prefix padding, 500 ms of silence), the same values the IR's
/// `IrVad::ServerVad` takes for a knob a written `server_vad` leaves out.
fn default_session() -> SessionConfig {
    SessionConfig {
        // `Some(Some(..))` — a CONFIGURED detector. The outer `Some` says the operator default names
        // turn detection at all (see `SessionConfig::turn_detection`'s three states).
        turn_detection: Some(Some(IrVad::ServerVad {
            threshold: 0.5,
            prefix_padding_ms: 300,
            silence_duration_ms: 500,
            create_response: true,
            interrupt_response: true,
        })),
        ..SessionConfig::default()
    }
}

/// The keys `streams.session` may name: the members of the GA `session` object the plane models
/// ([`SessionConfig`]). The section is the operator's, read strictly; the same object on the wire
/// is the caller's, read leniently.
pub(crate) const SESSION_KEYS: &[&str] = &[
    "model",
    "modalities",
    "instructions",
    "voice",
    "input_audio_format",
    "output_audio_format",
    "turn_detection",
    "tools",
    "tool_choice",
    "max_output_tokens",
];

/// The keys a `streams.session.turn_detection` of each `type` may name ([`IrVad`]).
pub(crate) const VAD_KEYS: &[(&str, &[&str])] = &[
    (
        "server_vad",
        &[
            "type",
            "threshold",
            "prefix_padding_ms",
            "silence_duration_ms",
            "create_response",
            "interrupt_response",
        ],
    ),
    ("semantic_vad", &["type", "eagerness"]),
];

/// The first key under `streams.session` the plane does not model, as its path; `None` when every
/// key is one it models.
fn unknown_session_key(session: &serde_json::Value) -> Option<String> {
    let session = session.as_object()?;
    if let Some(key) = session.keys().find(|k| !SESSION_KEYS.contains(&k.as_str())) {
        return Some(format!("session.{key}"));
    }
    let vad = session.get("turn_detection")?.as_object()?;
    let ty = vad.get("type")?.as_str()?;
    let (_, keys) = VAD_KEYS.iter().find(|(t, _)| *t == ty)?;
    vad.keys()
        .find(|k| !keys.contains(&k.as_str()))
        .map(|key| format!("session.turn_detection.{key}"))
}

/// `streams.session`, read strictly: a key the plane does not model is refused with its path,
/// never dropped. A typo'd `instruction:` would otherwise open every session with no locked
/// instructions while the governed splice still cut the caller's.
fn strict_session<'de, D: Deserializer<'de>>(d: D) -> Result<SessionConfig, D::Error> {
    use serde::de::Error as _;
    let value = serde_json::Value::deserialize(d)?;
    if let Some(path) = unknown_session_key(&value) {
        return Err(D::Error::custom(format!(
            "streams.{path}: not a key of the locked session"
        )));
    }
    let session = SessionConfig::deserialize(value).map_err(D::Error::custom)?;
    // ONE output modality: OpenAI Realtime GA's `output_modalities` cannot request both, and Gemini
    // Live's `responseModalities` takes one, so any other list would boot a session every far end
    // refuses.
    let one = matches!(session.modalities.as_slice(), [m] if m == "audio" || m == "text");
    if !session.modalities.is_empty() && !one {
        return Err(D::Error::custom(format!(
            "streams.session.modalities: {:?} is not one of [\"audio\"] or [\"text\"] (a realtime \
             session answers in one modality)",
            session.modalities
        )));
    }
    Ok(session)
}

/// THE `streams:` SECTION — the voice plane's owned config. Its VAD/session/media shape IS the GA
/// `session` object ([`SessionConfig`]); the three limits are the only plane-imposed ceilings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)] // a typo'd key is refused HERE exactly as the file refuses it
pub struct StreamsCfg {
    /// The locked session defaults every live session opens with (media formats, voice, instructions,
    /// turn_detection/VAD, tool set, per-response max_output_tokens). Absent ⇒ [`default_session`]
    /// (server_vad, 500ms silence).
    #[serde(default = "default_session", deserialize_with = "strict_session")]
    pub session: SessionConfig,
    /// Hard session wall-clock ceiling, in seconds. OPTIONAL, with no default: absent ⇒ no ceiling.
    /// Zero is refused at parse (a ceiling that closes every session at its open is not a ceiling),
    /// and so is a negative figure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_max_secs: Option<std::num::NonZeroU32>,
    /// Context-window ceiling. Default 32768.
    #[serde(default = "default_context_window_tokens")]
    pub context_window_tokens: u32,
    /// Output-token ceiling per response. Default 4096.
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
    /// The wire model a session's provider dial names, when the catalog entry `session.model`
    /// names sets an `upstream_model` (a model entry's override, else its name). Read from the
    /// `models` section the host hands beside this one ([`crate::plane_door::read_dealt`]), never a
    /// key of `streams:`.
    #[serde(skip)]
    pub upstream_model: Option<String>,
    /// THE BROWSER'S MEDIA LISTENER (row 132): where the SDP door's media associations arrive.
    /// Absent ⇒ no media is served, and the SDP door refuses every offer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaCfg>,
}

/// `streams.media`: the one datagram port busbar terminates the browser's media on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaCfg {
    /// The `ip:port` the port binds; the one candidate every association advertises, so never a
    /// wildcard.
    pub listen: String,
}

impl StreamsCfg {
    /// Why the section's two token ceilings cannot both hold, if they cannot: a per-response output
    /// cap at or above the context window leaves no room for the conversation (the locked session's
    /// context limit after its output, `context_window_tokens - max_output_tokens`, would be 0), so
    /// such a section is refused at load, naming both settings and their values.
    #[must_use]
    pub fn ceilings_conflict(&self) -> Option<String> {
        (self.max_output_tokens >= self.context_window_tokens).then(|| {
            format!(
                "streams.max_output_tokens ({}) must be below streams.context_window_tokens ({}): \
                 the output cap leaves no context window for the conversation",
                self.max_output_tokens, self.context_window_tokens
            )
        })
    }
}

// MANUAL `Default`, not derived: the serde field defaults above are non-trivial (the three ceilings
// and the synthesized `server_vad`), and `#[derive(Default)]` would give `0`/`SessionConfig::default`
// instead — so `StreamsCfg::default()` (what `streams_default_section` returns for an ABSENT section)
// would NOT equal the parse of an empty `streams: {}`. Spelling it by hand keeps those two byte-equal.
impl Default for StreamsCfg {
    fn default() -> Self {
        StreamsCfg {
            session: default_session(),
            session_max_secs: None,
            context_window_tokens: default_context_window_tokens(),
            max_output_tokens: default_max_output_tokens(),
            upstream_model: None,
            media: None,
        }
    }
}

impl StreamsCfg {
    /// `session` as a provider is told it: a `model` that is the session model this section names
    /// carries that model's provider wire name (`upstream_model`; a catalog alias never
    /// reaches a provider). Any other session is unchanged.
    #[must_use]
    pub fn wire_session(&self, mut session: SessionConfig) -> SessionConfig {
        if let (Some(wire), Some(model)) = (&self.upstream_model, &self.session.model) {
            if session.model.as_ref() == Some(model) {
                session.model = Some(wire.clone());
            }
        }
        session
    }
}

#[cfg(test)]
#[path = "tests/config_tests.rs"]
mod tests;
