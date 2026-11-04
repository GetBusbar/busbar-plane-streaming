// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

use busbar_contract::abi::sdk::cred::{Cred, NotJsonText, SealedJson, SealedWrite};

use crate::codec::ir::config::SessionConfig;

/// The prefix of every ticket the plane's mint door answers: the plane's issued-id prefix,
/// declared on its Statement, so the host leads every ticket it mints for the plane with it.
pub const TICKET_PREFIX: &str = "ek_";

/// The lifetime a minted ticket is asked for, in seconds.
pub const TTL_SECS: u64 = 600;

/// The provider's call path, and busbar's own, under which a created call is named.
pub const CALLS_PATH: &str = "/v1/realtime/calls";

/// The media type of a session description.
pub const SDP_CONTENT_TYPE: &str = "application/sdp";

/// The boundary word, written once for both [`FORM_BOUNDARY`] and [`CALL_FORM_TYPE`].
macro_rules! form_boundary {
    () => {
        "busbar-call-form"
    };
}

/// The boundary of the upstream call's form.
pub const FORM_BOUNDARY: &str = form_boundary!();

/// The upstream call's content type: a form under [`FORM_BOUNDARY`].
pub const CALL_FORM_TYPE: &str = concat!("multipart/form-data; boundary=", form_boundary!());

/// The header busbar's call names its caller in, so the provider can attribute the browser's
/// session: the kernel's caller reference, never the principal.
pub const SAFETY_IDENTIFIER_HEADER: &str = "OpenAI-Safety-Identifier";

/// The prefix of a call id, busbar's and the provider's alike.
const CALL_ID_PREFIX: &str = "rtc_";

/// The caller's answer to a mint: the ticket and its expiry, and nothing else, members in name
/// order.
pub fn minted_answer(ticket: &Cred, expires_at_unix: u64) -> Result<SealedWrite, NotJsonText> {
    SealedJson::new()
        .public("expires_at_unix", expires_at_unix)
        .sealed("value", ticket)
        .encoded()
}

/// The record stored with a ticket: the session config its session is locked to.
pub fn ticket_record(locked: &SessionConfig) -> Result<Vec<u8>, String> {
    serde_json::to_vec(locked).map_err(|e| format!("the locked session did not serialize: {e}"))
}

/// The session config a ticket's record locks.
pub fn locked_by(record: &[u8]) -> Result<SessionConfig, String> {
    serde_json::from_slice(record).map_err(|_| "the ticket's record is not a session config".into())
}

/// The upstream call's form ([`CALL_FORM_TYPE`]): its body up to the opened part busbar's own
/// description fills.
pub fn call_form(locked: &SessionConfig) -> Result<Vec<u8>, String> {
    let session = serde_json::to_vec(&serde_json::Value::Object(locked.ga_session()))
        .map_err(|e| format!("the locked session did not serialize: {e}"))?;
    let mut lead = Vec::with_capacity(session.len() + 256);
    lead.extend_from_slice(format!("--{FORM_BOUNDARY}\r\n").as_bytes());
    lead.extend_from_slice(b"Content-Disposition: form-data; name=\"session\"\r\n");
    lead.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
    lead.extend_from_slice(&session);
    lead.extend_from_slice(format!("\r\n--{FORM_BOUNDARY}\r\n").as_bytes());
    lead.extend_from_slice(b"Content-Disposition: form-data; name=\"sdp\"\r\n");
    lead.extend_from_slice(format!("Content-Type: {SDP_CONTENT_TYPE}\r\n\r\n").as_bytes());
    Ok(lead)
}

/// Busbar's own call id, from 16 bytes of the host's randomness.
#[must_use]
pub fn call_id(random: &[u8; 16]) -> String {
    let mut id = String::with_capacity(CALL_ID_PREFIX.len() + 32);
    id.push_str(CALL_ID_PREFIX);
    for b in random {
        id.push_str(&format!("{b:02x}"));
    }
    id
}

/// The `Location` of busbar's call `id`.
#[must_use]
pub fn call_location(id: &str) -> String {
    format!("{CALLS_PATH}/{id}")
}

/// A call id in a `Location`: the last path segment that is one, less any query or fragment.
#[must_use]
pub fn call_id_of(location: &str) -> Option<String> {
    location
        .split(['?', '#'])
        .next()
        .unwrap_or(location)
        .rsplit('/')
        .find(|seg| seg.starts_with(CALL_ID_PREFIX) && seg.len() > CALL_ID_PREFIX.len())
        .filter(|seg| {
            seg.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
        .map(str::to_string)
}

#[cfg(test)]
#[path = "tests/ticket_tests.rs"]
mod tests;
