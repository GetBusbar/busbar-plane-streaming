// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE DOOR'S SESSION CONSTRUCTOR: which live session a unit on a session door opens. Every session
//! door builds its session here, and nowhere else, so the door a unit arrived through decides its
//! codec and its caller-side envelope in one place.

use crate::codec::ir::codec::gemini::GeminiLiveCodec;
use crate::codec::ir::codec::OpenAiRealtimeCodec;
use crate::codec::ir::config::SessionConfig;
use crate::driven::Door;
use crate::session_params::g711_config;
use crate::session_unit::{Plan, SessionUnit};

/// One live session, over the codec its door speaks.
#[derive(Debug)]
pub enum Session {
    /// The OpenAI Realtime dialect: the browser sideband, the plain realtime socket, or the
    /// telephony door behind the Twilio envelope.
    Realtime(SessionUnit<OpenAiRealtimeCodec>),
    /// The Gemini Live dialect.
    Gemini(SessionUnit<GeminiLiveCodec>),
}

/// Open the session a unit on `door` opens: `None` for a door that answers one request. The
/// telephony door is the OpenAI Realtime dialect locked to `g711_ulaw` both ways, behind the Twilio
/// envelope, locked as the served telephony leg locked it; the other session doors lock the
/// operator's session params.
#[must_use]
pub fn open(
    door: Door,
    locked: &SessionConfig,
    now_ns: u64,
    ceiling_secs: Option<u64>,
    id: &str,
    owner: &str,
) -> Option<Session> {
    Some(match door {
        Door::Sideband | Door::Realtime => Session::Realtime(SessionUnit::open_as(
            OpenAiRealtimeCodec,
            locked.clone(),
            false,
            now_ns,
            ceiling_secs,
            id,
            owner,
        )),
        Door::Twilio => Session::Realtime(SessionUnit::open_as(
            OpenAiRealtimeCodec,
            g711_config(),
            true,
            now_ns,
            ceiling_secs,
            id,
            owner,
        )),
        Door::Gemini => Session::Gemini(SessionUnit::open_as(
            GeminiLiveCodec,
            locked.clone(),
            false,
            now_ns,
            ceiling_secs,
            id,
            owner,
        )),
        Door::Mint | Door::Sdp => return None,
    })
}

impl Session {
    /// A piece from the caller.
    #[allow(clippy::wrong_self_convention)]
    pub fn from_caller(&mut self, bytes: &[u8]) -> Plan {
        match self {
            Session::Realtime(s) => s.from_caller(bytes),
            Session::Gemini(s) => s.from_caller(bytes),
        }
    }
}

#[cfg(test)]
#[path = "tests/sessions_tests.rs"]
mod tests;
