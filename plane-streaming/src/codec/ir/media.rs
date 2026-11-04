// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Media: the audio formats a session meters in.

/// The negotiated PCM wire format. Two dialect-normalized shapes
/// a session negotiates: signed-16-bit little-endian PCM at 24 kHz (the Realtime default) and G.711
/// µ-law at 8 kHz (the telephony format); and a third a frame's own wire states: signed-16-bit PCM
/// at 16 kHz (the Gemini Live uplink). The plane needs this to turn a byte count into a millisecond position — the
/// barge-in truncate math — because raw PCM carries no timestamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    /// Signed 16-bit little-endian PCM, 24 kHz, mono — the Realtime default (`pcm16`).
    Pcm16,
    /// G.711 µ-law, 8 kHz, mono — the telephony format (`g711_ulaw`).
    G711Ulaw,
    /// Signed 16-bit little-endian PCM, 16 kHz, mono — the rate Gemini Live requires on its uplink
    /// (`audio/pcm;rate=16000`). No Realtime dialect names it, so no session config negotiates it:
    /// it is the format a frame's own wire states, and uplink audio is metered at it.
    Pcm16At16k,
}

impl AudioFormat {
    /// The dialect wire token for this format (`pcm16` / `g711_ulaw`).
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            AudioFormat::Pcm16 => "pcm16",
            AudioFormat::G711Ulaw => "g711_ulaw",

            AudioFormat::Pcm16At16k => "pcm16_16khz",
        }
    }

    /// The GA format object used by `audio.input.format` and `audio.output.format`.
    #[must_use]
    pub fn ga_format(self) -> serde_json::Value {
        match self {
            AudioFormat::Pcm16 => serde_json::json!({ "type": "audio/pcm", "rate": 24_000 }),
            AudioFormat::G711Ulaw => serde_json::json!({ "type": "audio/pcmu" }),
            AudioFormat::Pcm16At16k => serde_json::json!({ "type": "audio/pcm", "rate": 16_000 }),
        }
    }

    /// Parse a dialect wire token into a format, or `None` for an unrecognized one. `g711_alaw` is not supported.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "pcm16" => Some(AudioFormat::Pcm16),
            "g711_ulaw" => Some(AudioFormat::G711Ulaw),
            _ => None,
        }
    }

    /// Bytes of audio per millisecond of playback. pcm16 = 24 samples/ms × 2 bytes = 48; g711 µ-law = 8 bytes/ms; Gemini 16 kHz PCM = 32 bytes/ms.
    #[must_use]
    pub const fn bytes_per_ms(self) -> u64 {
        match self {
            AudioFormat::Pcm16 => 48,

            AudioFormat::G711Ulaw => 8,

            AudioFormat::Pcm16At16k => 32,
        }
    }
}
