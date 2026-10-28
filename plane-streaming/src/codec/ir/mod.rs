// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE PLANE-4 DUPLEX / SESSION VOCABULARY — the plane's OWN nouns (`BUSBAR-1.6.0.md` #18/#45).
//!
//! A live session is a relay: frames pass as they came, under the governed splice, and the plane
//! reads them only to meter. So what is modelled here is what a session locks and meters:
//!
//! | Concern | Module |
//! |---|---|
//! | the session config a session is locked to | [`config`] |
//! | the audio formats uplink audio is metered at | [`media`] |
//! | the usage a far end reports | [`usage`] |
//! | the meter's reads of each dialect ([`codec::OpenAiRealtimeCodec`], [`GeminiLiveCodec`]) | [`codec`] |

pub mod codec;
pub mod config;
pub mod media;
pub mod usage;

pub use codec::gemini::GeminiLiveCodec;
pub use media::AudioFormat;
pub use usage::IrDuplexUsage;
