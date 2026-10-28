// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The two one-shot passes the plane brokers to its provider, what it says and what it reads: the
//! ephemeral client-secret mint a browser session is handed, and the SDP offer it relays.
//!
//! Pure. Nothing here sends a request, reads a clock or holds a key: the caller carries the bytes
//! these functions shape to the provider, and hands the provider's answer back to be read. The real
//! provider key authenticates only the busbar-to-provider hop and never enters anything built or
//! returned here.

use core::fmt;

use busbar_contract::abi::sdk::cred::{Cred, NotJsonText, SealedJson, SealedWrite};

use crate::codec::ir::config::SessionConfig;

/// The provider's client-secret mint path, under its base URL.
pub const CLIENT_SECRETS_PATH: &str = "/v1/realtime/client_secrets";

/// The provider's SDP call path, under its base URL.
pub const CALLS_PATH: &str = "/v1/realtime/calls";

/// The media type of an SDP offer and of the answer relayed back.
pub const SDP_CONTENT_TYPE: &str = "application/sdp";

/// The header a mint request names its caller in, so the provider can attribute the browser session.
pub const SAFETY_IDENTIFIER_HEADER: &str = "OpenAI-Safety-Identifier";

/// The lifetime a minted secret is asked for, in seconds.
pub const TTL_SECS: u64 = 600;

/// The prefix every ephemeral secret the provider mints carries.
const EK_PREFIX: &str = "ek_";

/// The mint request's body: the secret's lifetime ([`TTL_SECS`]), anchored at its creation, and the locked session
/// params the browser's session is bound to, in the GA `session` shape the GA client-secret endpoint
/// reads ([`SessionConfig::ga_session`]: `type` is `realtime`).
///
/// # Errors
/// The serializer's text when the body does not serialize.
pub fn mint_request_body(config: &SessionConfig) -> Result<Vec<u8>, String> {
    let body = serde_json::json!({
        "expires_after": { "anchor": "created_at", "seconds": TTL_SECS },
        "session": serde_json::Value::Object(config.ga_session()),
    });
    serde_json::to_vec(&body).map_err(|e| format!("mint request body did not serialize: {e}"))
}

/// A minted ephemeral secret and when it expires. The secret is a [`Cred`]: no getter, no
/// `Display`, no `Serialize`; this plane only ever hands it on, through [`minted_answer`]. Its
/// `Debug` names the expiry alone.
#[derive(Clone, PartialEq, Eq)]
pub struct Minted {
    /// The `ek_` secret the browser presents.
    pub value: Cred,
    /// When it expires, in Unix seconds, as the provider stated it. Never zero: a secret whose
    /// expiry the provider did not state is refused, not handed to a browser as "expires at 0".
    pub expires_at_unix: u64,
}

impl std::fmt::Debug for Minted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Minted")
            .field("expires_at_unix", &self.expires_at_unix)
            .finish_non_exhaustive()
    }
}

/// The class of a reply that did not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonFailure {
    /// The body could not be read.
    Io,
    /// The body is not well-formed JSON.
    Syntax,
    /// A member is missing or has the wrong type.
    Data,
    /// The body ends before the JSON does.
    Eof,
}

/// Why a reply did not read: the class of the failure and where it sits, never the decoder's own
/// text (a data error quotes the offending value, which may be the credential).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JsonReplyError {
    failure: JsonFailure,
    line: usize,
    column: usize,
}

/// `<the class> (line <l>, column <c>; the decoder's text is withheld)`.
impl fmt::Display for JsonReplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let class = match self.failure {
            JsonFailure::Io => "the body could not be read",
            JsonFailure::Syntax => "it is not well-formed JSON",
            JsonFailure::Data => "a field is missing or has the wrong type",
            JsonFailure::Eof => "it ends before the JSON does",
        };
        write!(
            f,
            "{class} (line {}, column {}; the decoder's text is withheld)",
            self.line, self.column
        )
    }
}

/// Read a JSON reply `body` into `T`. Every [`Cred`] member of `T` is captured as it is read (from
/// the body straight into the credential's zeroizing buffer); the error names a class and a
/// position, never a value.
fn json_reply<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, JsonReplyError> {
    use serde_json::error::Category;
    serde_json::from_slice(body).map_err(|e| JsonReplyError {
        failure: match e.classify() {
            Category::Io => JsonFailure::Io,
            Category::Syntax => JsonFailure::Syntax,
            Category::Data => JsonFailure::Data,
            Category::Eof => JsonFailure::Eof,
        },
        line: e.line(),
        column: e.column(),
    })
}

/// The provider's mint answer, as the SDK reads it: the secret captured straight into a [`Cred`].
#[derive(serde::Deserialize)]
struct ClientSecretResponse {
    value: Cred,
    expires_at: Option<u64>,
}

/// Read a successful mint answer's body.
///
/// # Errors
/// A body that does not parse, a value that is not an `ek_` ephemeral secret, or a secret with no
/// stated expiry, each with the text the caller reports. A body that does not parse is described
/// by its class and position only (`JsonReplyError`): the decoder's own text would quote the
/// value it met, and the body carries the secret.
///
/// The expiry is part of the secret. A browser holds an `ek_` only for as long as it says
/// it lives; one the provider minted without saying when it dies was carried out as
/// `expires_at_unix: 0`, a figure that is not an instant, so a client could neither schedule its
/// refresh nor tell the secret was already past. An answer with no `expires_at`, or `0`, is refused.
pub fn read_minted(body: &[u8]) -> Result<Minted, String> {
    let parsed: ClientSecretResponse =
        json_reply(body).map_err(|e| format!("client-secret response did not parse: {e}"))?;
    if !parsed.value.starts_with(EK_PREFIX.as_bytes()) {
        return Err("client-secret response value is not an ek_ ephemeral secret".into());
    }
    let expires_at_unix = parsed
        .expires_at
        .filter(|at| *at != 0)
        .ok_or("client-secret response states no expiry for the ek_ secret")?;
    Ok(Minted {
        value: parsed.value,
        expires_at_unix,
    })
}

/// The browser's answer to a mint: the secret and its expiry, and nothing else, encoded by the SDK
/// into a sealed write (members in name order, as `serde_json` writes an object).
///
/// # Errors
/// The secret is not UTF-8 text (never, for one read off a JSON answer).
pub fn minted_answer(minted: &Minted) -> Result<SealedWrite, NotJsonText> {
    SealedJson::new()
        .public("expires_at_unix", minted.expires_at_unix)
        .sealed("value", &minted.value)
        .encoded()
}

/// The provider's call id in an SDP answer's `Location`: the last path segment starting `rtc_`, less
/// any query or fragment.
#[must_use]
pub fn rtc_call_id_of(location: &str) -> Option<String> {
    location
        .rsplit('/')
        .find(|seg| seg.starts_with("rtc_"))
        .map(|seg| seg.split(['?', '#']).next().unwrap_or(seg).to_string())
}

#[cfg(test)]
#[path = "tests/broker_tests.rs"]
mod tests;
