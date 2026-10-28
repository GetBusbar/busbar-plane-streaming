// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The names of the dialects this plane speaks, in the order the tail's `dialects` lists them.

/// OpenAI's bidirectional Realtime voice API.
pub const OPENAI_REALTIME: &str = "openai_realtime";

/// Google's Gemini Live `BidiGenerateContent` API.
pub const GEMINI_LIVE: &str = "gemini_live";

/// The telephony dialect a caller arrives in on the telephony door: Twilio Media Streams' JSON
/// event envelope (never dialled: its session's far end speaks [`OPENAI_REALTIME`]).
pub const TWILIO_MEDIA_STREAMS: &str = "twilio_media_streams";
