// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE GA `session` CONFIG OBJECT — the typed shape carried by `session.update` (client→server) and
//! echoed by `session.created` (server→client). Design `BUSBAR-1.6.0.md` #18/#45.
//!
//! This is the ONE place in the plane where a serde-derived struct models the wire directly, rather
//! than the hand-mapped `serde_json::Value` dispatch the event codec uses. The justification is the
//! LLM-plane precedent: serde-derive is reserved for CONFIG shapes (stable, named, closed field sets),
//! while streaming EVENTS are hand-mapped. The Realtime `session` object is exactly a config shape.
//!
//! This typed config IS the plane's neutral session-config IR — the cross-dialect superset both dialects
//! (OpenAI Realtime + Gemini Live, `BUSBAR-1.6.0.md` #18/#45) read and write, now that the plane
//! has earned a superset at its second wire format. The GA field set is modeled faithfully so a
//! decode→encode round-trip is JSON-stable (opaque `tools` / `tool_choice` ride as `serde_json::Value`;
//! the plane locks and reconciles them but never reshapes them).

use crate::codec::ir::media::AudioFormat;
use serde::{Deserialize, Deserializer, Serialize};

/// Deserialize a PRESENT key into `Some(_)`, so an `Option<Option<T>>` field can tell an absent key
/// (`None`, supplied by `#[serde(default)]` because this function is never called) from an explicit
/// `null` (`Some(None)`). Without it serde consumes a wire `null` at the outer `Option` and both
/// states arrive as `None`.
fn deserialize_some<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

/// THE GA `max_output_tokens` FIELD — either an explicit cap or the `"inf"` sentinel (uncapped). A
/// bespoke (de)serialize keeps the int-or-string wire union without dragging an untagged-enum null
/// ambiguity into the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxOutputTokens {
    /// An explicit output-token ceiling for a response.
    Limit(u32),
    /// The `"inf"` sentinel — no plane-imposed ceiling.
    Inf,
}

impl Serialize for MaxOutputTokens {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            MaxOutputTokens::Limit(n) => s.serialize_u32(*n),
            MaxOutputTokens::Inf => s.serialize_str("inf"),
        }
    }
}

impl<'de> Deserialize<'de> for MaxOutputTokens {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::String(s) if s == "inf" => Ok(MaxOutputTokens::Inf),
            serde_json::Value::Number(n) => n
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .map(MaxOutputTokens::Limit)
                .ok_or_else(|| D::Error::custom("max_output_tokens out of u32 range")),
            other => Err(D::Error::custom(format!(
                "max_output_tokens must be a u32 or \"inf\", got {other}"
            ))),
        }
    }
}

/// serde glue for the optional negotiated audio formats — the enum carries its own dialect tokens
/// (`pcm16` / `g711_ulaw`), so a small module bridges `Option<AudioFormat>` to the wire string.
mod opt_audio_fmt {
    use super::AudioFormat;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(
        v: &Option<AudioFormat>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match v {
            Some(f) => s.serialize_str(f.wire_name()),
            None => s.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<AudioFormat>, D::Error> {
        use serde::de::Error as _;
        match Option::<String>::deserialize(d)? {
            None => Ok(None),
            Some(s) => AudioFormat::from_wire(&s)
                .map(Some)
                .ok_or_else(|| D::Error::custom(format!("unknown audio format: {s}"))),
        }
    }
}

/// THE GA `session` CONFIG OBJECT (`BUSBAR-1.6.0.md` #18/#45). Every field is optional on the wire (a partial
/// `session.update` patches only what it names), so absent keys decode to `None`/empty and are
/// omitted on re-encode — keeping a partial patch JSON-stable. `turn_detection` is the ONE field with
/// THREE wire states rather than two, because GA gives `null` its own meaning: see the field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionConfig {
    /// THE UPSTREAM MODEL ID the session targets. OpenAI Realtime carries this SERVER-SIDE (it appears
    /// on `session.created`, not the writable `session.update` patch), so it stays `None` for the
    /// OpenAI dialect; Gemini Live carries it as `setup.model`. Modeled here as the genuinely-shared
    /// field the SECOND dialect (Gemini) earns into the superset IR (`BUSBAR-1.6.0.md` #18/#45). Optional — an OpenAI
    /// `session.update` omits it (decodes to `None`, skipped on re-encode, so the OpenAI round-trip is
    /// unaffected).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Requested modalities (e.g. `["audio", "text"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modalities: Vec<String>,
    /// System instructions the plane locks (the browser cannot override them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// The synthesis voice (e.g. `alloy`, `marin`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// Negotiated INPUT (uplink) audio format.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_audio_fmt"
    )]
    pub input_audio_format: Option<AudioFormat>,
    /// Negotiated OUTPUT (downlink) audio format — the format the truncate math measures against.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "opt_audio_fmt"
    )]
    pub output_audio_format: Option<AudioFormat>,
    /// Voice-activity-detection config — THREE-STATE, because the GA wire gives each state a
    /// different meaning and a partial `session.update` patches only what it names:
    ///
    /// - `None` — the key was ABSENT. The patch says nothing about turn detection, so re-encoding
    ///   omits the key and whatever the session already had keeps applying.
    /// - `Some(None)` — the key was an explicit `null`. That is GA's "disable VAD"; the client
    ///   drives turn boundaries. Re-encoded as `null`.
    /// - `Some(Some(vad))` — a configured detector, re-encoded verbatim.
    ///
    /// Collapsing absent and `null` into one `None` (which is what a plain `Option` does) makes a
    /// patch that merely renames the voice re-frame upstream with `"turn_detection": null`, which
    /// silently DISABLES server VAD — the upstream then waits for a client-driven turn that a
    /// VAD-expecting client never sends, and the model never answers. The `deserialize_some` shim is
    /// required: serde maps a wire `null` onto the OUTER `Option` by default, which would collapse
    /// the two states again no matter how the field is typed.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_some"
    )]
    pub turn_detection: Option<Option<IrVad>>,
    /// The tool set, carried VERBATIM as opaque JSON (the plane locks the set but never reshapes a
    /// definition — the tool layer (`BUSBAR-1.6.0.md` #18/#45) normalizes call CORRELATION, not the
    /// argument/definition bytes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<serde_json::Value>,
    /// Tool-choice policy (`"auto"` / `"none"` / `"required"` / a forced-call object), opaque.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    /// Per-response output-token ceiling, or `"inf"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<MaxOutputTokens>,
    /// THE CONTEXT-WINDOW CEILING the plane imposes on a live session (`streams.context_window_tokens`),
    /// in tokens. Never read from or written to a wire `session` object under this name: it is the
    /// plane's own ceiling, which each dialect states in its own key when the session is locked (the
    /// OpenAI Realtime `truncation` limit, the Gemini Live `contextWindowCompression` trigger).
    #[serde(skip)]
    pub context_window_tokens: Option<u32>,
}

impl SessionConfig {
    /// THE LOCKED SESSION IN THE GA `session` SHAPE, the shape the GA `/v1/realtime` socket reads
    /// (no `OpenAI-Beta` header): `type` is `realtime`; the requested modalities are
    /// `output_modalities`; the uplink format and turn detection sit under `audio.input`, the
    /// downlink format and the voice under `audio.output`, each format the GA format object
    /// (`{"type":"audio/pcm","rate":24000}`, `{"type":"audio/pcmu"}`). Only what the lock names is
    /// stated, so a member the operator left unset keeps whatever the caller or the far end has.
    #[must_use]
    pub fn ga_session(&self) -> serde_json::Map<String, serde_json::Value> {
        use serde_json::{json, Map, Value};
        let mut s = Map::new();
        s.insert("type".into(), json!("realtime"));
        if let Some(model) = &self.model {
            s.insert("model".into(), json!(model));
        }
        if !self.modalities.is_empty() {
            s.insert("output_modalities".into(), json!(self.modalities));
        }
        if let Some(instructions) = &self.instructions {
            s.insert("instructions".into(), json!(instructions));
        }
        let mut input = Map::new();
        if let Some(format) = self.input_audio_format {
            input.insert("format".into(), format.ga_format());
        }
        if let Some(vad) = &self.turn_detection {
            input.insert(
                "turn_detection".into(),
                serde_json::to_value(vad).unwrap_or(Value::Null),
            );
        }
        let mut output = Map::new();
        if let Some(format) = self.output_audio_format {
            output.insert("format".into(), format.ga_format());
        }
        if let Some(voice) = &self.voice {
            output.insert("voice".into(), json!(voice));
        }
        let mut audio = Map::new();
        if !input.is_empty() {
            audio.insert("input".into(), Value::Object(input));
        }
        if !output.is_empty() {
            audio.insert("output".into(), Value::Object(output));
        }
        if !audio.is_empty() {
            s.insert("audio".into(), Value::Object(audio));
        }
        if !self.tools.is_empty() {
            s.insert("tools".into(), Value::Array(self.tools.clone()));
        }
        if let Some(choice) = &self.tool_choice {
            s.insert("tool_choice".into(), choice.clone());
        }
        if let Some(max) = self.max_output_tokens {
            s.insert(
                "max_output_tokens".into(),
                serde_json::to_value(max).unwrap_or(Value::Null),
            );
        }
        if let Some(limit) = self.context_window_tokens {
            // The GA retention-ratio truncation with a custom limit: the conversation after the
            // instructions is held under the ceiling, less the output the response may still write.
            let out = match self.max_output_tokens {
                Some(MaxOutputTokens::Limit(n)) => n,
                _ => 0,
            };
            s.insert(
                "truncation".into(),
                json!({
                    "type": "retention_ratio",
                    "retention_ratio": 1.0,
                    "token_limits": { "post_instructions": limit.saturating_sub(out) },
                }),
            );
        }
        s
    }
}

/// SEMANTIC-VAD EAGERNESS — how eagerly the model judges end-of-turn. The GA `semantic_vad` knob,
/// a closed set of dialect tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Eagerness {
    /// Waits longest before closing a turn.
    Low,
    /// The middle setting.
    Medium,
    /// Closes a turn quickly.
    High,
    /// Let the model pick.
    Auto,
}

fn default_threshold() -> f32 {
    0.5
}
fn default_prefix_padding_ms() -> u32 {
    300
}
fn default_silence_duration_ms() -> u32 {
    200
}
fn default_true() -> bool {
    true
}

/// VOICE-ACTIVITY-DETECTION config — the plane's neutral VAD surface. Its serde shape IS the GA
/// `turn_detection` object (tagged on `type`): `server_vad` with the threshold/padding/silence knobs,
/// or `semantic_vad` with an eagerness. VAD DISABLED is not a variant here — it is the ABSENCE of a
/// `turn_detection` (a `null` on the wire, `None` at [`SessionConfig::turn_detection`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IrVad {
    /// Server-side voice-activity detection with amplitude threshold and silence windows.
    ServerVad {
        /// Activation threshold (0.0–1.0).
        #[serde(default = "default_threshold")]
        threshold: f32,
        /// Audio (ms) retained BEFORE detected speech, folded into the turn.
        #[serde(default = "default_prefix_padding_ms")]
        prefix_padding_ms: u32,
        /// Trailing silence (ms) that ends a turn.
        #[serde(default = "default_silence_duration_ms")]
        silence_duration_ms: u32,
        /// Whether the server auto-issues `response.create` at end-of-turn.
        #[serde(default = "default_true")]
        create_response: bool,
        /// Whether a new detected turn interrupts an in-flight response (barge-in).
        #[serde(default = "default_true")]
        interrupt_response: bool,
    },
    /// Semantic (model-judged) end-of-turn detection with an eagerness knob.
    SemanticVad {
        /// How eagerly the model closes a turn.
        eagerness: Eagerness,
    },
}
