// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The duplex / session vocabulary: the plane's own nouns.

pub mod codec;
pub mod config;
pub mod media;
pub mod usage;

pub use codec::gemini::GeminiLiveCodec;
pub use media::AudioFormat;
pub use usage::IrDuplexUsage;
