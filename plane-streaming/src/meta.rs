//! What this plane declares about itself: its meter classes and its one operation class.
//!
//! The meter-class list is the voice row's list (`audio_tokens_in/out`, `text_tokens_in/out`,
//! `audio_seconds_in`, `tool_calls`; `cached_tokens` is attribution only) and the two input classes
//! the providers bill apart from text, `image_tokens_in` and `video_tokens_in` (#77(4): classes are
//! strings the plane declares). It names `audio_seconds_in` and no outbound twin: the downlink is
//! already priced by `audio_tokens_out`.

use busbar_contract::ids::{MeterClassId, OpClassId};

const MS_PER_SECOND: u64 = 1_000;

/// The seconds figure the `audio_seconds_in` class carries, from the milliseconds this plane counts.
/// A part-second rounds UP: audio that arrived is not audio that cost nothing.
#[must_use]
pub const fn audio_seconds_in(ms: u64) -> u64 {
    ms.div_ceil(MS_PER_SECOND)
}

/// The class key the audio a turn RECEIVED is counted under.
pub const CLASS_AUDIO_TOKENS_IN: MeterClassId = MeterClassId::new("audio_tokens_in");

/// The class key the audio a turn EMITTED is counted under.
pub const CLASS_AUDIO_TOKENS_OUT: MeterClassId = MeterClassId::new("audio_tokens_out");

/// The class key the text that PROMPTED a turn is counted under.
pub const CLASS_TEXT_TOKENS_IN: MeterClassId = MeterClassId::new("text_tokens_in");

/// The class key the text a turn EMITTED is counted under.
pub const CLASS_TEXT_TOKENS_OUT: MeterClassId = MeterClassId::new("text_tokens_out");

/// The key that names a cache-read token count: NOT a billable class (it is a subset of the input
/// token classes, and pricing it beside them would double-bill the same cached input). The tests
/// hold that no turn reports it.
#[cfg(test)]
pub const CLASS_CACHED_TOKENS: MeterClassId = MeterClassId::new("cached_tokens");

/// The class key the seconds of inbound audio a turn consumed are counted under.
pub const CLASS_AUDIO_SECONDS_IN: MeterClassId = MeterClassId::new("audio_seconds_in");

/// The class key the image input tokens of a turn are counted under (OpenAI Realtime's
/// `input_token_details.image_tokens`, Gemini Live's IMAGE modality entries).
pub const CLASS_IMAGE_TOKENS_IN: MeterClassId = MeterClassId::new("image_tokens_in");

/// The class key the video input tokens of a turn are counted under (Gemini Live's VIDEO modality
/// entries).
pub const CLASS_VIDEO_TOKENS_IN: MeterClassId = MeterClassId::new("video_tokens_in");

/// The class key a tool call is counted under.
pub const CLASS_TOOL_CALLS: MeterClassId = MeterClassId::new("tool_calls");

/// The operation class a session's opening unit is audited and priced under.
pub const OP_SESSION_OPEN: OpClassId = OpClassId::new("streaming.session.open");
