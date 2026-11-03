// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The streaming plane's door: the one function a compiled-in row holds and a dropped-in image

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::piece::{self, Owed};
use busbar_contract::abi::hook::REQUEST_STREAM;
use busbar_contract::abi::host::conn::connector::{
    Need, DIRECTION_INBOUND, DIRECTION_OUTBOUND, EGRESS_PROVIDER, KEEP_NAMED,
};
use busbar_contract::abi::mechanism::call::{AbiStr, Blob, OutHead, Outcome, Span};
use busbar_contract::abi::mechanism::door::{
    MarkWord, Section, MARK_WORD_ISSUED_ID_PREFIX, SECTION_CONSUMED, SECTION_DECLARING,
};
use busbar_contract::abi::mechanism::lifecycle::{GenIn, TickIn, TickOut};
use busbar_contract::abi::mechanism::ticket::{CompletionHandle, Ticket};
use busbar_contract::abi::plane::{
    class_of, reason_of, ArriveIn, ArriveOut, BillableClass, DialectAuth, OnPieceIn, OnPieceOut,
    OpClass, OutField, PlaneCancelIn, PlaneCancelOut, PlaneDriveIn, PlaneDriveOut, PlaneOpenIn,
    PlaneSnapshot, PlaneTail, ProjectIn, ProjectOut, RecordWrite, RefusalClass, RefusalCode,
    RefusalIn, RefusalOut, ServeIn, ServeOut, UnitCount, ANSWER_LAYOUT, AUDIT_REJECTED,
    CLAIM_TICKET, EMIT_DONE, EMIT_TO_FAR_END, FROM_CALLER, FROM_FAR_END, FROM_KERNEL,
    INGRESS_DUPLEX_SESSION, INGRESS_REQUEST_RESPONSE, PIECE_CUT, PIECE_HAS_STATUS, PIECE_LAST,
    PIECE_OUT_TEXT, PIECE_REFUSED, PRINCIPAL_REQUIRED, RECORD_AUDIT, ROUTE_DIRECT, SHAPE_PIECEWISE,
    SPAN_ABSENT, TAIL_REFUSED_PIECE, TAP_FAULT_DECODE, UNITS_FLOOR, UNITS_REPORTED,
};
use busbar_contract::abi::sdk::door::{abi_str, statement};
use busbar_contract::abi::sdk::life::Refusal;
use busbar_contract::abi::sdk::plane::{cancel_disposition, UnitCancel};
use busbar_contract::abi::sdk::plane_life::{self as life, PlaneLife};
use busbar_contract::abi::sdk::publish::{ClaimSpec, ServedRoute, SnapshotSpec};
use busbar_contract::abi::sdk::{
    Generations, HostBuf, Instance, Lent, Minted, Out, Safe, SafeSlot, Services, StaticWake, Wake,
};
use busbar_contract::plane::{PER_SESSION, TOKEN_FAMILY};

use crate::claims::{HTTP_TRANSPORT, WEBRTC_TRANSPORT, WS_TRANSPORT};
use crate::codec::ir::config::SessionConfig;
use crate::config::StreamsCfg;
use crate::driven::{Door, Rejected, Reply, FIELD_CONTENT_LENGTH};
use crate::meta;
use crate::provider::{GEMINI_LIVE, OPENAI_REALTIME, TWILIO_MEDIA_STREAMS};
use crate::request_unit::{self, Answer, Piece, RequestUnit};
use crate::session_door::{
    committed_session_config, report_unapplied_rewrite, Live, Sessions, Side, CEILING_TICK_NS,
};

/// The name the plane's Statement carries.
pub const NAME: &str = crate::codec::PLANE_KEY;
/// The version the Statement names: the crate's (a test pins the two equal).
pub const VERSION: &str = "1.6.0";
/// The plane's config section.
pub const SECTION: &str = "streams";
/// The grant kind that admits traffic on this plane: one live session.
pub const SCOPE: &str = "session";
/// The plane's human label.
pub const LABEL: &str = "Streaming";
/// What one live session on this plane is called.
pub const SUBJECT_NOUN: &str = "streaming session";
/// The singular noun for one session, in admin responses.
pub const ADMIN_NOUN: &str = "streaming-session";
/// The record resource kind a session is audited under.
pub const AUDIT_KIND: &str = "streaming_session";
/// The section the plane reads but does not own: the upstream destinations (core's grammar, as the
pub const PROVIDERS_SECTION: &str = "providers";
/// The section the plane reads but does not own: the model catalog, whose entry for the session
pub const MODELS_SECTION: &str = "models";
/// A model entry's provider wire-model override, inside [`MODELS_SECTION`].
pub const UPSTREAM_MODEL_KEY: &str = "upstream_model";
/// Where, inside `streams:`, the upstream a session dials is named.
pub const EGRESS_TARGET: &str = "session.model";
/// The inbound auth style of a keyed door.
pub const KEY_AUTH: &str = "bearer";
/// The inbound auth style of the telephony door: the carrier's signed request.
pub const SIGNATURE_AUTH: &str = "webhook-signature";

/// The mount every door of the plane sits under.
pub const MOUNT_PATH: &str = "/v1/realtime";
/// The metadata URL a refused caller is pointed at (bound, never claimed: 1.5.5 parity).
pub const METADATA_PATH: &str = "/.well-known/oauth-protected-resource/v1/realtime";

/// An absent string.
const NONE: AbiStr = AbiStr::ABSENT;

const SECTIONS: &[Section] = &[
    Section {
        name: abi_str(SECTION),
        flags: SECTION_DECLARING,
        ..Section::ZERO
    },
    Section {
        name: abi_str(PROVIDERS_SECTION),
        flags: SECTION_CONSUMED,
        ..Section::ZERO
    },
    Section {
        name: abi_str(MODELS_SECTION),
        flags: SECTION_CONSUMED,
        ..Section::ZERO
    },
];

/// The dialects, in the order the per-call `dialect` index names them; then the PROVIDER
pub const DIALECTS_BY_NAME: &[&str] = &[
    OPENAI_REALTIME,
    GEMINI_LIVE,
    TWILIO_MEDIA_STREAMS,
    OPENAI_PROVIDER,
    GEMINI_PROVIDER,
];

/// The provider protocol whose realtime dialect is OpenAI Realtime.
pub const OPENAI_PROVIDER: &str = "openai";
/// The provider protocol whose realtime dialect is Gemini Live.
pub const GEMINI_PROVIDER: &str = "gemini";

const DIALECTS: &[AbiStr] = &[
    abi_str(DIALECTS_BY_NAME[0]),
    abi_str(DIALECTS_BY_NAME[1]),
    abi_str(DIALECTS_BY_NAME[2]),
    abi_str(DIALECTS_BY_NAME[3]),
    abi_str(DIALECTS_BY_NAME[4]),
];

/// The OpenAI Realtime far end's credential style (`Authorization: Bearer`).
pub const REALTIME_STYLE: &str = "bearer";
/// The Gemini Live far end's credential style (the `x-goog-api-key` header).
pub const LIVE_STYLE: &str = "x-goog-api-key";

const fn dialect_auth(dialect: u32, style: &'static str) -> DialectAuth {
    DialectAuth {
        dialect,

        style: abi_str(style),
        params: Blob::ABSENT,
        ..DialectAuth::ZERO
    }
}

/// The upstream dialects' default auth styles: each realtime dialect's, and its provider
const DIALECT_AUTH: &[DialectAuth] = &[
    dialect_auth(0, REALTIME_STYLE),
    dialect_auth(1, LIVE_STYLE),
    dialect_auth(3, REALTIME_STYLE),
    dialect_auth(4, LIVE_STYLE),
];

/// The scope kind the kernel's pool guard judges a routed entry by (a key's `allowed_pools`), which
const POOL_SCOPE: &str = "pool";

const SCOPE_KINDS: &[AbiStr] = &[abi_str(SCOPE), abi_str(POOL_SCOPE)];

/// The one operation every door serves: a session open (or the ticket one is opened with).
const OP_CLASSES: &[OpClass] = &[OpClass {
    op: abi_str(meta::OP_SESSION_OPEN.as_str()),
    name: abi_str(LABEL),
}];

const fn class(class: &'static str, family: &'static str) -> BillableClass {
    BillableClass {
        class: abi_str(class),
        family: abi_str(family),
    }
}

/// The eight classes a session counts in (`cached_tokens` is attribution only, never billed), then
const BILLABLE_CLASSES: &[BillableClass] = &[
    class(meta::CLASS_AUDIO_TOKENS_IN.as_str(), TOKEN_FAMILY),
    class(meta::CLASS_AUDIO_TOKENS_OUT.as_str(), TOKEN_FAMILY),
    class(meta::CLASS_TEXT_TOKENS_IN.as_str(), TOKEN_FAMILY),
    class(meta::CLASS_TEXT_TOKENS_OUT.as_str(), TOKEN_FAMILY),
    class(meta::CLASS_AUDIO_SECONDS_IN.as_str(), "duration"),
    class(meta::CLASS_TOOL_CALLS.as_str(), "count"),
    class(meta::CLASS_IMAGE_TOKENS_IN.as_str(), TOKEN_FAMILY),
    class(meta::CLASS_VIDEO_TOKENS_IN.as_str(), TOKEN_FAMILY),
    class(PER_SESSION, "count"),
];

const FEE_UNITS: &[AbiStr] = &[abi_str(PER_SESSION)];

const EGRESS_TARGETS: &[AbiStr] = &[abi_str(EGRESS_TARGET)];

const fn need(direction: u32, transport: &'static str, auth: AbiStr, target: AbiStr) -> Need {
    let mut n = Need::EMPTY;
    n.direction = direction;
    n.transport = abi_str(transport);
    n.auth = auth;
    n.target_from = target;
    n.keep_mode = KEEP_NAMED;
    n
}

/// One outbound need: to a configured provider, its dials judged in the provider egress class, its
const fn outbound(transport: &'static str, style: &'static str) -> Need {
    let mut n = need(DIRECTION_OUTBOUND, transport, abi_str(style), NONE);
    n.egress_class = EGRESS_PROVIDER;
    n
}

/// The response field busbar's call keeps: where the provider names the call it created.
const KEEP_LOCATION: &[AbiStr] = &[abi_str(crate::driven::FIELD_LOCATION)];

/// The settings block the browser's media listener is bound from (`streams.media`).
pub const MEDIA_SETTINGS: &str = "media";

/// The plane's connection needs: its keyed doors and its telephony door inbound; outbound, to the
const NEEDS: &[Need] = &[
    need(
        DIRECTION_INBOUND,
        WS_TRANSPORT,
        abi_str(KEY_AUTH),
        abi_str("listen"),
    ),
    need(
        DIRECTION_INBOUND,
        HTTP_TRANSPORT,
        abi_str(KEY_AUTH),
        abi_str("listen"),
    ),
    need(
        DIRECTION_INBOUND,
        WS_TRANSPORT,
        abi_str(SIGNATURE_AUTH),
        abi_str("listen"),
    ),
    outbound(WS_TRANSPORT, REALTIME_STYLE),
    outbound(WS_TRANSPORT, LIVE_STYLE),
    Need {
        keep_response_headers: KEEP_LOCATION.as_ptr(),
        keep_response_headers_len: KEEP_LOCATION.len(),
        ..outbound(WEBRTC_TRANSPORT, REALTIME_STYLE)
    },
    need(
        DIRECTION_INBOUND,
        WEBRTC_TRANSPORT,
        abi_str(NO_AUTH),
        abi_str(MEDIA_SETTINGS),
    ),
];

/// The need a far request rides, as `OnPieceOut::need` names it (its index in [`NEEDS`] plus one):
pub const RIDES_REALTIME_SOCKET: u32 = 4;
/// Gemini Live's socket.
pub const RIDES_LIVE_SOCKET: u32 = 5;

pub const RIDES_REALTIME_CALL: u32 = 6;

/// The plane's word marks: the issued-id prefix every ticket the host mints for it carries.
const MARK_WORDS: &[MarkWord] = &[MarkWord {
    class: MARK_WORD_ISSUED_ID_PREFIX,
    word: abi_str(crate::ticket::TICKET_PREFIX),
    ..MarkWord::ZERO
}];

/// The Statement tail.
pub const TAIL: &PlaneTail = &{
    let mut t = PlaneTail::EMPTY
        .with_dialects(DIALECTS)
        .with_dialect_auth(DIALECT_AUTH)
        .with_scope_kinds(SCOPE_KINDS)
        .with_op_classes(OP_CLASSES)
        .with_billable_classes(BILLABLE_CLASSES)
        .with_fee_units(FEE_UNITS)
        .with_egress_targets(EGRESS_TARGETS);
    t.ingress = INGRESS_REQUEST_RESPONSE | INGRESS_DUPLEX_SESSION;
    t.dispatch_shape = SHAPE_PIECEWISE;

    t.flags = TAIL_REFUSED_PIECE;

    t.answer_layout = ANSWER_LAYOUT;
    t.scope = abi_str(SCOPE);
    t.label = abi_str(LABEL);
    t.subject_noun = abi_str(SUBJECT_NOUN);
    t.admin_noun = abi_str(ADMIN_NOUN);
    t.audit_kind = abi_str(AUDIT_KIND);

    t
};

/// The inbound auth of the browser's media listener: an association is admitted by the SDP door's
const NO_AUTH: &str = "none";

/// One door a live session is opened through: one line of this plane on a listener's guest list
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    /// The method set: every door takes one method.
    pub verb: &'static str,
    /// The path: exact, or a pattern where `{…}` is one path segment. No door is a prefix, and none
    /// carries a field-presence predicate: no two doors share a path.
    pub target: &'static str,
    /// The transport the line's bytes are read by.
    pub carrier: &'static str,
    /// `true` for an upgrade line: the listener's transport runs the line's auth at `Head` on the
    /// upgrade request, then the connection is handed to [`Route::carrier`].
    pub upgrade: bool,
    /// The default inbound auth style: applied only where the operator's config gives the line no
    /// auth. Every door is keyed or signed: none is read without a credential.
    pub auth: &'static str,
    /// The dialect the line's units speak, by its index in the tail's dialects.
    pub dialect: u32,
    /// The dialect a refusal on this line is rendered in, by the same index.
    pub refusal_dialect: u32,
    /// The line takes a ticket the host minted for the plane, as well as a key (`CLAIM_TICKET`).
    pub ticket: bool,
}

impl Route {
    /// `true` for an exact path, `false` for a pattern.
    #[cfg(test)]
    #[must_use]
    pub const fn exact(&self) -> bool {
        let b = self.target.as_bytes();
        let mut j = 0;
        while j < b.len() {
            if b[j] == b'{' {
                return false;
            }
            j += 1;
        }
        true
    }
}

/// The doors, in the order a snapshot claims them: the ticket mint (one request), the SDP offer (a
pub const ROUTES: &[Route] = &[
    Route {
        verb: "POST",
        target: "/v1/realtime/client_secrets",
        carrier: HTTP_TRANSPORT,
        upgrade: false,
        auth: KEY_AUTH,
        dialect: 0,
        refusal_dialect: 0,
        ticket: false,
    },
    Route {
        verb: "POST",
        target: "/v1/realtime/calls",
        carrier: WEBRTC_TRANSPORT,
        upgrade: false,
        auth: KEY_AUTH,
        dialect: 0,
        refusal_dialect: 0,
        ticket: true,
    },
    Route {
        verb: "GET",
        target: "/v1/realtime/sideband/{call_id}",
        carrier: WS_TRANSPORT,
        upgrade: true,
        auth: KEY_AUTH,
        dialect: 0,
        refusal_dialect: 0,
        ticket: false,
    },
    Route {
        verb: "GET",
        target: "/v1/realtime/gemini/{call_id}",
        carrier: WS_TRANSPORT,
        upgrade: true,
        auth: KEY_AUTH,
        dialect: 1,
        refusal_dialect: 1,
        ticket: false,
    },
    Route {
        verb: "GET",
        target: "/twilio/{call_id}",
        carrier: WS_TRANSPORT,
        upgrade: true,
        auth: SIGNATURE_AUTH,
        dialect: 2,
        refusal_dialect: 2,
        ticket: false,
    },
    Route {
        verb: "GET",
        target: "/v1/realtime",
        carrier: WS_TRANSPORT,
        upgrade: true,
        auth: KEY_AUTH,
        dialect: 0,
        refusal_dialect: 0,
        ticket: false,
    },
];

impl Route {
    /// The door as a generation snapshot claims it.
    #[must_use]
    pub fn claim(&self) -> ClaimSpec {
        let spec = ServedRoute {
            verb: self.verb,
            target: self.target,
            carrier: self.carrier,
            open: false,
        }
        .claim_spec();
        ClaimSpec {
            refusal_dialect: self.refusal_dialect as u16,
            flags: spec.flags | if self.ticket { CLAIM_TICKET } else { 0 },
            ..spec
        }
    }
}

/// Judge a settings blob with the plane's grammar; an empty blob is the default section.
pub fn read_settings(settings: &[u8]) -> Result<StreamsCfg, String> {
    if settings.is_empty() {
        return Ok(StreamsCfg::default());
    }
    checked(serde_json::from_slice::<StreamsCfg>(settings).map_err(|e| e.to_string())?)
}

/// THE SECTION'S OWN RULE, on whichever form handed it: its two token ceilings must both hold.
fn checked(cfg: StreamsCfg) -> Result<StreamsCfg, String> {
    match cfg.ceilings_conflict() {
        Some(reason) => Err(reason),
        None => Ok(cfg),
    }
}

/// The settings the host hands, read: the plane's grammar over its own `streams:` section, with the
pub fn read_dealt(settings: &[u8]) -> Result<StreamsCfg, String> {
    let Ok(serde_json::Value::Object(mut keyed)) =
        serde_json::from_slice::<serde_json::Value>(settings)
    else {
        return read_settings(settings);
    };
    let Some(section) = keyed.remove(SECTION) else {
        return read_settings(settings);
    };
    let mut cfg = if section.is_null() {
        StreamsCfg::default()
    } else {
        checked(serde_json::from_value::<StreamsCfg>(section).map_err(|e| e.to_string())?)?
    };
    cfg.upstream_model = cfg
        .session
        .model
        .as_deref()
        .and_then(|model| {
            keyed
                .get(MODELS_SECTION)?
                .get(model)?
                .get(UPSTREAM_MODEL_KEY)
        })
        .and_then(serde_json::Value::as_str)
        .filter(|wire| !wire.is_empty())
        .map(str::to_owned);
    Ok(cfg)
}

/// ONE READING of the public base URL: parse it and REPLACE the path wholesale, dropping query and
#[must_use]
pub fn absolute(public_url: &str, path: &str) -> Option<String> {
    let mut u = url::Url::parse(public_url).ok()?;
    u.set_path(path);
    u.set_query(None);
    u.set_fragment(None);
    Some(u.to_string())
}

/// One generation's answer: the strings its snapshot names, read once from the public base URL.
pub struct Generation {
    generation: u64,
    audience: String,
    resource_metadata: String,
}

impl Generation {
    /// The generation `generation` of a plane whose deployment states `public_url`.
    #[must_use]
    pub fn build(generation: u64, public_url: Option<&str>) -> Self {
        let (audience, resource_metadata) = public_url
            .and_then(|p| Some((absolute(p, MOUNT_PATH)?, absolute(p, METADATA_PATH)?)))
            .unwrap_or_default();
        Generation {
            generation,
            audience,
            resource_metadata,
        }
    }

    /// Its generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Its snapshot as the plane publishes it: every door claimed and the audience and metadata
    #[must_use]
    pub fn snapshot(&self) -> SnapshotSpec {
        if self.audience.is_empty() {
            return SnapshotSpec::default();
        }
        SnapshotSpec {
            claims: ROUTES.iter().map(Route::claim).collect(),
            audience: Some(self.audience.clone()),
            resource_metadata: Some(self.resource_metadata.clone()),
            ..SnapshotSpec::default()
        }
    }
}

/// The plane's refusal code for an arrival on a claim it does not publish: no door of the plane
pub const REFUSAL_NO_DOOR: u32 = 1;

/// The plane's refusal code for a sideband arrival when [`MAX_PENDING_CALLS`] arrivals already
pub const REFUSAL_CALLS_FULL: u32 = 2;

/// The plane's refusal code for an SDP offer arriving while the instance holds
pub const REFUSAL_OFFERS_FULL: u32 = 3;

/// The plane's refusal code for a request unit (the mint door's) arriving while the instance holds
pub const REFUSAL_UNITS_FULL: u32 = 4;

/// The plane's refusal code for a session (a door that opens one) arriving while the instance
pub const REFUSAL_SESSIONS_FULL: u32 = 5;

/// The status [`REFUSAL_CALLS_FULL`], [`REFUSAL_OFFERS_FULL`], [`REFUSAL_UNITS_FULL`] and
pub const STATUS_CALLS_FULL: u32 = 429;

/// The words an SDP offer refused at [`MAX_BROKERED_CALLS`], a request unit refused at
pub const CALLS_FULL_TEXT: &str =
    "busbar is holding as many realtime calls as it admits at once; retry once one has ended";

/// The most request units an instance keeps state for at once; at it, a new unit is refused at
pub const MAX_UNITS: usize = 4096;

/// The most sideband arrivals whose call is kept until their session opens; past it, a new
pub const MAX_PENDING_CALLS: usize = 4096;

/// The most live calls busbar has placed for its SDP door that an instance keeps the owner of; at
pub const MAX_BROKERED_CALLS: usize = 4096;

/// The most live brokered calls one caller (one `caller_ref`) keeps at once. At it, that caller's
pub const MAX_CALLS_PER_CALLER: usize = MAX_BROKERED_CALLS / 64;

/// The longest an OpenAI Realtime call lives, in milliseconds: OpenAI's published limit, "The
pub const REALTIME_CALL_MAX_MS: u64 = 60 * 60 * 1000;

/// While a brokered row waits for its age to start (no tick has run since it was recorded), the
pub const BROKERED_TICK_MS: u64 = 60 * 1000;

/// One unit on a one-request door, kept from its first piece to the caller's answer.
struct Held {
    /// The plane's answers to the unit's pieces.
    unit: RequestUnit,
    /// What the unit's answer still owes the reply buffer.
    owed: Owed,
    /// An answer that did not fit its field or arena buffer: the re-call carries the same piece,
    short: Option<Answer>,
}

/// The request units in flight, by the kernel's unit key, and the tickets their pieces crossed on,
/// so a cancel on a unit's ticket forgets it.
#[derive(Default)]
struct Units {
    held: BTreeMap<u64, Held>,
    tickets: std::collections::HashMap<Ticket, u64>,
}

impl Units {
    /// Hold `held` for unit `unit`. Answers `false`, holding nothing, at [`MAX_UNITS`]: no unit in
    fn hold(&mut self, unit: u64, held: Held) -> bool {
        if !self.held.contains_key(&unit) && self.held.len() >= MAX_UNITS {
            return false;
        }
        self.held.insert(unit, held);
        true
    }

    /// Whether the table holds [`MAX_UNITS`] units: a new unit is refused at its arrival.
    fn full(&self) -> bool {
        self.held.len() >= MAX_UNITS
    }

    /// A piece of unit `unit` crossed on `ticket`.
    fn crossed(&mut self, unit: u64, ticket: Ticket) {
        if ticket != Ticket::NONE {
            self.tickets.insert(ticket, unit);
        }
    }

    /// Whether the unit on `ticket`, or unit `unit` (`0` = none named), had its far end answer.
    fn far_answered(&self, ticket: Ticket, unit: u64) -> bool {
        self.tickets
            .get(&ticket)
            .copied()
            .into_iter()
            .chain((unit != 0).then_some(unit))
            .any(|u| self.held.get(&u).is_some_and(|h| h.unit.far_answered()))
    }

    /// The op on `ticket` is cancelled, or the kernel cancels unit `unit` itself (`0` = none
    fn cancel(&mut self, ticket: Ticket, unit: u64) -> bool {
        let by_ticket = self.tickets.get(&ticket).copied();
        let mut forgot = false;
        for u in by_ticket.into_iter().chain((unit != 0).then_some(unit)) {
            forgot |= self.forget(u);
        }
        forgot
    }

    /// Unit `unit` is over: it and its tickets are forgotten. Answers whether it was held.
    fn forget(&mut self, unit: u64) -> bool {
        let held = self.held.remove(&unit).is_some();
        self.tickets.retain(|_, u| *u != unit);
        held
    }
}

/// The provider call each sideband arrival names, by the kernel's unit key, kept until the unit's
/// session opens, or the unit ends (refused, cancelled).
#[derive(Default)]
struct PendingCalls {
    pending: BTreeMap<u64, String>,
}

impl PendingCalls {
    /// Keep `call` for unit `unit`. Answers `false`, keeping nothing, when [`MAX_PENDING_CALLS`]
    fn pend(&mut self, unit: u64, call: String) -> bool {
        if self.pending.len() >= MAX_PENDING_CALLS && !self.pending.contains_key(&unit) {
            return false;
        }
        self.pending.insert(unit, call);
        true
    }

    /// Unit `unit`'s session opens: its call, taken.
    fn take(&mut self, unit: u64) -> Option<String> {
        self.pending.remove(&unit)
    }

    /// Unit `unit` ended before its session opened: its call is forgotten. Answers whether one
    fn forget(&mut self, unit: u64) -> bool {
        self.pending.remove(&unit).is_some()
    }
}

/// The live calls busbar placed for its SDP door, by busbar's own call id, each with the caller
/// reference of the unit whose offer placed it (a sideband attaches only to its own caller's call,
/// and may attach again after its sideband closes) and the provider's call id once the provider
/// named it, and each caller's calls, oldest first. A call leaves when its session ends, when the
/// far end ends it (seen through its sideband), when it is older than [`REALTIME_CALL_MAX_MS`] (it
/// has certainly ended), or when its own caller's next call displaces it at [`MAX_CALLS_PER_CALLER`]; never to
/// make room for another caller. At [`MAX_BROKERED_CALLS`] unexpired rows a new offer is refused
/// instead. Expiry is read at each tick, offer and attach, on the door's tick clock.
#[derive(Default)]
struct Brokered {
    owners: HashMap<String, BrokeredRow>,
    per_caller: HashMap<String, VecDeque<String>>,
}

/// One brokered call's row.
struct BrokeredRow {
    /// The caller reference of the unit whose offer created the call.
    owner: String,
    /// The provider's call id, once the provider's answer to busbar's call named it.
    upstream: Option<String>,
    /// The tick clock's reading (milliseconds) when the call was recorded: the last tick before it.
    recorded: u64,
    /// The first tick clock reading greater than `recorded`, stamped by the first `tick` (or offer,
    since: Option<u64>,
}

impl Brokered {
    /// Every row older than [`REALTIME_CALL_MAX_MS`] at `now_ms` (the tick clock) is freed: its call
    fn expire(&mut self, now_ms: u64) {
        let mut expired = Vec::new();
        for (call, row) in &mut self.owners {
            if row.since.is_none() && now_ms > row.recorded {
                row.since = Some(now_ms);
            }
            if row
                .since
                .is_some_and(|since| now_ms.saturating_sub(since) >= REALTIME_CALL_MAX_MS)
            {
                expired.push(call.clone());
            }
        }
        for call in expired {
            self.end(&call);
        }
    }

    /// When the door next needs its clock read (milliseconds on the tick clock), while it holds a
    fn next_tick_ms(&self, now_ms: u64) -> Option<u64> {
        self.owners
            .values()
            .map(|row| match row.since {
                Some(since) => since.saturating_add(REALTIME_CALL_MAX_MS),
                None => now_ms.saturating_add(BROKERED_TICK_MS),
            })
            .min()
    }

    /// Whether a new call may be brokered at `now_ms`: the instance holds fewer than
    fn admits(&mut self, now_ms: u64) -> bool {
        self.expire(now_ms);
        self.owners.len() < MAX_BROKERED_CALLS
    }

    /// The far end created `call` for `caller` at `now_ms`. A caller already holding
    fn record(&mut self, call: String, caller: String, now_ms: u64) {
        self.expire(now_ms);
        let row = BrokeredRow {
            owner: caller.clone(),
            upstream: None,
            recorded: now_ms,
            since: None,
        };
        if let Some(previous) = self.owners.insert(call.clone(), row) {
            self.release(&previous.owner, &call);
        }
        let rows = self.per_caller.entry(caller).or_default();
        while rows.len() >= MAX_CALLS_PER_CALLER {
            match rows.pop_front() {
                Some(oldest) => {
                    self.owners.remove(&oldest);
                }
                None => break,
            }
        }
        rows.push_back(call);
    }

    /// The provider named busbar's call `call` `upstream`.
    fn named(&mut self, call: &str, upstream: String) {
        if let Some(row) = self.owners.get_mut(call) {
            row.upstream = Some(upstream);
        }
    }

    /// The provider's call busbar's call `call` resolves to, when `call` is live at `now_ms`,
    fn upstream_of(&mut self, call: &str, caller: &str, now_ms: u64) -> Option<String> {
        self.expire(now_ms);
        self.owners
            .get(call)
            .filter(|row| row.owner == caller)
            .and_then(|row| row.upstream.clone())
    }

    /// Whether busbar's call `call` is held at all (any caller's).
    fn holds(&self, call: &str) -> bool {
        self.owners.contains_key(call)
    }

    /// The provider ended its call `upstream`: busbar's call that resolves to it is freed.
    fn end_upstream(&mut self, upstream: &str) {
        let call = self
            .owners
            .iter()
            .find(|(_, row)| row.upstream.as_deref() == Some(upstream))
            .map(|(call, _)| call.clone());
        if let Some(call) = call {
            self.end(&call);
        }
    }

    /// Whether `call` is live at `now_ms` and `caller`'s.
    #[cfg(test)]
    fn owns(&mut self, call: &str, caller: &str, now_ms: u64) -> bool {
        self.expire(now_ms);
        self.owners.get(call).is_some_and(|row| row.owner == caller)
    }

    /// `call` ended: its row is freed. Answers whether it was held.
    fn end(&mut self, call: &str) -> bool {
        match self.owners.remove(call) {
            Some(row) => {
                self.release(&row.owner, call);
                true
            }
            None => false,
        }
    }

    /// `call` is no longer `caller`'s.
    fn release(&mut self, caller: &str, call: &str) {
        if let Some(rows) = self.per_caller.get_mut(caller) {
            rows.retain(|c| c != call);
            if rows.is_empty() {
                self.per_caller.remove(caller);
            }
        }
    }
}

/// One live generation: its section, the session params a request unit under it mints over (built
type Published = (Arc<StreamsCfg>, Arc<SessionConfig>, Generation);

/// One open plane instance.
pub struct Plane {
    /// The deployment's public base URL, as `open` read it; a refresh keeps it.
    public_url: Option<String>,
    /// The live generations' settings, the session params a request unit under each mints over
    generations: Mutex<Vec<Published>>,
    /// The published snapshots, held by the SDK until their generation's `retire`.
    snapshots: Generations<PlaneSnapshot>,
    /// The request units in flight.
    units: Mutex<Units>,
    /// Busbar's call each sideband arrival names, by the kernel's unit key, until the unit's
    calls: Mutex<PendingCalls>,
    /// The live calls busbar placed for its SDP door, by owner: a sideband attaches only to its
    brokered: Mutex<Brokered>,
    /// The host services `open` handed the instance: tickets, and the randomness call ids are
    services: Option<Services>,
    /// The live sessions, by the kernel's stream.
    sessions: Mutex<Sessions>,
    /// The host's wake, as `open` handed it; `None` = no session output can be named.
    wake: Option<StaticWake>,
    /// The instance's driver ticket, as its last `tick` was handed it, and that tick's clock
    driver: Mutex<(Ticket, u64)>,
}

/// A ticketless handle: the services the plane calls never pend, so they are called outside any
const UNTICKETED: CompletionHandle = CompletionHandle {
    ticket: Ticket::NONE,
    ..CompletionHandle::ZERO
};

/// Why a mint answered no ticket: the host served none (none is minted, or the host refused).
pub const MINTED_NONE: &str = "the host minted no ticket";

/// The session a browser's session is locked to from `session`: under the section's two token
fn browser_lock(session: SessionConfig, cfg: &StreamsCfg) -> SessionConfig {
    crate::session_door::with_ceilings(cfg.wire_session(session), cfg)
}

impl Plane {
    /// MINT A TICKET (`ticket.mint`) for the principal of the unit this crossing serves, living
    fn mint_ticket(&self, ttl_secs: u64, record: &[u8]) -> Result<Minted, String> {
        let services = self.services.as_ref().ok_or(MINTED_NONE)?;
        services
            .ticket_mint(UNTICKETED, ttl_secs, record)
            .map_err(|_| MINTED_NONE.to_string())
    }

    /// THE ENTRY A MINT'S GRANT MUST REACH: the session's model as the newest generation names it,
    fn mint_entry(&self) -> Option<String> {
        let generations = lock(&self.generations);
        generations
            .last()
            .and_then(|(cfg, _, _)| cfg.session.model.clone())
    }

    /// WHETHER THE UNIT'S KEY IS GRANTED `entry` (`entitlement.check` over the pool scope, the one
    fn mint_granted(&self, entry: &str) -> bool {
        self.services.as_ref().is_none_or(|services| {
            services
                .entitled(UNTICKETED, &format!("{POOL_SCOPE}:{entry}"))
                .unwrap_or(false)
        })
    }

    /// TAKE what admitted the unit this crossing serves (`ticket.take`): `Some` a ticket's record,
    fn take_ticket(&self) -> Result<Option<Vec<u8>>, ()> {
        let services = self.services.as_ref().ok_or(())?;
        let mut room = vec![0u8; busbar_contract::abi::host::service::MAX_TICKET_RECORD as usize];
        services
            .ticket_take(UNTICKETED, &mut room)
            .map(|r| r.map(<[u8]>::to_vec))
            .map_err(|_| ())
    }

    /// The session config the SDP door's session on `stream` is locked to: the one already stated
    fn lock_call(
        &self,
        sessions: &mut Sessions,
        stream: u64,
        cfg: &StreamsCfg,
    ) -> Option<SessionConfig> {
        if let Some(stated) = sessions.rewritten(stream) {
            return Some(browser_lock(stated.clone(), cfg));
        }
        let locked = match self.take_ticket().ok()? {
            Some(record) => crate::ticket::locked_by(&record).ok()?,
            None => cfg.session.clone(),
        };
        let locked = browser_lock(locked, cfg);
        sessions.rewrite(stream, locked.clone()).then_some(locked)
    }

    /// Busbar's own call id: opaque, from the host's randomness.
    fn new_call_id(&self) -> Option<String> {
        let mut random = [0u8; 16];
        self.services
            .as_ref()?
            .random_fill(UNTICKETED, &mut random)
            .ok()?;
        Some(crate::ticket::call_id(&random))
    }

    /// The SDP door's sessions that ended since this was last asked: busbar's calls they placed
    fn end_calls(&self, sessions: &mut Sessions) {
        let ended = sessions.take_ended_calls();
        if !ended.is_empty() {
            let mut brokered = lock(&self.brokered);
            for call in ended {
                brokered.end(&call);
            }
        }
    }

    /// A unit arriving on claim `claim`, under the newest live generation's session params. `None`
    fn held(&self, claim: u32) -> Option<Held> {
        let door = Door::of(claim)?;

        let locked = lock(&self.generations)
            .last()
            .map(|(_, locked, _)| Arc::clone(locked))?;
        let unit = match RequestUnit::open(door, Arc::clone(&locked)) {
            Some(unit) => unit,
            None if door == Door::Sdp => RequestUnit::plain_offer(locked, self.mint_entry()),
            None => return None,
        };
        Some(Held {
            unit,
            owed: Owed::default(),
            short: None,
        })
    }

    /// Unit `unit` is over (refused, or its refusal is about to be told): what the door holds for
    fn end_unit(&self, unit: u64) {
        lock(&self.units).forget(unit);
        lock(&self.calls).forget(unit);
        let mut sessions = lock(&self.sessions);
        sessions.close(unit);
        self.end_calls(&mut sessions);
    }

    /// The tick clock: the instance's last tick's reading, in milliseconds.
    fn now_ms(&self) -> u64 {
        lock(&self.driver).1
    }

    /// A far-end piece with `flags` reached a session on `door` attached to the provider's call
    fn far_end_ended(&self, door: Door, flags: u32, call: Option<&str>, accepted: bool) {
        let ended =
            door == Door::Sideband && accepted && flags & PIECE_LAST != 0 && flags & PIECE_CUT == 0;
        if let (true, Some(call)) = (ended, call) {
            lock(&self.brokered).end_upstream(call);
        }
    }

    /// The newest live generation's section; `None` before the first publish.
    fn newest(&self) -> Option<Arc<StreamsCfg>> {
        lock(&self.generations)
            .last()
            .map(|(cfg, _, _)| Arc::clone(cfg))
    }

    /// Whether any live generation configures a session wall-clock ceiling.
    fn any_ceiling(&self) -> bool {
        lock(&self.generations)
            .iter()
            .any(|(cfg, _, _)| cfg.session_max_secs.is_some())
    }

    /// Name the instance's driver ticket: a session owes output of its own.
    fn wake_driver(&self) {
        let (ticket, _) = *lock(&self.driver);
        if let (Some(wake), false) = (self.wake, ticket == Ticket::NONE) {
            wake.wake(ticket);
        }
    }

    /// Generation `generation` over `cfg`, held live: the snapshot it publishes.
    fn publish(&self, generation: u64, cfg: StreamsCfg) -> life::Published<()> {
        let g = Generation::build(generation, self.public_url.as_deref());
        let spec = g.snapshot();

        let locked =
            crate::session_door::with_ceilings(cfg.wire_session(cfg.session.clone()), &cfg);
        lock(&self.generations).push((Arc::new(cfg), Arc::new(locked), g));
        life::Published { spec, held: () }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One slot body on the SDK's safe surface over the instance's [`Plane`].
macro_rules! slot {
    ($name:ident, $in:ty, $out:ty, |$inst:pat_param, $input:pat_param, $o:ident| $body:block) => {
        struct $name;
        impl SafeSlot for $name {
            type In = $in;
            type Out = $out;
            type State = Plane;
            fn call(
                $inst: Instance<'_, Plane>,
                $input: Lent<'_, $in>,
                #[allow(unused_mut)] mut $o: Out<'_, $out>,
            ) -> Outcome {
                $body
            }
        }
    };
}

impl PlaneLife for Plane {
    type Held = ();

    fn generations(&self) -> &Generations<PlaneSnapshot> {
        &self.snapshots
    }

    fn validate(settings: &[u8]) -> Result<(), Refusal> {
        read_dealt(settings).map(|_| ()).map_err(Refusal::refused)
    }

    fn open(input: &Lent<'_, PlaneOpenIn>) -> Result<life::Opened<Self>, Refusal> {
        let cfg = read_dealt(input.open().settings().bytes()).map_err(Refusal::refused)?;
        let public_url = input
            .public_url()
            .as_str()
            .ok()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let wake = input.open().host().and_then(Wake::of);
        let services = input.open().host().and_then(Services::of);
        let plane = Plane {
            public_url,
            generations: Mutex::new(Vec::new()),
            snapshots: Generations::new(),
            units: Mutex::new(Units::default()),
            calls: Mutex::new(PendingCalls::default()),
            brokered: Mutex::new(Brokered::default()),
            services,
            sessions: Mutex::new(Sessions::default()),
            wake,
            driver: Mutex::new((Ticket::NONE, 0)),
        };
        let first = plane.publish(input.open().generation(), cfg);
        Ok(life::Opened { plane, first })
    }

    fn refresh(&self, settings: &[u8], generation: u64) -> Result<life::Published<()>, Refusal> {
        let cfg = read_dealt(settings).map_err(Refusal::refused)?;
        Ok(self.publish(generation, cfg))
    }

    fn retire(&self, generation: u64) {
        lock(&self.generations).retain(|(_, _, g)| g.generation() != generation);
    }

    fn tick(
        instance: Instance<'_, Self>,
        input: Lent<'_, TickIn>,
        mut out: Out<'_, TickOut>,
    ) -> Outcome {
        let given = input;
        let next = match instance.get() {
            Some(p) => {
                *lock(&p.driver) = (given.head().ticket(), given.now_ns() / 1_000_000);
                let owes = {
                    let mut sessions = lock(&p.sessions);
                    let owes = sessions.tick(given.now_ns());
                    p.end_calls(&mut sessions);
                    owes
                };
                if owes {
                    p.wake_driver();
                }
                let ceiling = if p.any_ceiling() {
                    Some(given.now_ns().saturating_add(CEILING_TICK_NS))
                } else {
                    None
                };
                let now_ms = given.now_ns() / 1_000_000;
                let brokered = {
                    let mut brokered = lock(&p.brokered);
                    brokered.expire(now_ms);
                    brokered
                        .next_tick_ms(now_ms)
                        .map(|ms| ms.saturating_mul(1_000_000))
                };
                match (ceiling, brokered) {
                    (Some(a), Some(b)) => a.min(b),
                    (a, b) => a.or(b).unwrap_or(0),
                }
            }
            None => 0,
        };
        out.set(|o| &o.next_tick_ns, next);
        Outcome::Ready
    }

    fn drive(
        instance: Instance<'_, Self>,
        input: Lent<'_, PlaneDriveIn>,
        mut out: Out<'_, PlaneDriveOut>,
    ) -> Outcome {
        let Some(p) = instance.get() else {
            return Outcome::Ready;
        };
        let sessions = lock(&p.sessions);
        let mut buf = input.sessions_buf();
        for stream in sessions.ready(usize::MAX) {
            buf.push(stream);
        }
        if !buf.fits() {
            out.set(|o| &o.sessions_needed, buf.needed() as u32);
            return Outcome::Failed;
        }
        out.set(|o| &o.sessions_written, buf.written() as u32);
        Outcome::Ready
    }

    fn cancel(
        instance: Instance<'_, Self>,
        input: Lent<'_, PlaneCancelIn>,
        mut out: Out<'_, PlaneCancelOut>,
    ) -> Outcome {
        let mut answered = false;
        if let Some(p) = instance.get() {
            let UnitCancel { ticket, unit } = UnitCancel::of(&input);

            let mut sessions = lock(&p.sessions);
            let mut units = lock(&p.units);
            let session_answered = sessions.far_answered(ticket, unit);
            let unit_answered = units.far_answered(ticket, unit);
            answered = session_answered || unit_answered;
            sessions.cancel(ticket);
            units.cancel(ticket, unit);
            drop(units);
            if unit != 0 {
                lock(&p.calls).forget(unit);
                sessions.close(unit);
            }

            p.end_calls(&mut sessions);
        }
        out.set(|o| &o.cancel.disposition, cancel_disposition(answered));
        Outcome::Ready
    }
}

slot!(Arrive, ArriveIn, ArriveOut, |instance, input, out| {
    let Some(a) = crate::driven::arrive(input.claim()) else {
        out.set(|o| &o.refusal, REFUSAL_NO_DOOR);
        out.set(|o| &o.refusal_status, 404);
        return Outcome::Refused;
    };
    // No admission estimate: a session pays for what the far end reports, so no unit is written.
    out.set(|o| &o.units_written, 0);
    out.set(|o| &o.op_class, a.op_class);
    out.set(|o| &o.dialect, a.dialect);
    // Every door is keyed, signed or ticketed: a unit on it needs a principal.
    out.set(|o| &o.principal_need, PRINCIPAL_REQUIRED);
    // A SESSION on an instance holding as many live sessions as it keeps is refused before its
    // first piece (and before a sideband's call is held for it), in the bound's words: no live
    // session is dropped for it.
    if a.door.is_session() && instance.get().is_some_and(|p| lock(&p.sessions).full()) {
        out.set(|o| &o.refusal, REFUSAL_SESSIONS_FULL);
        out.set(|o| &o.refusal_status, STATUS_CALLS_FULL);
        out.error(CALLS_FULL_TEXT);
        return Outcome::Refused;
    }
    // THE SIDEBAND DOOR controls a call busbar placed: its `{call_id}` is busbar's own. A call
    // busbar does not hold is no door's (a 404). A held call is kept until the unit's session
    // opens, where it is resolved for its own caller only. At the bound the arrival is refused: no
    // pending call is dropped to make room (admission bounds live work, nothing evicts it).
    if a.door == Door::Sideband {
        let Some(p) = instance.get() else {
            return Outcome::Failed;
        };
        let call = input
            .target()
            .as_str()
            .ok()
            .and_then(crate::driven::sideband_call_id)
            .filter(|call| lock(&p.brokered).holds(call));
        let Some(call) = call else {
            out.set(|o| &o.refusal, REFUSAL_NO_DOOR);
            out.set(|o| &o.refusal_status, 404);
            return Outcome::Refused;
        };
        if !lock(&p.calls).pend(input.unit(), call.to_string()) {
            out.set(|o| &o.refusal, REFUSAL_CALLS_FULL);
            out.set(|o| &o.refusal_status, STATUS_CALLS_FULL);
            return Outcome::Refused;
        }
    }
    // An SDP offer on an instance holding as many live calls as it keeps is refused before
    // anything is taken or dialled: no other caller's live call is dropped for it.
    if a.door == Door::Sdp {
        let full = instance
            .get()
            .is_some_and(|p| !lock(&p.brokered).admits(p.now_ms()));
        if full {
            out.set(|o| &o.refusal, REFUSAL_OFFERS_FULL);
            out.set(|o| &o.refusal_status, STATUS_CALLS_FULL);
            out.error(CALLS_FULL_TEXT);
            return Outcome::Refused;
        }
    }
    // A REQUEST UNIT (a door that opens no session) on an instance holding as many units as it
    // keeps is refused before its first piece, in words: no unit in flight is dropped for it.
    if !a.door.is_session() && instance.get().is_some_and(|p| lock(&p.units).full()) {
        out.set(|o| &o.refusal, REFUSAL_UNITS_FULL);
        out.set(|o| &o.refusal_status, STATUS_CALLS_FULL);
        out.error(CALLS_FULL_TEXT);
        return Outcome::Refused;
    }
    // Busbar mints the ticket locally; the request counts
    // against the caller's caps as any admitted call does.
    if a.door == Door::Mint {
        out.local();
        out.counted();
        return Outcome::Ready;
    }
    // The model is read where the newest generation holds it: an arrival copies no section.
    if let Some(p) = instance.get() {
        let generations = lock(&p.generations);
        if let Some(model) = generations
            .last()
            .and_then(|(cfg, _, _)| cfg.session.model.as_deref())
        {
            out.route(ROUTE_DIRECT, model);
        }
    }
    Outcome::Ready
});

/// The piece `input` carries, in the plane's own vocabulary; `None` for a source the kind does not
fn piece_of(input: Lent<'_, OnPieceIn>) -> Option<Piece> {
    let given = input;
    let from = match given.from() {
        FROM_CALLER => request_unit::From::Caller,
        FROM_FAR_END => request_unit::From::FarEnd,
        FROM_KERNEL => request_unit::From::Kernel,
        _ => return None,
    };
    Some(Piece {
        from,
        last: given.flags() & PIECE_LAST != 0,
    })
}

/// Write the head fields `pairs` (name, value) into the host's field buffer over its arena.
fn push_fields(
    fields: &mut HostBuf<'_, OutField>,
    arena: &mut HostBuf<'_, u8>,
    pairs: &[(&'static str, String)],
) {
    for (name, value) in pairs {
        fields.push(OutField {
            name: arena.span(name.as_bytes()),
            value: arena.span(value.as_bytes()),
        });
    }
}

/// Write the plane's `answer` for `held` into the host's buffers, minting the ticket a mint answer
fn answer_piece(
    p: &Plane,
    held: &mut Held,
    answer: Answer,
    input: Lent<'_, OnPieceIn>,
    out: &mut Out<'_, OnPieceOut>,
) -> (Outcome, bool) {
    match answer {
        Answer::Nothing => (Outcome::Ready, false),
        Answer::Refused => (Outcome::Refused, true),
        Answer::Mint { ttl_secs, record } => {
            let answer = match p.mint_entry() {
                Some(entry) if !p.mint_granted(&entry) => held.unit.not_granted(&entry),
                _ => held.unit.minted(p.mint_ticket(ttl_secs, &record)),
            };
            answer_piece(p, held, answer, input, out)
        }
        Answer::ToCaller(reply) => to_caller(held, reply, None, input, out),
        Answer::Rejected(reply, row) => to_caller(held, reply, Some(row), input, out),
    }
}

/// The caller's whole answer `reply`, with the unit's audit row `row` of a refusal the door
fn to_caller(
    held: &mut Held,
    reply: Reply,
    row: Option<Rejected>,
    input: Lent<'_, OnPieceIn>,
    out: &mut Out<'_, OnPieceOut>,
) -> (Outcome, bool) {
    let (mut fields, units, mut arena, mut records) = (
        input.fields_buf(),
        input.units_buf(),
        input.arena_buf(),
        input.records_buf(),
    );

    let mut head = reply.fields.clone();
    head.push((FIELD_CONTENT_LENGTH, reply.body.len().to_string()));
    push_fields(&mut fields, &mut arena, &head);
    if let Some(row) = &row {
        records.push(RecordWrite {
            kind: AUDIT_REJECTED,
            op: RECORD_AUDIT,
            key: arena.span(row.action.as_bytes()),
            value: arena.span(row.resource.as_bytes()),
        });
    }
    if piece::settle(out, &fields, &units, &arena, Some(&records)) {
        held.short = Some(match row {
            Some(row) => Answer::Rejected(reply, row),
            None => Answer::ToCaller(reply),
        });
        return (Outcome::Failed, false);
    }
    out.set(|o| &o.reply_status, u32::from(reply.status));
    let done = held.owed.owe_body(reply.body, EMIT_DONE, input, out);
    (Outcome::Ready, done)
}

/// A live session's piece (`stream != 0`), answered by its [`crate::session_door::Live`]: the
fn session_piece(p: &Plane, input: Lent<'_, OnPieceIn>, out: &mut Out<'_, OnPieceOut>) -> Outcome {
    let given = input;
    let Some(door) = Door::of(given.claim()).filter(|d| d.is_session()) else {
        return Outcome::Refused;
    };
    let Some(cfg) = p.newest() else {
        return Outcome::Refused;
    };
    let now_ms = lock(&p.driver).1;
    let caller = input.caller_ref().as_str().ok().filter(|r| !r.is_empty());
    let mut sessions = lock(&p.sessions);
    let opening = given.from() == FROM_CALLER && sessions.get(given.stream()).is_none();

    let upstream = if opening && door == Door::Sideband {
        let Some(call) = lock(&p.calls).take(given.unit()) else {
            return Outcome::Refused;
        };
        let Some(upstream) =
            caller.and_then(|caller| lock(&p.brokered).upstream_of(&call, caller, now_ms))
        else {
            return Outcome::Refused;
        };
        Some(upstream)
    } else {
        None
    };
    let mut placed: Option<(String, Vec<u8>)> = None;
    if opening && door == Door::Sdp {
        let Some(locked) = p.lock_call(&mut sessions, given.stream(), &cfg) else {
            return Outcome::Refused;
        };
        let (Some(id), Ok(form)) = (p.new_call_id(), crate::ticket::call_form(&locked)) else {
            return Outcome::Refused;
        };
        placed = Some((id, form));
    }

    let live = if given.from() == FROM_CALLER {
        sessions.get_or_open(given.stream(), door, &cfg)
    } else {
        sessions.get(given.stream())
    };
    let Some(live) = live else {
        if given.from() == FROM_CALLER && sessions.full() {
            out.error(CALLS_FULL_TEXT);
        }
        return Outcome::Refused;
    };
    if let Some(upstream) = upstream {
        live.attach(upstream);
    }

    let mut head: Option<(u16, Vec<(&'static str, String)>)> = None;
    if let Some((id, form)) = placed {
        live.open_far(form);
        live.place(id.clone());
        lock(&p.brokered).record(id.clone(), caller.unwrap_or_default().to_string(), now_ms);
        head = Some((
            201,
            vec![(
                crate::driven::FIELD_LOCATION,
                crate::ticket::call_location(&id),
            )],
        ));
    }
    sessions.crossed(given.stream(), given.head().ticket());
    let Some(live) = sessions.get(given.stream()) else {
        return Outcome::Refused;
    };
    let near = given.from() != FROM_FAR_END;

    let owed = if near { &mut live.near } else { &mut live.far };
    if piece::is_recall(input) && owed.pending() {
        owed.pay(input, out);
        return Outcome::Ready;
    }
    let short = if near {
        live.short_near.take()
    } else {
        live.short_far.take()
    };

    if short.is_some() && near && head.is_none() {
        head = live.short_head.take();
    }
    let recorded = head.is_some();
    let emit = match short {
        Some(emit) => emit,
        None if head.is_some() => crate::session_door::Emit::default(),
        None => {
            let caller_last = given.from() == FROM_CALLER && given.flags() & PIECE_LAST != 0;
            let bytes = input.bytes().bytes();
            match given.from() {
                FROM_CALLER => live.from_caller(bytes, caller_last),

                FROM_FAR_END if door == Door::Sdp && given.flags() & PIECE_HAS_STATUS != 0 => {
                    if (200..300).contains(&given.status_code()) {
                        live.answered_call();
                        let upstream = input
                            .head_fields()
                            .iter()
                            .find(|f| {
                                f.name()
                                    .bytes()
                                    .eq_ignore_ascii_case(crate::driven::FIELD_LOCATION.as_bytes())
                            })
                            .and_then(|f| std::str::from_utf8(f.value().bytes()).ok());
                        let upstream = provider_call(upstream);
                        if let (Some(call), Some(upstream)) = (live.placed_call(), upstream) {
                            lock(&p.brokered).named(call, upstream);
                        }
                    } else {
                        live.end();
                    }
                }
                FROM_FAR_END => {
                    if upgrade_accepted(given.flags(), given.status_code(), given.status_class()) {
                        live.accept();
                    }
                    live.from_far_end(bytes);
                    p.far_end_ended(door, given.flags(), live.call_id(), live.accepted());
                }
                FROM_KERNEL => {}
                _ => return Outcome::Refused,
            }

            if caller_last {
                crate::session_door::Emit {
                    frame: None,
                    done: true,
                }
            } else if given.from() == FROM_KERNEL && given.attempt_no() > 0 {
                crate::session_door::Emit::default()
            } else {
                live.next(near)
            }
        }
    };
    let (mut fields, mut units, mut arena) =
        (input.fields_buf(), input.units_buf(), input.arena_buf());

    let floored = live.floored();
    for (class, amount) in live.units() {
        let floor = floored && class == crate::session_unit::TEXT_TOKENS_OUT;
        units.push(UnitCount {
            class,
            source: if floor { UNITS_FLOOR } else { UNITS_REPORTED },
            amount,
        });
    }
    let target = match emit.frame {
        Some((Side::FarEnd, _)) => door
            .route(caller)
            .map(|a| (a.verb, live.dial_target(a.target), a.fields)),
        _ => None,
    };
    let verb = target
        .as_ref()
        .map(|(verb, _, _)| arena.span(verb.as_bytes()));
    let path = target
        .as_ref()
        .map(|(_, path, _)| arena.span(path.as_bytes()));
    if let Some((_, _, pairs)) = &target {
        push_fields(&mut fields, &mut arena, pairs);
    }
    if let Some((_, pairs)) = &head {
        push_fields(&mut fields, &mut arena, pairs);
    }
    if piece::settle(out, &fields, &units, &arena, None) {
        if near {
            live.short_near = Some(emit);
            live.short_head = head;
        } else {
            live.short_far = Some(emit);
        }
        return Outcome::Failed;
    }
    if let Some((status, _)) = head {
        out.set(|o| &o.reply_status, u32::from(status));
    }
    let done = match emit.frame {
        Some((Side::FarEnd, frame)) => {
            if let (Some(verb), Some(path)) = (verb, path) {
                out.set(|o| &o.verb, verb);
                out.set(|o| &o.target, path);
            }
            out.set(|o| &o.need, door.rides());

            let flags = if live.take_opening() {
                EMIT_TO_FAR_END
            } else {
                EMIT_TO_FAR_END | PIECE_OUT_TEXT
            };
            live.near.owe(&frame, flags, input, out);
            false
        }
        Some((Side::Caller, frame)) => {
            let owed = if near { &mut live.near } else { &mut live.far };
            owed.owe(&frame, PIECE_OUT_TEXT, input, out);
            false
        }
        None if emit.done => {
            out.set(|o| &o.flags, EMIT_DONE);
            true
        }
        None => false,
    };

    if live.take_unreadable() {
        out.set(|o| &o.tap_fault, TAP_FAULT_DECODE);
    }
    let owes = live.ready();
    if done {
        sessions.close(given.stream());
        p.end_calls(&mut sessions);
    } else if owes || recorded {
        drop(sessions);
        p.wake_driver();
    }
    Outcome::Ready
}

/// The provider's call a success answering busbar's call names in its `Location`, or `None`, said
fn provider_call(location: Option<&str>) -> Option<String> {
    let call = location.and_then(crate::ticket::call_id_of);
    if call.is_none() {
        tracing::warn!(
            location = location.unwrap_or("(none)"),
            "the provider answered busbar's realtime call with a success naming no call it can be \
             reached by; no sideband can attach to it, and its row is held until it expires"
        );
    }
    call
}

/// Whether a far-end piece with `flags` and status `code` (class `class`) is the far end accepting
fn upgrade_accepted(flags: u32, code: u32, class: u32) -> bool {
    flags & PIECE_HAS_STATUS != 0
        && ((101..300).contains(&code)
            || (code == 0 && class == u32::from(busbar_contract::abi::transport::STATUS_SUCCESS)))
}

slot!(OnPiece, OnPieceIn, OnPieceOut, |instance, input, out| {
    let Some(p) = instance.get() else {
        return Outcome::Failed;
    };
    let given = input;
    // The refused piece (`TAIL_REFUSED_PIECE`): the kernel is about to refuse the unit (its
    // member's credential is gone, or no member is left). What the door holds for it is
    // forgotten; nothing it would emit reaches anyone, so it answers nothing.
    if given.from() == FROM_KERNEL && given.flags() & PIECE_REFUSED != 0 {
        p.end_unit(given.unit());
        if given.stream() != 0 {
            let mut sessions = lock(&p.sessions);
            sessions.close(given.stream());
            p.end_calls(&mut sessions);
        }
        return Outcome::Ready;
    }
    if given.stream() != 0 {
        return session_piece(p, input, &mut out);
    }
    let mut units = lock(&p.units);
    if piece::is_recall(input) {
        if let Some(held) = units.held.get_mut(&given.unit()) {
            if held.owed.pending() {
                if held.owed.pay(input, &mut out) {
                    units.forget(given.unit());
                }
                return Outcome::Ready;
            }
        }
    }
    if !units.held.contains_key(&given.unit()) {
        let Some(held) = p.held(given.claim()) else {
            return Outcome::Refused;
        };
        // At the bound a new unit is refused: no unit in flight is dropped to make room.
        if !units.hold(given.unit(), held) {
            return Outcome::Refused;
        }
    }
    units.crossed(given.unit(), given.head().ticket());
    let Some(held) = units.held.get_mut(&given.unit()) else {
        return Outcome::Failed;
    };
    let answer = match held.short.take() {
        Some(answer) => answer,
        None => {
            let Some(piece) = piece_of(input) else {
                return Outcome::Refused;
            };
            held.unit.on_piece(piece)
        }
    };
    let (outcome, done) = answer_piece(p, held, answer, input, &mut out);
    if done || outcome == Outcome::Refused {
        units.forget(given.unit());
    }
    outcome
});

/// A refusal in the realtime dialects' error shape (the OpenAI REST error envelope, which both
#[must_use]
pub fn refusal_body(status: u16, text: &str) -> Vec<u8> {
    error_envelope(status, text, None)
}

/// The message the realtime dialects answer a bad or missing credential with: the vendor's own copy,
pub const AUTH_FAILURE_MESSAGE: &str = "Incorrect API key provided.";

/// The error code the realtime dialects answer a bad or missing credential with, beside
pub const AUTH_FAILURE_CODE: &str = "invalid_api_key";

/// A refusal the kernel hands the door, rendered by its reason's class (the one classification,
#[must_use]
pub fn refusal_render(status: u16, reason: u32, text: &str) -> Vec<u8> {
    let authentication =
        reason_of(reason).is_some_and(|r| class_of(r) == RefusalClass::Unauthenticated);
    if authentication {
        error_envelope(status, AUTH_FAILURE_MESSAGE, Some(AUTH_FAILURE_CODE))
    } else {
        refusal_body(status, text)
    }
}

/// The OpenAI REST error envelope: `status` read as the error's type, `message` and `code` as given.
fn error_envelope(status: u16, message: &str, code: Option<&str>) -> Vec<u8> {
    let kind = match status {
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        400..=499 => "invalid_request_error",
        _ => "server_error",
    };
    serde_json::to_vec(&serde_json::json!({
        "error": {"message": message, "type": kind, "param": null, "code": code}
    }))
    .unwrap_or_default()
}

/// The response field naming a refusal body's document type.
const FIELD_CONTENT_TYPE: &str = "content-type";
/// A refusal body's document type.
const CONTENT_TYPE_JSON: &str = "application/json";

/// The OpenAI Realtime dialect's index in the tail's dialects.
const REALTIME_DIALECT: u32 = 0;

/// The error code a live session's budget cut is told under.
pub const BUDGET_EXHAUSTED_CODE: &str = "budget_exhausted";

/// A live session's budget cut, told in its stream (where the protocol carries a place to say so,
#[must_use]
pub fn session_cut_render(dialect: u32, text: &str) -> Vec<u8> {
    if dialect != REALTIME_DIALECT {
        return Vec::new();
    }
    serde_json::to_vec(&serde_json::json!({
        "type": "error",
        "error": {
            "type": "invalid_request_error",
            "code": BUDGET_EXHAUSTED_CODE,
            "message": text,
        }
    }))
    .unwrap_or_default()
}

slot!(
    RefusalSlot,
    RefusalIn,
    RefusalOut,
    |instance, input, out| {
        let given = input;
        let status = u16::try_from(given.status()).unwrap_or(0);
        let text = input.text().as_str().unwrap_or_default();
        let cut = given.reason() == RefusalCode::OverBudget as u32
            && given.unit() != 0
            && instance
                .get()
                .is_some_and(|p| lock(&p.sessions).served(given.unit()));
        if let (Some(p), true) = (instance.get(), given.unit() != 0) {
            p.end_unit(given.unit());
        }
        let body = if cut {
            session_cut_render(given.dialect(), text)
        } else {
            refusal_render(status, given.reason(), text)
        };
        let (mut reply, mut fields, mut arena) =
            (input.reply_buf(), input.fields_buf(), input.arena_buf());
        reply.extend(&body);
        fields.push(OutField {
            name: arena.span(FIELD_CONTENT_TYPE.as_bytes()),
            value: arena.span(CONTENT_TYPE_JSON.as_bytes()),
        });
        let short = !(reply.fits() && fields.fits() && arena.fits());
        let (rw, rnd) = reply.settle(short);
        let (fw, fnd) = fields.settle(short);
        let (aw, and) = arena.settle(short);
        out.set(|o| &o.reply_written, rw as u64);
        out.set(|o| &o.reply_needed, rnd as u64);
        out.set(|o| &o.fields_written, fw as u32);
        out.set(|o| &o.fields_needed, fnd as u32);
        out.set(|o| &o.arena_written, aw as u64);
        out.set(|o| &o.arena_needed, and as u64);
        if short {
            Outcome::Failed
        } else {
            Outcome::Ready
        }
    }
);

slot!(Serve, ServeIn, ServeOut, |_, _, _out| { Outcome::Refused });

slot!(Hydrate, GenIn, OutHead, |_, _, _out| { Outcome::Ready });

slot!(Start, GenIn, OutHead, |_, _, _out| { Outcome::Ready });

slot!(Project, ProjectIn, ProjectOut, |instance, input, out| {
    let Some(p) = instance.get() else {
        return Outcome::Failed;
    };
    let given = input;
    let Some(door) = Door::of(given.claim()) else {
        return Outcome::Refused;
    };
    let Some(cfg) = p.newest() else {
        return Outcome::Refused;
    };
    let rewrite = input.rewrite().bytes();
    let mut rewrite_rejection = None;
    let (body, rewritten) = if door.is_session() {
        let mut locked = Live::locked(door, &cfg);
        // The SDP session is locked to its ticket: the hooks are shown the
        // session the ticket's record locks, kept for the session the unit opens. A re-call after
        // a short answer finds it kept (the ticket is taken once): the rewrite is patched over it
        // again, which writes the same params (a patch over its own result changes nothing).
        // Each later hook patches the session the preceding hook accepted, including recalls.
        let stated = lock(&p.sessions).rewritten(given.unit()).cloned();
        if let Some(stated) = stated {
            locked = stated;
        } else if door == Door::Sdp {
            locked = match p.take_ticket() {
                Ok(Some(record)) => match crate::ticket::locked_by(&record) {
                    Ok(ticketed) => ticketed,
                    Err(_) => return Outcome::Refused,
                },
                Ok(None) => locked,
                Err(()) => return Outcome::Refused,
            };
            // Both admission kinds are taken once. Keep key-admitted defaults too, before a
            // short answer or a generation change can make the next crossing take them again.
            if !lock(&p.sessions).rewrite(given.unit(), locked.clone()) {
                out.error(CALLS_FULL_TEXT);
                return Outcome::Refused;
            }
        }
        // Rejection preserves the session already accepted for this unit. Report it only when
        // this projection completes, so a short answer and its recall do not warn twice.
        let applied = if rewrite.is_empty() {
            None
        } else {
            match committed_session_config(&locked, rewrite) {
                Ok(params) => Some(params),
                Err(why) => {
                    rewrite_rejection = Some(why);
                    None
                }
            }
        };
        let rewritten = applied.is_some();
        let params = match applied {
            Some(params) => {
                if !lock(&p.sessions).rewrite(given.unit(), params.clone()) {
                    out.error(CALLS_FULL_TEXT);
                    return Outcome::Refused;
                }
                params
            }
            None => locked,
        };
        let Ok(body) = serde_json::to_vec(&params) else {
            return Outcome::Refused;
        };
        (Some(body), rewritten)
    } else if rewrite.is_empty() {
        (None, false)
    } else {
        return Outcome::Refused;
    };
    let (signals, mut arena, turns) =
        (input.signals_buf(), input.arena_buf(), input.messages_buf());
    let dialect = DIALECTS_BY_NAME
        .get(door.dialect() as usize)
        .map(|d| arena.span(d.as_bytes()));
    let pool = cfg
        .session
        .model
        .as_deref()
        .map(|m| arena.span(m.as_bytes()));
    let body = body.as_deref().map(|b| arena.span(b));
    let short = !(signals.fits() && arena.fits() && turns.fits());
    let (sw, snd) = signals.settle(short);
    let (aw, and) = arena.settle(short);
    let (tw, tnd) = turns.settle(short);
    out.set(|o| &o.signals_needed, snd as u32);
    out.set(|o| &o.arena_written, aw as u64);
    out.set(|o| &o.arena_needed, and as u64);
    out.set(|o| &o.messages_needed, tnd as u32);
    let absent = Span {
        offset: SPAN_ABSENT,
        len: 0,
    };
    out.set(|o| &o.body, absent);
    out.set(|o| &o.rewritten, absent);
    if short {
        // The driver re-calls once with the buffers this named, the same rewrite with it: patching
        // it again over the params kept for the unit writes the same body.
        return Outcome::Failed;
    }
    if let Some(pool) = pool {
        out.host_str(|o| &o.view.pool, &arena, pool);
    }
    if let Some(dialect) = dialect {
        out.host_str(|o| &o.view.ingress_dialect, &arena, dialect);
    }
    out.set(
        |o| &o.view.flags,
        if door.is_session() { REQUEST_STREAM } else { 0 },
    );
    out.host_rows(|o| &o.view.signals, &signals);
    out.set(|o| &o.view.signals_len, sw);
    out.set(|o| &o.prompt.message_count, tw as u64);
    out.host_rows(|o| &o.prompt.messages, &turns);
    out.set(|o| &o.prompt.messages_len, tw);
    if let Some(body) = body {
        out.set(|o| &o.body, body);
        if rewritten {
            out.set(|o| &o.rewritten, body);
        }
    }
    if let Some(why) = rewrite_rejection {
        report_unapplied_rewrite(&why);
    }
    Outcome::Ready
});

busbar_contract::statement_extensions!(
    /// The Statement's extensions: this plugin's whole `declares` document.
    EXTENSIONS: [(
        busbar_contract::abi::mechanism::extensions::EXT_DECLARES,
        include_bytes!("../declares.json")
    )]
);

/// The most calls the kernel keeps in flight on one instance.
const MAX_INFLIGHT: u32 = 64;

/// The Statement: the plane's name and version, its sections, its needs, its tail, its extensions
pub const STATEMENT: busbar_contract::abi::mechanism::door::Statement =
    busbar_contract::abi::mechanism::door::Statement {
        extensions: EXTENSIONS,
        mark_words: MARK_WORDS.as_ptr(),
        mark_words_len: MARK_WORDS.len(),
        ..statement(NAME, VERSION, MAX_INFLIGHT)
            .with_sections(SECTIONS)
            .with_needs(NEEDS)
            .with_kind_tail(TAIL)
    };

busbar_contract::plugin_door! {
    ops: busbar_contract::abi::plane::Ops,
    statement: STATEMENT,
    lifecycle: plane_life(Plane),
    kind_ops: {
        arrive: Safe<Arrive>, on_piece: Safe<OnPiece>, refusal: Safe<RefusalSlot>, serve: Safe<Serve>,
        hydrate: Safe<Hydrate>, start: Safe<Start>, project: Safe<Project>,
    },
}

#[cfg(test)]
#[path = "tests/ladder_tests.rs"]
mod ladder_tests;

#[cfg(test)]
#[path = "tests/plane_door_tests.rs"]
mod plane_door_tests;

const _: () = {
    assert!(NEEDS[RIDES_REALTIME_SOCKET as usize - 1].direction == DIRECTION_OUTBOUND);
    assert!(NEEDS[RIDES_LIVE_SOCKET as usize - 1].direction == DIRECTION_OUTBOUND);
    assert!(NEEDS[RIDES_REALTIME_CALL as usize - 1].direction == DIRECTION_OUTBOUND);
    assert!(DIALECTS.len() == DIALECTS_BY_NAME.len());
};
