// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE `streams:` SECTION — the voice plane's config grammar, and the one place its values are read.

use crate::codec::ir::config::{IrVad, MaxOutputTokens, SessionConfig};
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
fn default_session() -> SessionConfig {
    SessionConfig {
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

/// The flat session keys the plane models and translates into provider wire settings.
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

/// The first key under `streams.session` the plane does not model, as its path; `None` when
/// every key is one it models.
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
/// never dropped.
fn strict_session<'de, D: Deserializer<'de>>(d: D) -> Result<SessionConfig, D::Error> {
    use serde::de::Error as _;
    let value = serde_json::Value::deserialize(d)?;
    if let Some(path) = unknown_session_key(&value) {
        return Err(D::Error::custom(format!(
            "streams.{path}: not a key of the locked session"
        )));
    }
    let default_vad = value
        .as_object()
        .is_some_and(|v| !v.contains_key("turn_detection"));
    let mut session = SessionConfig::deserialize(value).map_err(D::Error::custom)?;
    if default_vad {
        session.turn_detection = default_session().turn_detection;
    }

    if session.max_output_tokens == Some(MaxOutputTokens::Limit(0)) {
        return Err(D::Error::custom(
            "streams.session.max_output_tokens: 0 is refused (a response capped at no token \
             cannot answer)",
        ));
    }
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

/// The `streams:` section: flat session settings translated into each provider wire format,
/// together with the plane's resource ceilings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// The configured model's provider protocol, used to reject incompatible session doors.
    #[serde(skip)]
    pub provider_protocol: Option<String>,
    /// The browser media listener where the SDP door's media associations arrive.
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
    /// Whether this door can speak the configured provider's protocol; unknown catalog metadata
    /// is left to the host's provider resolution.
    pub fn supports(&self, door: crate::driven::Door) -> bool {
        use crate::driven::Door;
        matches!(
            (door, self.provider_protocol.as_deref()),
            (Door::Mint, _)
                | (_, None)
                | (Door::Gemini, Some("gemini"))
                | (
                    Door::Sdp | Door::Sideband | Door::Twilio | Door::Realtime,
                    Some("openai")
                )
        )
    }

    /// Why the section's two token ceilings cannot both hold, if they cannot: a per-response output
    /// cap at or above the context window leaves no room for the conversation (the locked session's
    /// context limit after its output, `context_window_tokens - max_output_tokens`, would be 0), so
    /// such a section is refused at load, naming both settings and their values.
    #[must_use]
    pub fn ceilings_conflict(&self) -> Option<String> {
        if self.max_output_tokens == 0 {
            return Some(
                "streams.max_output_tokens: 0 is refused (a response capped at no token cannot \
                 answer)"
                    .to_string(),
            );
        }
        (self.max_output_tokens >= self.context_window_tokens).then(|| {
            format!(
                "streams.max_output_tokens ({}) must be below streams.context_window_tokens ({}): \
                 the output cap leaves no context window for the conversation",
                self.max_output_tokens, self.context_window_tokens
            )
        })
    }
}

impl Default for StreamsCfg {
    fn default() -> Self {
        StreamsCfg {
            session: default_session(),
            session_max_secs: None,
            context_window_tokens: default_context_window_tokens(),
            max_output_tokens: default_max_output_tokens(),
            upstream_model: None,
            provider_protocol: None,
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
