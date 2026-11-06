// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Streaming plane behavior through both linked and dropped-in plugin doors.

use busbar_contract::services::{
    Caller, DiskDest, HookAsk, Later, NestAsk, Ran, Reading, RecordsList, Snapshot, Stored,
    TrustKeyRef,
};
use std::mem::zeroed;
use std::sync::Arc;

use busbar_contract::abi::mechanism::call::{AbiStr, Blob, Field, Outcome, Span, BLOB_OCTETS};
use busbar_contract::abi::mechanism::lifecycle::{
    slot as life, GenIn, RefreshIn, TickIn, TickOut, ValidateIn,
};
use busbar_contract::abi::plane::{
    slot, ArriveIn, ArriveOut, OnPieceIn, OnPieceOut, OutField, PlaneOpenIn, PlaneOpenOut,
    PlaneRefreshOut, PlaneSnapshot, RecordWrite, UnitCount, AUDIT_REJECTED, CANCEL_ABORTED,
    CANCEL_OK_PARTIAL, CLAIM_EXACT, CLAIM_OPEN, CLAIM_TICKET, EMIT_DONE, EMIT_TO_FAR_END,
    FROM_CALLER, FROM_FAR_END, FROM_KERNEL, PIECE_HAS_STATUS, PIECE_LAST, PIECE_OUT_TEXT,
    RECORD_AUDIT,
};
use busbar_plane_streaming::door;
use busbar_plugin_loader::dispatch::kinds::plane::Plane;
use busbar_plugin_loader::dispatch::{
    in_head, load_dropped, load_linked, out_head, Bind, DispatchConfig, Dispatcher, Frame,
    LinkedRow, LoadedDoor, NoSink, Plugin,
};

#[path = "support/admission_ticket.rs"]
mod admission_ticket;

fn z<T>() -> T {
    // SAFETY: every `in`/`out` here is plain C data; all-zero is a valid value of each.
    unsafe { zeroed() }
}

/// The bind for one instance, labelled `instance` (two instances never share a label).
fn bind(d: &Dispatcher, instance: &str) -> Bind {
    Bind {
        instance: Arc::from(instance),
        max_inflight_cap: 8,
        sink: Arc::new(NoSink),
        dispatcher: d.adopter(),

        conns: busbar_plugin_loader::dispatch::ConnTable::Probe,
    }
}

/// This plugin's own door, named for the loader.
fn loaded_door() -> LoadedDoor {
    let door: busbar_contract::abi::mechanism::door::DoorFn = door;
    LoadedDoor::from(door)
}

/// The compiled-in row: the door and the Statement rendering it states.
fn row() -> LinkedRow {
    LinkedRow::of_loaded(loaded_door()).expect("the streaming door states itself")
}

/// The Statement tail the door states, read off the door itself: the crate's one public item.
fn tail() -> &'static busbar_contract::abi::plane::PlaneTail {
    use busbar_contract::abi::plane::PlaneTail;
    // SAFETY: the door's `'static` Door, its Statement, and the plane tail it points at.
    #[allow(unused_unsafe)]
    unsafe {
        &*(*door().statement()).kind_tail().cast::<PlaneTail>()
    }
}

/// The need a far request rides, as `OnPieceOut::need` names it: the index plus one of the outbound
/// need on `transport` under the auth style `auth`, read off the door's Statement.
fn rides(transport: &str, auth: &str) -> u32 {
    use busbar_contract::abi::host::conn::connector::DIRECTION_OUTBOUND;
    let read = busbar_contract::abi::mechanism::rendering::read(&row().statement)
        .expect("the Statement reads back");
    let at = read
        .needs
        .iter()
        .position(|n| {
            n.direction == DIRECTION_OUTBOUND && n.transport == transport && n.auth == auth
        })
        .expect("the door states the need");
    u32::try_from(at + 1).expect("a need index")
}

/// The realtime dialects' vendor copy for a bad or missing credential (the logic crate's
/// `plane_door_tests` pins the door's own constants to these words).
const AUTH_FAILURE_MESSAGE: &str = "Incorrect API key provided.";
const AUTH_FAILURE_CODE: &str = "invalid_api_key";

/// The realtime dialects' REST error envelope for a refusal at `status` worded `text`, built as the
/// door builds it (the logic crate's `plane_door_tests` reads the door's `refusal_body` in this
/// shape).
fn refusal_body(status: u16, text: &str) -> Vec<u8> {
    let kind = match status {
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        400..=499 => "invalid_request_error",
        _ => "server_error",
    };
    serde_json::to_vec(&serde_json::json!({
        "error": {"message": text, "type": kind, "param": null, "code": null}
    }))
    .expect("the envelope serializes")
}

/// The audit action for a mint refused because the key is not granted the session model.
const MINT_AUDIT_ACTION: &str = "realtime_ticket.mint";
/// The audit action an SDP offer's refusal is recorded under.
const OFFER_AUDIT_ACTION: &str = "realtime_call.offer";
/// THE SDP DOOR'S ANSWER TO A PLAIN REQUEST (RFC 9110 section 15.6.2, 501 Not Implemented): the offer
/// opens a call session over the WebRTC carrier; this listener does not carry one.
const SDP_PLAIN_REQUEST: &str = r#"{"error":{"code":"session_carrier_required","message":"an SDP offer opens a realtime call over the WebRTC carrier, which this listener does not carry; send the offer to a listener serving WebRTC","param":null,"type":"server_error"}}"#;
const MINT_NOT_GRANTED: &str = r#"{"error":{"code":"not_granted","message":"this key is not granted the session's model `m-cap`, so no ephemeral secret is minted for it","param":null,"type":"permission_error"}}"#;

fn json(b: &'static [u8]) -> Blob {
    Blob::json(b)
}

fn text(s: &'static str) -> AbiStr {
    AbiStr::of(s)
}

fn read(s: AbiStr) -> String {
    if s.is_absent() {
        return String::new();
    }
    // SAFETY: the plugin's text, valid while its generation is live.
    String::from_utf8_lossy(unsafe { s.bytes() }).into_owned()
}

/// A published snapshot, read while its generation is live: its generation, its claims
/// (`verb target carrier exact?`) and the audience and metadata it binds.
fn snapshot(p: *const PlaneSnapshot) -> String {
    assert!(!p.is_null(), "a READY open/refresh publishes a snapshot");
    // SAFETY: the plugin's generation data, valid until `retire` of its generation.
    let s = unsafe { &*p };
    let claims: Vec<String> = (0..s.claims_len)
        .map(|i| {
            // SAFETY: the snapshot names `claims_len` claims.
            let c = unsafe { &*s.claims.add(i) };
            format!(
                "{} {} {}{}{}{}",
                read(c.verb),
                read(c.target),
                read(c.carrier),
                if c.flags & CLAIM_EXACT != 0 {
                    " exact"
                } else {
                    ""
                },
                if c.flags & CLAIM_OPEN != 0 {
                    " open"
                } else {
                    ""
                },
                if c.flags & CLAIM_TICKET != 0 {
                    " ticket"
                } else {
                    ""
                }
            )
        })
        .collect();
    format!(
        "gen={} claims=[{}] audience={} metadata={} routes={}",
        s.generation,
        claims.join(", "),
        read(s.audience),
        read(s.resource_metadata),
        s.admin_routes_len
    )
}

fn linked(d: &Dispatcher) -> Plugin<Plane> {
    load_linked::<Plane>(&row(), bind(d, "linked")).expect("the linked streaming door loads")
}

/// This crate's dropped-in image, the `busbar_plane_streaming_plugin` cdylib `cargo test` builds
/// beside the test (uplifted to `target/<profile>`, or hashed in its `deps`). A missing artifact is
/// a failure, never a skip: this test IS the dropped-in door's proof.
fn dropped(d: &Dispatcher) -> Plugin<Plane> {
    dropped_bound(bind(d, "dropped"))
}

fn dropped_bound(bound: Bind) -> Plugin<Plane> {
    let path = plugin_cdylib();
    let stated = row().statement;
    load_dropped::<Plane>(&path, &stated, bound).expect("the dropped streaming door loads")
}

fn validate(p: &Plugin<Plane>, settings: &'static [u8]) -> Outcome {
    let mut v = Frame::new(
        ValidateIn {
            head: in_head(),
            settings: json(settings),
            err_buf: std::ptr::null_mut(),
            err_cap: 0,
        },
        out_head(),
    );
    p.call(life::VALIDATE, &mut v).outcome
}

fn open(
    p: &Plugin<Plane>,
    generation: u64,
    settings: &'static [u8],
    public_url: Option<&'static str>,
) -> (Outcome, String) {
    let mut i: PlaneOpenIn = z();
    i.open.head = in_head();
    i.open.generation = generation;
    i.open.settings = json(settings);
    if let Some(u) = public_url {
        i.public_url = text(u);
    }
    let mut o: PlaneOpenOut = z();
    o.open.head = out_head();
    let mut f = Frame::new(i, o);
    let c = p.call(life::OPEN, &mut f);
    let snap = if c.outcome == Outcome::Ready {
        snapshot(f.out.snapshot)
    } else {
        String::new()
    };
    (c.outcome, snap)
}

/// THE SCRIPT: the lifecycle over two generations, then one unit offered to the door.
fn script(p: &Plugin<Plane>, public_url: Option<&'static str>) -> Vec<String> {
    let mut t = Vec::new();
    t.push(format!(
        "validate unknown key {:?}",
        validate(p, br#"{"nonsense_key":1}"#)
    ));
    t.push(format!(
        "validate zero ceiling {:?}",
        validate(p, br#"{"session_max_secs":0}"#)
    ));
    t.push(format!("validate empty {:?}", validate(p, b"")));
    t.push(format!(
        "validate ceiling {:?}",
        validate(p, br#"{"session_max_secs":1800}"#)
    ));

    t.push(format!(
        "validate dealt {:?}",
        validate(p, br#"{"streams":{"session":{"model":"m-cap"}}}"#)
    ));
    t.push(format!(
        "validate dealt unknown key {:?}",
        validate(p, br#"{"streams":{"nonsense_key":1}}"#)
    ));

    let (outcome, snap) = open(p, 1, br#"{"session_max_secs":1800}"#, public_url);
    t.push(format!("open {outcome:?} {snap}"));

    let mut r = Frame::new(
        RefreshIn {
            head: in_head(),
            generation: 2,
            settings: json(b"{}"),
            secrets: std::ptr::null(),
            secrets_len: 0,
        },
        PlaneRefreshOut {
            head: out_head(),
            snapshot: std::ptr::null(),
        },
    );
    let c = p.call(life::REFRESH, &mut r);
    t.push(format!(
        "refresh {:?} {}",
        c.outcome,
        snapshot(r.out.snapshot)
    ));

    let mut g = Frame::new(
        GenIn {
            head: in_head(),
            generation: 1,
        },
        out_head(),
    );
    t.push(format!(
        "retire 1 {:?}",
        p.call(life::RETIRE, &mut g).outcome
    ));

    for claim in [3, 6, 9] {
        let mut a: Frame<ArriveIn, ArriveOut> = Frame::new(z(), z());
        a.input.head = in_head();
        a.input.claim = claim;
        a.out.head = out_head();
        let c = p.call(slot::ARRIVE, &mut a);
        t.push(format!(
            "arrive {claim} {:?} dialect={} op={} principal={}",
            c.outcome, a.out.dialect, a.out.op_class, a.out.principal_need
        ));
    }

    let mut k: Frame<TickIn, TickOut> = Frame::new(z(), z());
    k.input.head = in_head();
    k.input.now_ns = 5;
    k.out.head = out_head();
    let c = p.call(life::TICK, &mut k);
    t.push(format!("tick {:?} next={}", c.outcome, k.out.next_tick_ns));

    let mut x: Frame<
        busbar_contract::abi::plane::PlaneCancelIn,
        busbar_contract::abi::plane::PlaneCancelOut,
    > = Frame::new(z(), z());
    x.input.cancel.head = in_head();
    x.out.cancel.head = out_head();
    let c = p.call(life::CANCEL, &mut x);
    t.push(format!(
        "cancel {:?} aborted={}",
        c.outcome,
        x.out.cancel.disposition == CANCEL_ABORTED
    ));
    t
}

const PUBLIC: &str = "https://gw.example.com/ignored?q=1#f";

fn expected(public: bool) -> Vec<String> {
    let doors = "POST /v1/realtime/client_secrets http exact, \
                 POST /v1/realtime/calls webrtc exact ticket, \
                 GET /v1/realtime/sideband/{call_id} ws, GET /v1/realtime/gemini/{call_id} ws, \
                 GET /twilio/{call_id} ws, GET /v1/realtime ws exact";
    let (claims, audience, metadata) = if public {
        (
            doors,
            "https://gw.example.com/v1/realtime",
            "https://gw.example.com/.well-known/oauth-protected-resource/v1/realtime",
        )
    } else {
        ("", "", "")
    };
    let snap = |g: u64| {
        format!("gen={g} claims=[{claims}] audience={audience} metadata={metadata} routes=0")
    };
    vec![
        "validate unknown key Refused".into(),
        "validate zero ceiling Refused".into(),
        "validate empty Ready".into(),
        "validate ceiling Ready".into(),
        "validate dealt Ready".into(),
        "validate dealt unknown key Refused".into(),
        format!("open Ready {}", snap(1)),
        format!("refresh Ready {}", snap(2)),
        "retire 1 Ready".into(),
        "arrive 3 Ready dialect=1 op=0 principal=1".into(),
        "arrive 6 Refused dialect=0 op=0 principal=0".into(),
        "arrive 9 Refused dialect=0 op=0 principal=0".into(),
        "tick Ready next=0".into(),
        "cancel Ready aborted=true".into(),
    ]
}

#[test]
fn the_linked_door_states_the_planes_doors_under_its_public_url() {
    let d = Dispatcher::new(DispatchConfig::default());
    assert_eq!(script(&linked(&d), Some(PUBLIC)), expected(true));
}

#[test]
fn a_deployment_with_no_public_url_is_claimed_by_no_door() {
    let d = Dispatcher::new(DispatchConfig::default());
    assert_eq!(script(&linked(&d), None), expected(false));
}

#[test]
fn the_dropped_door_answers_as_the_linked_door_does() {
    let d = Dispatcher::new(DispatchConfig::default());
    for url in [Some(PUBLIC), None] {
        let dropped = dropped(&d);
        assert_eq!(
            script(&dropped, url),
            script(&linked(&d), url),
            "the two doors must answer identically"
        );
    }
}

/// The words of a REFUSED lifecycle answer.
fn words(error: Option<Vec<u8>>) -> String {
    String::from_utf8(error.expect("a refusal states why")).expect("the words are text")
}

/// A REFUSED `open` or `refresh` says why, in the grammar's words, as `validate` does and as the
/// sibling planes' `open` and `refresh` do; the two doors alike. RED before: both answered a bare
/// REFUSED.
#[test]
fn an_open_or_refresh_the_grammar_refuses_says_why() {
    const BAD: &[u8] = br#"{"nonsense_key":1}"#;
    let d = Dispatcher::new(DispatchConfig::default());
    for door in [linked as fn(&Dispatcher) -> Plugin<Plane>, dropped] {
        let p = door(&d);
        let mut i: PlaneOpenIn = z();
        i.open.head = in_head();
        i.open.generation = 1;
        i.open.settings = json(BAD);
        let mut o: PlaneOpenOut = z();
        o.open.head = out_head();
        let mut f = Frame::new(i, o);
        let c = p.call(life::OPEN, &mut f);
        assert_eq!(c.outcome, Outcome::Refused);
        let said = words(c.error);
        assert!(said.contains("nonsense_key"), "open said: {said}");

        let p = door(&d);
        assert_eq!(open(&p, 1, b"{}", None).0, Outcome::Ready);
        let mut r = Frame::new(
            RefreshIn {
                head: in_head(),
                generation: 2,
                settings: json(BAD),
                secrets: std::ptr::null(),
                secrets_len: 0,
            },
            PlaneRefreshOut {
                head: out_head(),
                snapshot: std::ptr::null(),
            },
        );
        let c = p.call(life::REFRESH, &mut r);
        assert_eq!(c.outcome, Outcome::Refused);
        let said = words(c.error);
        assert!(said.contains("nonsense_key"), "refresh said: {said}");
    }
}

#[test]
fn the_tail_states_what_the_served_plane_declares() {
    let t = tail();
    assert_eq!(read(t.scope), "session");
    assert_eq!(read(t.audit_kind), "streaming_session");
    // SAFETY: the tail's `'static` lists.
    let classes: Vec<String> =
        unsafe { std::slice::from_raw_parts(t.billable_classes_ptr(), t.billable_classes_len()) }
            .iter()
            .map(|c| format!("{}/{}", read(c.class), read(c.family)))
            .collect();
    assert_eq!(
        classes,
        [
            "audio_tokens_in/token",
            "audio_tokens_out/token",
            "text_tokens_in/token",
            "text_tokens_out/token",
            "audio_seconds_in/duration",
            "tool_calls/count",
            "image_tokens_in/token",
            "video_tokens_in/token",
            "per_session/count",
        ]
    );
    // SAFETY: as above.
    let fees: Vec<String> =
        unsafe { std::slice::from_raw_parts(t.fee_units_ptr(), t.fee_units_len()) }
            .iter()
            .map(|f| read(*f))
            .collect();
    assert_eq!(fees, ["per_session"]);
    busbar_contract::abi::plane::check::check_tail(t)
        .expect("the kind's own check admits the tail");
    // The fee unit is one of the billable classes: the plane can
    // report it, and the loader's bind-time check admits it.
    // SAFETY: as above.
    let (fee_units, billable) = unsafe {
        (
            std::slice::from_raw_parts(t.fee_units_ptr(), t.fee_units_len()),
            std::slice::from_raw_parts(t.billable_classes_ptr(), t.billable_classes_len()),
        )
    };
    // SAFETY: the live plane's static tail owns every fee/class string referenced by these
    // slices, including their initialized nested bytes, throughout this validation call.
    unsafe { busbar_contract::abi::plane::check::check_fee_units(fee_units, billable) }
        .expect("every fee unit is a billable class");
}

/// The section the unit script opens under: the session params a mint locks the browser to.
const SESSION: &[u8] = br#"{"session":{"model":"m-cap"}}"#;
/// The kernel's reference for the caller.
const CALLER_REF: &str = "c0ffee";
/// The ticket the services double mints, and when it expires.
const TICKET: &str = "ek_conformance_0001";
const TICKET_EXPIRES: u64 = 1_767_225_600;

fn octets(b: &[u8]) -> Blob {
    // SAFETY: the test's own bytes, which outlive the call each `in` is built for.
    unsafe { Blob::from_raw_parts(b.as_ptr(), b.len(), BLOB_OCTETS, 0) }
}

fn at(buf: &[u8], s: Span) -> String {
    String::from_utf8_lossy(&buf[s.offset as usize..(s.offset + s.len) as usize]).into_owned()
}

/// The host's buffers for one `on_piece`.
struct Bufs {
    reply: Vec<u8>,
    units: Vec<UnitCount>,
    fields: Vec<OutField>,
    arena: Vec<u8>,
    /// The units the last answer wrote (class, source, amount), or, short, the room it needed.
    last_units: String,
    /// The need the last answer named for its far request (`0` = none).
    last_need: u32,
    records: Vec<RecordWrite>,
    /// The flags the last answer stated (`EMIT_*`, `PIECE_OUT_TEXT`).
    last_flags: u32,
    /// The audit rows the last answer wrote (`outcome action resource`), or, short, the room it
    /// needed.
    last_audit: String,
    /// The caller reference the kernel lends each piece.
    caller: &'static str,
    /// The room the fields buffer is lent with (at most its length).
    fields_cap: usize,
}

/// One piece pushed to a unit: whose it is, its flags, bytes, status and kept head fields.
#[derive(Clone, Copy)]
struct Push<'a> {
    unit: u64,
    /// A live session's stream (`0` = a request unit).
    stream: u64,
    claim: u32,
    from: u32,
    flags: u32,
    attempt_no: u32,
    bytes: &'a [u8],
    status: u32,
    head: &'a [Field],
}

impl Bufs {
    fn new(reply_cap: usize) -> Self {
        Self::with_units(reply_cap, 2)
    }

    fn with_units(reply_cap: usize, units: usize) -> Self {
        Self {
            reply: vec![0; reply_cap],
            units: vec![z(); units],
            fields: vec![z(); 4],
            arena: vec![0; 256],
            last_units: String::new(),
            last_need: 0,
            records: vec![z(); 2],
            last_flags: 0,
            last_audit: String::new(),
            caller: CALLER_REF,
            fields_cap: 4,
        }
    }

    fn frame(&mut self, push: &Push<'_>) -> Frame<OnPieceIn, OnPieceOut> {
        let mut i: OnPieceIn = z();
        i.head = in_head();
        (i.unit, i.claim, i.from, i.flags) = (push.unit, push.claim, push.from, push.flags);
        i.stream = push.stream;
        (i.attempt_no, i.bytes, i.status_code) = (push.attempt_no, octets(push.bytes), push.status);
        i.caller_ref = text(self.caller);
        if !push.head.is_empty() {
            (i.head_fields, i.head_fields_len) = (push.head.as_ptr(), push.head.len());
        }
        (i.reply_buf, i.reply_cap) = (self.reply.as_mut_ptr(), self.reply.len());
        (i.units_buf, i.units_cap) = (self.units.as_mut_ptr(), self.units.len());
        (i.fields_buf, i.fields_cap) = (self.fields.as_mut_ptr(), self.fields_cap);
        (i.arena_buf, i.arena_cap) = (self.arena.as_mut_ptr(), self.arena.len());
        (i.records_buf, i.records_cap) = (self.records.as_mut_ptr(), self.records.len());
        let mut o: OnPieceOut = z();
        o.head = out_head();
        Frame::new(i, o)
    }

    /// `push` answered: the outcome and what it wrote, as text.
    fn call(&mut self, p: &Plugin<Plane>, push: &Push<'_>) -> String {
        let mut f = self.frame(push);
        let c = p.call(slot::ON_PIECE, &mut f);
        self.answer(&c, f.out)
    }

    fn answer(&mut self, c: &busbar_plugin_loader::dispatch::Called, o: OnPieceOut) -> String {
        self.last_need = o.need;
        self.last_flags = o.flags;
        self.last_audit = if o.records_needed != 0 {
            format!("needed {}", o.records_needed)
        } else {
            self.records[..o.records_written as usize]
                .iter()
                .map(|r| {
                    let outcome = if r.op == RECORD_AUDIT && r.kind == AUDIT_REJECTED {
                        "rejected".to_string()
                    } else {
                        format!("op{}/kind{}", r.op, r.kind)
                    };
                    format!(
                        "{outcome} {} {}",
                        at(&self.arena, r.key),
                        at(&self.arena, r.value)
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ")
        };
        self.last_units = if o.units_needed != 0 {
            format!("needed {}", o.units_needed)
        } else {
            self.units[..o.units_written as usize]
                .iter()
                .map(|u| format!("{}:{}={}", u.class, u.source, u.amount))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let fields: Vec<String> = self.fields[..o.fields_written as usize]
            .iter()
            .map(|f| format!("{}={}", at(&self.arena, f.name), at(&self.arena, f.value)))
            .collect();
        format!(
            "{:?} emitted={} more={} far={} done={} status={} verb={} target={} fields={fields:?}",
            c.outcome,
            String::from_utf8_lossy(&self.reply[..o.emitted as usize]),
            o.more,
            o.flags & EMIT_TO_FAR_END != 0,
            o.flags & EMIT_DONE != 0,
            o.reply_status,
            at(&self.arena, o.verb),
            at(&self.arena, o.target),
        )
    }
}

/// A piece of `unit` on `claim`.
const fn push(unit: u64, claim: u32, from: u32, flags: u32, bytes: &[u8]) -> Push<'_> {
    Push {
        unit,
        stream: 0,
        claim,
        from,
        flags,
        attempt_no: 0,
        bytes,
        status: 0,
        head: &[],
    }
}

/// A session crossing keeps the stream identity on every caller and collection piece.
const fn live_push(unit: u64, claim: u32, from: u32, flags: u32, bytes: &[u8]) -> Push<'_> {
    Push {
        stream: unit,
        ..push(unit, claim, from, flags, bytes)
    }
}

/// The ATTEMPT that opens `unit`'s first attempt.
const fn attempt(unit: u64, claim: u32) -> Push<'static> {
    Push {
        attempt_no: 1,
        ..push(unit, claim, FROM_KERNEL, 0, &[])
    }
}

/// THE UNIT SCRIPT: the mint (busbar mints the ticket itself and dials nothing; the dispatcher here
/// grants the key no pool, so the mint is refused 403 `not_granted` before any ticket is minted,
/// [`MINT_NOT_GRANTED`]), the SDP door's request piece (it opens a
/// session, so a plain request is refused 501, explained), a session door's request piece and an unpublished
/// claim's (the metadata path among them: it is no claim, 1.5.5 parity).
fn unit_script(p: &Plugin<Plane>) -> Vec<String> {
    let mut t = vec![format!("open {:?}", open(p, 1, SESSION, Some(PUBLIC)).0)];
    let mut b = Bufs::new(256);
    t.push(format!("mint attempt {}", b.call(p, &attempt(7, 0))));
    let caller = push(7, 0, FROM_CALLER, PIECE_LAST, &[]);
    t.push(format!("mint caller {}", b.call(p, &caller)));
    t.push(format!("mint units [{}]", b.last_units));
    t.push(format!("sdp request {}", b.call(p, &attempt(8, 1))));
    t.push(format!("sideband {}", b.call(p, &attempt(10, 2))));
    t.push(format!("unpublished {}", b.call(p, &attempt(11, 9))));
    t.push(format!("metadata path {}", b.call(p, &attempt(12, 6))));
    t
}

/// What the unit script must read, through the door.
fn unit_expected() -> Vec<String> {
    let none = "verb= target= fields=[]";
    vec![
        "open Ready".to_string(),
        format!("mint attempt Ready emitted= more=0 far=false done=false status=0 {none}"),
        format!(
            "mint caller Ready emitted={MINT_NOT_GRANTED} more=0 far=false done=true status=403 \
             verb= target= fields=[\"content-type=application/json\", \"content-length={}\"]",
            MINT_NOT_GRANTED.len()
        ),
        "mint units []".to_string(),
        format!(
            "sdp request Ready emitted={SDP_PLAIN_REQUEST} more=0 far=false done=true status=501 \
             verb= target= fields=[\"content-type=application/json\", \"content-length={}\"]",
            SDP_PLAIN_REQUEST.len()
        ),
        format!("sideband Refused emitted= more=0 far=false done=false status=0 {none}"),
        format!("unpublished Refused emitted= more=0 far=false done=false status=0 {none}"),
        format!("metadata path Refused emitted= more=0 far=false done=false status=0 {none}"),
    ]
}

#[test]
fn the_one_request_doors_answer_their_pieces_through_the_door() {
    let (d, host) = ticket_host_granting(&[]);
    assert_eq!(unit_script(&linked(&d)), unit_expected());
    assert!(host
        .minted
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_empty());
}

/// An unavailable grant service must never be reported as a known policy denial.
#[test]
fn an_unavailable_mint_grant_service_fails_closed_without_a_ticket() {
    let d = Dispatcher::new(DispatchConfig::default());
    for p in [linked(&d), dropped(&d)] {
        assert_eq!(open(&p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::new(512);
        let answer = b.call(&p, &push(71, 0, FROM_CALLER, PIECE_LAST, &[]));
        assert!(answer.contains("status=502"), "{answer}");
        assert!(
            answer.contains("the host could not check the key's grant"),
            "{answer}"
        );
        assert!(!answer.contains("not_granted"), "{answer}");
        assert!(!answer.contains(TICKET), "{answer}");
        assert_eq!(b.last_units, "", "a failed mint bills nothing");
        assert_eq!(b.last_need, 0, "and dials nothing");
    }
}

#[test]
fn the_dropped_door_answers_the_one_request_doors_as_the_linked_door_does() {
    let d = Dispatcher::new(DispatchConfig::default());
    let dropped = dropped(&d);
    assert_eq!(
        unit_script(&dropped),
        unit_script(&linked(&d)),
        "the two doors must answer identically"
    );
}

/// Mint a local ticket through a 16-byte reply buffer and collect the complete answer.
fn narrow_mint_script(p: &Plugin<Plane>) -> (Vec<u8>, Vec<String>) {
    let _ = open(p, 1, SESSION, Some(PUBLIC));
    let mut narrow = Bufs::new(16);
    let mut lines = vec![narrow.call(p, &push(7, 0, FROM_CALLER, PIECE_LAST, &[]))];
    let mut bytes = Vec::new();
    loop {
        let line = lines.last().expect("a line");
        let emitted = line
            .split(" emitted=")
            .nth(1)
            .and_then(|r| r.split(" more=").next())
            .expect("emitted");
        bytes.extend_from_slice(emitted.as_bytes());
        if line.contains(" more=0 ") || lines.len() > 64 {
            break;
        }
        lines.push(narrow.call(p, &push(7, 0, FROM_FAR_END, 0, &[])));
    }
    (bytes, lines)
}

/// The mint answer streams through a narrow reply buffer, byte for byte the minted answer, and the
/// dropped-in door answers it as the linked door does.
#[test]
fn a_mint_answer_streams_through_a_narrow_reply_buffer() {
    let d = host();
    let (bytes, lines) = narrow_mint_script(&linked(&d));

    let minted =
        format!(r#"{{"expires_at_unix":{TICKET_EXPIRES},"value":"{TICKET}"}}"#).into_bytes();
    assert_eq!(bytes, minted, "{lines:#?}");
    assert_eq!(lines.len(), minted.len().div_ceil(16), "{lines:#?}");
    for line in &lines[..lines.len() - 1] {
        assert!(
            line.contains(" more=1 ") && line.contains(" done=false "),
            "{line}"
        );
    }
    assert!(lines[lines.len() - 1].contains(" done=true "), "{lines:#?}");
    assert_eq!(narrow_mint_script(&dropped(&d)), (bytes, lines));
}

/// A piece of the live session on stream `stream` (claim 2, the sideband socket).
const fn session(stream: u64, from: u32, flags: u32, bytes: &[u8]) -> Push<'_> {
    Push {
        stream,
        ..push(stream, 2, from, flags, bytes)
    }
}

/// `ms` milliseconds of 24 kHz PCM16 uplink audio, as the caller's realtime frame.
fn uplink(ms: usize) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"type":"input_audio_buffer.append",
        "audio": busbar_contract::media::base64_encode(&vec![0u8; ms * 48])}))
    .expect("json")
}

const USAGE_DONE: &[u8] = br#"{"type":"response.done","response":{"usage":{"input_token_details":{"audio_tokens":10,"text_tokens":3},"output_token_details":{"audio_tokens":20,"text_tokens":4}}}}"#;

/// A host that mints and takes tickets and draws randomness: unit 50 was admitted by a ticket whose
/// record locks `instructions: from-ticket`; every other unit by a key. Each draw of randomness is
/// a fresh count, so every call busbar places has its own id.
struct TicketHost {
    draws: std::sync::atomic::AtomicU64,
    /// The record the last mint stored beside its ticket.
    minted: std::sync::Mutex<Vec<u8>>,
    /// The `pool:<entry>` targets the key is granted (`entitlement.check`).
    granted: &'static [&'static str],
    /// Unit 50's ticket is taken once, as a host takes it: a second take is refused.
    once: bool,
    /// Whether unit 50's ticket was taken.
    took: std::sync::atomic::AtomicBool,
}

/// The record unit 50's ticket locks: a session config naming its instructions.
const TICKET_RECORD: &[u8] = br#"{"instructions":"from-ticket"}"#;

impl busbar_contract::services::HostServices for TicketHost {
    fn now(&self) -> Reading {
        Reading {
            wall_ns: 0,
            mono_ns: 0,
        }
    }
    fn dest_judge(&self, _dest: &str, _class: u32, _flags: u32, _later: Option<Later>) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn records_get(&self, _caller: &Caller, _kind: &str, _key: &[u8], _later: Later) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn records_list(&self, _caller: &Caller, _list: RecordsList, _later: Later) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn records_claim(
        &self,
        _caller: &Caller,
        _kind: &str,
        _key: &[u8],
        _ttl_ms: u64,
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn sign(&self, _caller: &Caller, _data: &[u8]) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn trust_sight(
        &self,
        _caller: &Caller,
        _counterparty: &str,
        _hash: &str,
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn trust_unreached(&self, _caller: &Caller, _counterparty: &str) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn trust_sight_item(
        &self,
        _caller: &Caller,
        _counterparty: &str,
        _item: &str,
        _digest: &str,
    ) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn trust_decide(
        &self,
        _caller: &Caller,
        _key: TrustKeyRef<'_>,
        _expected: Option<&str>,
        _approve: bool,
    ) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn trust_state(&self, _caller: &Caller, _counterparty: &str) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn trust_serves(
        &self,
        _caller: &Caller,
        _counterparty: &str,
        _item: Option<&str>,
        _digest: Option<&str>,
    ) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn trust_due(&self, _caller: &Caller) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn trust_verify(
        &self,
        _caller: &Caller,
        _counterparty: &str,
        _payload: &[u8],
        _signatures: &[u8],
    ) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn session_emit(&self, _caller: &Caller, _session: u64, _bytes: &[u8]) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn records_secret(&self, _kind: &str, _id: &str, _later: Later) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn unit_nest(&self, _caller: &Caller, _unit: Option<u64>, _ask: NestAsk, _later: Later) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn work_open(
        &self,
        _caller: &Caller,
        _unit: Option<u64>,
        _kind: &str,
        _record: &[u8],
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn work_find(
        &self,
        _caller: &Caller,
        _unit: Option<u64>,
        _reference: &[u8],
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn work_settle(
        &self,
        _caller: &Caller,
        _unit: Option<u64>,
        _handle: u64,
        _record: &[u8],
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn work_resume(
        &self,
        _caller: &Caller,
        _unit: Option<u64>,
        _handle: u64,
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn disk_append(&self, _dest: &DiskDest, _bytes: Vec<u8>, _later: Later) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn verify_lookup(&self, _caller: &Caller, _key: &[u8], _later: Later) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn verify_store(&self, _caller: &Caller, _key: &[u8], _entry: &[u8], _ttl_ms: u64) -> Stored {
        Stored::refused(busbar_contract::services::UNSERVED)
    }
    fn content_scan(
        &self,
        _caller: &Caller,
        _unit: Option<u64>,
        _content: &[u8],
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn hook_call(&self, _caller: &Caller, _unit: Option<u64>, _ask: HookAsk, _later: Later) -> Ran {
        Ran::Now(Stored::refused(busbar_contract::services::UNSERVED))
    }
    fn snapshot_read(&self, _caller: &Caller, _scope: u32) -> Snapshot {
        Snapshot::Refused(busbar_contract::services::UNSERVED)
    }
    fn entitlement_check(
        &self,
        _caller: &busbar_contract::services::Caller,
        _unit: Option<u64>,
        target: &str,
    ) -> busbar_contract::services::Stored {
        use busbar_contract::abi::host::service::{ENTITLED, NOT_ENTITLED};
        busbar_contract::services::Stored::ready(if self.granted.contains(&target) {
            ENTITLED
        } else {
            NOT_ENTITLED
        })
    }

    fn ticket_mint(
        &self,
        _caller: &busbar_contract::services::Caller,
        _unit: Option<u64>,
        _ttl_secs: u64,
        record: &[u8],
    ) -> busbar_contract::services::Stored {
        *self
            .minted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = record.to_vec();
        let mut stored = busbar_contract::services::Stored::ready(TICKET_EXPIRES);
        stored.bytes = TICKET.as_bytes().to_vec();
        stored
    }

    fn ticket_take(
        &self,
        _caller: &busbar_contract::services::Caller,
        unit: Option<u64>,
    ) -> busbar_contract::services::Stored {
        use busbar_contract::abi::host::service::{TICKET_NONE, TICKET_TAKEN};
        let again = unit == Some(50)
            && self.took.swap(true, std::sync::atomic::Ordering::Relaxed)
            && self.once;
        if again {
            busbar_contract::services::Stored::refused("the unit's ticket was taken already")
        } else if unit == Some(50) {
            let mut stored = busbar_contract::services::Stored::ready(TICKET_TAKEN);
            stored.bytes = TICKET_RECORD.to_vec();
            stored
        } else {
            busbar_contract::services::Stored::ready(TICKET_NONE)
        }
    }

    fn random_fill(&self, len: u64) -> busbar_contract::services::Stored {
        let draw = self
            .draws
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_le_bytes();
        let mut stored = busbar_contract::services::Stored::ready(0);
        stored.bytes = draw
            .iter()
            .copied()
            .cycle()
            .take(usize::try_from(len).expect("a length"))
            .collect();
        stored
    }
}

/// A dispatcher whose host serves tickets and randomness ([`TicketHost`]) to a key granted the
/// session's model (`pool:m-cap`), and that host.
fn ticket_host() -> (Dispatcher, Arc<TicketHost>) {
    ticket_host_granting(&["pool:m-cap"])
}

/// [`ticket_host`] for a key granted exactly `granted`.
fn ticket_host_granting(granted: &'static [&'static str]) -> (Dispatcher, Arc<TicketHost>) {
    let host = Arc::new(TicketHost {
        draws: std::sync::atomic::AtomicU64::new(0xa0),
        minted: std::sync::Mutex::new(Vec::new()),
        granted,
        once: false,
        took: std::sync::atomic::AtomicBool::new(false),
    });
    let d = Dispatcher::with_services(DispatchConfig::default(), host.clone());
    (d, host)
}

/// A dispatcher whose host serves tickets and randomness.
fn host() -> Dispatcher {
    ticket_host().0
}

/// `s`, for the life of the test binary (a pushed target is `'static`).
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// The SDP offer of stream `unit` by `caller`: busbar answers 201 naming its own call, places its
/// call to the provider, and the provider answers 201 naming its call at `location`: the call that
/// caller's sideband may attach to. Busbar's call id.
fn broker(p: &Plugin<Plane>, unit: u64, location: &'static str, caller: &'static str) -> String {
    let mut b = Bufs::with_units(1 << 16, 8);
    b.caller = caller;
    let opened = b.call(
        p,
        &Push {
            stream: unit,
            ..push(unit, 1, FROM_CALLER, 0, &[])
        },
    );
    let id = opened
        .split("location=/v1/realtime/calls/")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("the open names busbar's call: {opened}"))
        .to_string();
    assert!(
        opened.contains("status=201"),
        "the open is answered 201: {opened}"
    );
    let placed = b.call(
        p,
        &Push {
            stream: unit,
            ..push(unit, 1, FROM_KERNEL, 0, &[])
        },
    );
    assert!(placed.contains("far=true"), "busbar's call: {placed}");
    let head = [Field {
        name: text("location"),
        value: text(location),
    }];
    let answered = b.call(
        p,
        &Push {
            stream: unit,
            status: 201,
            head: &head,
            ..push(unit, 1, FROM_FAR_END, PIECE_HAS_STATUS, &[])
        },
    );
    assert!(answered.starts_with("Ready"), "{answered}");
    id
}

/// The sideband target naming busbar's call `id`.
fn sideband_to(id: &str) -> &'static str {
    leak(format!("/v1/realtime/sideband/{id}"))
}

/// The SDP offer the test caller makes for the provider's call `location` (on a stream of its
/// own), then `arrive` of `unit` on the sideband door naming busbar's call: the provider call the
/// unit's session attaches to.
fn attach_sideband(p: &Plugin<Plane>, unit: u64, location: &'static str) -> Outcome {
    let id = broker(p, unit + 100_000, location, CALLER_REF);
    arrive_sideband(p, unit, sideband_to(&id))
}

/// `arrive` of `unit` on the sideband door at `target` (`/v1/realtime/sideband/{call_id}`): the
/// provider call the unit's session attaches to.
fn arrive_sideband(p: &Plugin<Plane>, unit: u64, target: &'static str) -> Outcome {
    let mut a: Frame<ArriveIn, ArriveOut> = Frame::new(z(), z());
    a.input.head = in_head();
    (a.input.unit, a.input.claim) = (unit, 2);
    a.input.target = text(target);
    a.out.head = out_head();
    p.call(slot::ARRIVE, &mut a).outcome
}

/// THE SESSION SCRIPT, in the order the driver pushes a session's pieces: the sideband arrival
/// naming its call, the caller's audio (a turn toward the far end), the turn's ATTEMPT, the far
/// end's usage, a collection with nothing owed, an answer short of unit room and its re-call, and
/// the caller's end.
fn session_script(p: &Plugin<Plane>) -> Vec<String> {
    let mut t = vec![format!("open {:?}", open(p, 1, SESSION, Some(PUBLIC)).0)];
    t.push(format!(
        "arrive {:?}",
        attach_sideband(p, 20, "/v1/realtime/calls/rtc_both0020")
    ));
    let mut b = Bufs::with_units(1 << 18, 8);
    let line = |t: &mut Vec<String>, b: &mut Bufs, what: &str, push: &Push<'_>| {
        let answer = b.call(p, push);
        t.push(format!("{what} {answer} units=[{}]", b.last_units));
    };
    let audio = uplink(2000);
    line(
        &mut t,
        &mut b,
        "caller audio",
        &session(20, FROM_CALLER, 0, &audio),
    );
    let turn = Push {
        attempt_no: 1,
        ..session(20, FROM_KERNEL, 0, &[])
    };
    line(&mut t, &mut b, "turn attempt", &turn);
    let far = Push {
        status: 101,
        ..session(20, FROM_FAR_END, PIECE_HAS_STATUS, USAGE_DONE)
    };
    line(&mut t, &mut b, "far usage", &far);
    line(&mut t, &mut b, "collect", &session(20, FROM_KERNEL, 0, &[]));
    let mut narrow = Bufs::with_units(1 << 18, 0);
    let short = session(20, FROM_CALLER, 0, &audio);
    let answer = narrow.call(p, &short);
    t.push(format!("short {answer} units=[{}]", narrow.last_units));
    line(&mut t, &mut b, "short re-call", &short);
    line(
        &mut t,
        &mut b,
        "caller end",
        &session(20, FROM_CALLER, PIECE_LAST, &[]),
    );
    line(
        &mut t,
        &mut b,
        "request unit on a session door",
        &attempt(21, 2),
    );
    t
}

#[test]
fn a_live_session_is_answered_through_the_door() {
    let d = host();
    let t = session_script(&linked(&d));
    let payload = busbar_contract::media::base64_encode(&vec![0u8; 2000 * 48]);
    let line = |what: &str| {
        t.iter()
            .find(|l| l.starts_with(what))
            .unwrap_or_else(|| panic!("no `{what}` line in {t:#?}"))
            .clone()
    };
    assert_eq!(t[0], "open Ready");
    assert_eq!(t[1], "arrive Ready");
    let caller = line("caller audio");
    assert!(
        caller.contains("far=true")
            && caller.contains("verb=GET target=/v1/realtime?call_id=rtc_both0020 ")
            && caller.contains("input_audio_buffer.append")
            && caller.contains(&payload)
            && caller.contains("more=0")
            && caller.ends_with("units=[4:1=2]"),
        "the sideband relays audio unchanged and reports only its locally injected audio: {caller}"
    );
    assert_eq!(
        line("turn attempt"),
        "turn attempt Ready emitted= more=0 far=false done=false status=0 verb= target= \
         fields=[] units=[4:1=2]",
        "the turn's request is the caller side's answer"
    );
    let far = line("far usage");
    assert!(
        far.contains("far=false done=false") && far.ends_with("units=[4:1=2]"),
        "mirrored provider usage adds no duplicate token billing: {far}"
    );
    assert!(line("collect ").contains("emitted= more=0 far=false done=false"));
    assert!(
        line("short ").starts_with("short Failed") && line("short ").ends_with("units=[needed 1]"),
        "{}",
        line("short ")
    );
    let recall = line("short re-call");
    assert!(
        recall.contains("far=true") && recall.ends_with("units=[4:1=4]"),
        "the recall counts its injected audio exactly once: {recall}"
    );
    let end = line("caller end");
    assert!(
        end.contains("done=true") && end.ends_with("units=[4:1=4]"),
        "sideband close settles only its locally injected audio: {end}"
    );
    assert!(line("request unit on a session door")
        .starts_with("request unit on a session door Refused"));
}

/// A sideband session attaches to the call named by its arrival.
#[test]
fn a_sideband_session_attaches_to_the_call_its_arrival_names() {
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::with_units(1 << 18, 8);
        let unnamed = b.call(p, &session(60, FROM_CALLER, 0, &uplink(100)));
        assert!(unnamed.starts_with("Refused"), "{unnamed}");
        assert_eq!(
            attach_sideband(p, 61, "/v1/realtime/calls/rtc_both0061"),
            Outcome::Ready
        );
        let named = b.call(p, &session(61, FROM_CALLER, 0, &uplink(100)));
        assert!(
            named.contains("far=true")
                && named.contains("verb=GET target=/v1/realtime?call_id=rtc_both0061 "),
            "{named}"
        );
        assert_eq!(
            arrive_sideband(p, 62, "/v1/realtime/sideband/rtc_x&model=y"),
            Outcome::Refused,
            "an id that is not a call id's shape names no call"
        );
        let forged = b.call(p, &session(62, FROM_CALLER, 0, &uplink(100)));
        assert!(forged.starts_with("Refused"), "{forged}");
        assert_eq!(
            arrive_sideband(p, 63, "/v1/realtime/sideband/rtc_both0061"),
            Outcome::Refused,
            "the provider's own call id is not busbar's: no call busbar holds"
        );
    }
}

/// A sideband attaches only to a call owned by its caller.
#[test]
fn a_sideband_attaches_only_to_its_own_callers_call() {
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let theirs = sideband_to(&broker(p, 90, "/v1/realtime/calls/rtc_theirs0001", "beef"));
        let mut b = Bufs::with_units(1 << 18, 8);
        assert_eq!(arrive_sideband(p, 91, theirs), Outcome::Ready);
        let foreign = b.call(p, &session(91, FROM_CALLER, 0, &uplink(100)));
        assert!(
            foreign.starts_with("Refused"),
            "another caller's call: {foreign}"
        );
        assert_eq!(
            arrive_sideband(p, 92, "/v1/realtime/sideband/rtc_unbrokered01"),
            Outcome::Refused,
            "a call no offer here created"
        );
        let unknown = b.call(p, &session(92, FROM_CALLER, 0, &uplink(100)));
        assert!(
            unknown.starts_with("Refused"),
            "a call no offer here created: {unknown}"
        );
        b.caller = "beef";
        assert_eq!(arrive_sideband(p, 93, theirs), Outcome::Ready);
        let own = b.call(p, &session(93, FROM_CALLER, 0, &uplink(100)));
        assert!(
            own.contains("far=true")
                && own.contains("verb=GET target=/v1/realtime?call_id=rtc_theirs0001 "),
            "the offer's own caller attaches: {own}"
        );
    }
}

/// A sideband joining a brokered call adds no second session fee.
#[test]
fn a_sideband_joining_a_brokered_call_adds_no_second_fee() {
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let id = broker(p, 94, "/v1/realtime/calls/rtc_fee0001", CALLER_REF);
        assert_eq!(arrive_sideband(p, 95, sideband_to(&id)), Outcome::Ready);
        let mut b = Bufs::with_units(1 << 18, 8);
        let _ = b.call(p, &session(95, FROM_CALLER, 0, &uplink(100)));
        let far = Push {
            status: 101,
            ..session(95, FROM_FAR_END, PIECE_HAS_STATUS, USAGE_DONE)
        };
        let answered = b.call(p, &far);
        assert!(answered.starts_with("Ready"), "{answered}");
        assert_eq!(
            b.last_units, "4:1=1",
            "only locally injected audio is billed on the attached sideband"
        );
    }
}

/// A brokered browser session reports its session fee.
#[test]
fn a_brokered_browser_session_reports_its_session_fee() {
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::with_units(1 << 16, 8);

        let minted = b.call(p, &push(70, 0, FROM_CALLER, PIECE_LAST, &[]));
        assert!(minted.contains("done=true status=200"), "{minted}");
        assert_eq!(b.last_units, "", "a ticket places no call: {minted}");

        for (stream, status, fee) in [(71, 201, "8:1=1"), (73, 400, "")] {
            let opened = b.call(
                p,
                &Push {
                    stream,
                    ..push(stream, 1, FROM_CALLER, 0, &[])
                },
            );
            assert!(opened.contains("status=201"), "{opened}");
            assert_eq!(b.last_units, "", "nothing answered yet: {opened}");
            let placed = b.call(
                p,
                &Push {
                    stream,
                    ..push(stream, 1, FROM_KERNEL, 0, &[])
                },
            );
            assert!(placed.contains("far=true"), "{placed}");
            let head = [Field {
                name: text("location"),
                value: text("/v1/realtime/calls/rtc_fee0071"),
            }];
            let answered = b.call(
                p,
                &Push {
                    stream,
                    status,
                    head: &head,
                    ..push(stream, 1, FROM_FAR_END, PIECE_HAS_STATUS, &[])
                },
            );
            assert!(answered.starts_with("Ready"), "{answered}");
            assert_eq!(
                b.last_units, fee,
                "the provider's {status} to busbar's call: {answered}"
            );
        }
        let ended = b.call(
            p,
            &Push {
                stream: 73,
                ..push(73, 1, FROM_KERNEL, 0, &[])
            },
        );
        assert!(
            ended.contains("done=true"),
            "a refused call ends its session: {ended}"
        );
        assert_eq!(b.last_units, "", "and bills nothing: {ended}");
    }
}

#[test]
fn the_dropped_door_answers_a_live_session_as_the_linked_door_does() {
    let d = host();
    let dropped = dropped(&d);
    assert_eq!(
        session_script(&dropped),
        session_script(&linked(&d)),
        "the two doors must answer a session identically"
    );
}

/// `arrive` on `claim`: its outcome, the route class and the entry it names.
fn arrive_route(p: &Plugin<Plane>, claim: u32) -> (Outcome, u8, String) {
    let mut a: Frame<ArriveIn, ArriveOut> = Frame::new(z(), z());
    a.input.head = in_head();
    a.input.claim = claim;
    a.out.head = out_head();
    let c = p.call(slot::ARRIVE, &mut a);
    (c.outcome, a.out.route, read(a.out.pool))
}

/// `arrive` on `claim`: the route flags it states.
fn arrive_flags(p: &Plugin<Plane>, claim: u32) -> u8 {
    let mut a: Frame<ArriveIn, ArriveOut> = Frame::new(z(), z());
    a.input.head = in_head();
    a.input.claim = claim;
    a.out.head = out_head();
    let _ = p.call(slot::ARRIVE, &mut a);
    a.out.route_flags
}

/// Every door that reaches the far end names the DIRECT route `streams.session.model` (the entry the
/// composition folds from the top-level models catalog); the mint door is the plane's own to answer
/// (row 132: busbar mints the ticket and dials nothing), counted against the caller's caps; the
/// sideband door names a call busbar placed, and one it does not hold is refused; claim 6 (the
/// metadata path) is no claim and arrives nowhere; with no model configured nothing is named (the
/// door steps refuse an unnamed keyed route). Both ways.
#[test]
fn a_door_that_reaches_the_far_end_names_the_session_model_as_its_direct_route() {
    use busbar_contract::abi::plane::{
        ROUTE_COUNTED, ROUTE_DIRECT, ROUTE_LOCAL, ROUTE_POOL, ROUTE_SCOPE_LOCAL,
    };
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        for claim in [1, 3, 4, PLAIN_CLAIM] {
            assert_eq!(
                arrive_route(p, claim),
                (Outcome::Ready, ROUTE_DIRECT, "m-cap".to_string()),
                "claim {claim}"
            );
        }
        assert_eq!(
            arrive_route(p, 0),
            (Outcome::Ready, ROUTE_SCOPE_LOCAL, "m-cap".to_string()),
            "the mint stays local while naming the model whose grant precedes counted admission"
        );
        assert_eq!(
            arrive_flags(p, 0) & ROUTE_COUNTED,
            ROUTE_COUNTED,
            "the mint counts against the caller's caps"
        );
        assert_eq!(
            arrive_route(p, 2).0,
            Outcome::Refused,
            "a sideband naming no call busbar holds"
        );
        assert_ne!(
            arrive_route(p, 6).0,
            Outcome::Ready,
            "the metadata path is no claim of the plane (1.5.5 parity)"
        );
    }
    for bare in [linked(&d), dropped(&d)] {
        assert_eq!(open(&bare, 1, b"{}", Some(PUBLIC)).0, Outcome::Ready);
        assert_eq!(
            arrive_route(&bare, 3),
            (Outcome::Ready, ROUTE_POOL, String::new()),
            "no model, no route named"
        );
        assert_eq!(
            arrive_route(&bare, 0),
            (Outcome::Ready, ROUTE_LOCAL, String::new()),
            "an unconfigured mint stays local and names no wildcard model"
        );
        assert_eq!(
            arrive_flags(&bare, 0) & ROUTE_COUNTED,
            ROUTE_COUNTED,
            "the unconfigured local mint retains its counted admission contract"
        );
    }
}

/// `project` of unit 31 on `claim` with `rewrite` (empty = none): its outcome, the projected body
/// and whether it was answered as rewritten.
fn project(p: &Plugin<Plane>, claim: u32, rewrite: &'static [u8]) -> (Outcome, String, bool) {
    project_at(p, claim, 31, rewrite, 4096)
}

/// [`project`] of `unit`, its arena lent with `arena_cap` bytes.
fn project_at(
    p: &Plugin<Plane>,
    claim: u32,
    unit: u64,
    rewrite: &'static [u8],
    arena_cap: usize,
) -> (Outcome, String, bool) {
    let (outcome, body, rewritten, _) = project_recall_at(p, claim, unit, rewrite, arena_cap, None);
    (outcome, body, rewritten)
}

fn project_recall_at(
    p: &Plugin<Plane>,
    claim: u32,
    unit: u64,
    rewrite: &'static [u8],
    arena_cap: usize,
    recall: Option<busbar_plugin_loader::dispatch::Recall>,
) -> (
    Outcome,
    String,
    bool,
    Option<busbar_plugin_loader::dispatch::Recall>,
) {
    use busbar_contract::abi::hook::{MessageView, SignalEntry};
    use busbar_contract::abi::plane::{ProjectIn, ProjectOut, SPAN_ABSENT};
    let mut arena = vec![0u8; arena_cap];
    let mut signals: Vec<SignalEntry> = vec![z(); 4];
    let mut turns: Vec<MessageView> = vec![z(); 4];
    let mut j: Frame<ProjectIn, ProjectOut> = Frame::new(z(), z());
    (j.input.head, j.out.head) = (in_head(), out_head());
    j.input.claim = claim;
    j.input.unit = unit;
    j.input.arena_buf = arena.as_mut_ptr();
    j.input.arena_cap = arena.len();
    j.input.signals_buf = signals.as_mut_ptr();
    j.input.signals_cap = signals.len();
    j.input.messages_buf = turns.as_mut_ptr();
    j.input.messages_cap = turns.len();
    if !rewrite.is_empty() {
        j.input.rewrite = json(rewrite);
    }
    let c = match recall {
        Some(token) => p.recall(token, slot::PROJECT, &mut j),
        None => p.call(slot::PROJECT, &mut j),
    };
    let body = if j.out.body.offset == SPAN_ABSENT {
        String::new()
    } else {
        at(&arena, j.out.body)
    };
    (
        c.outcome,
        body,
        j.out.rewritten.offset != SPAN_ABSENT,
        c.recall,
    )
}

/// A session door projects the session-open params it locks as the view's body; a hook's rewrite is
/// patched over them and THAT is answered (kept for the session the unit opens); a one-request door
/// projects no body and takes no rewrite. Both ways.
#[test]
fn a_session_open_is_projected_for_the_hooks_and_a_rewrite_patches_it() {
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let (outcome, body, rewritten) = project(p, 5, b"");
        assert_eq!(outcome, Outcome::Ready);
        let view: serde_json::Value = serde_json::from_str(&body).expect("the locked params");
        assert_eq!(view["model"], "m-cap", "{body}");
        assert!(!rewritten);
        let (outcome, body, rewritten) = project(p, 5, br#"{"voice":"marin"}"#);
        assert_eq!(outcome, Outcome::Ready);
        let view: serde_json::Value = serde_json::from_str(&body).expect("the patched params");
        assert_eq!(view["voice"], "marin", "the hook's key wins: {body}");
        assert_eq!(
            view["model"], "m-cap",
            "the unnamed key keeps its value: {body}"
        );
        assert!(rewritten, "answered as rewritten");
        assert_eq!(project(p, 0, b""), (Outcome::Ready, String::new(), false));
        assert_eq!(project(p, 0, br#"{"x":1}"#).0, Outcome::Refused);
    }
}

/// Every far request names the outbound need it rides: a live session's frames OpenAI Realtime's
/// socket, busbar's own call for an SDP door's session the outbound webrtc need (each a declared
/// outbound need under the dialect's style, held by the door's own compile-time check); the mint
/// dials nothing (row 132). Both ways.
#[test]
fn every_far_request_names_the_need_it_rides() {
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        assert_eq!(
            attach_sideband(p, 40, "/v1/realtime/calls/rtc_both0040"),
            Outcome::Ready
        );
        let mut b = Bufs::with_units(1 << 18, 8);
        let answer = b.call(p, &session(40, FROM_CALLER, 0, &uplink(100)));
        assert!(answer.contains("far=true"), "{answer}");
        assert_eq!(b.last_need, rides("ws", "bearer"), "{answer}");
        let opening = Push {
            stream: 42,
            ..push(42, 1, FROM_CALLER, 0, &[])
        };
        let _ = b.call(p, &opening);
        let call = b.call(
            p,
            &Push {
                stream: 42,
                ..push(42, 1, FROM_KERNEL, 0, &[])
            },
        );
        assert!(call.contains("far=true"), "{call}");
        assert_eq!(b.last_need, rides("webrtc", "bearer"), "{call}");
        let mut b = Bufs::new(1 << 16);
        let answer = b.call(p, &attempt(41, 0));
        assert!(
            answer.contains("far=false"),
            "the mint dials nothing: {answer}"
        );
        assert_eq!(b.last_need, 0, "{answer}");
    }
}

/// The kernel's refusal of `reason` at `status`, worded `words`, for a unit-less request on
/// `target`, rendered by the door's `refusal`: the outcome and the body it wrote.
fn refused(
    p: &Plugin<Plane>,
    status: u32,
    reason: busbar_contract::caps::ReasonCode,
    words: &'static str,
    target: &'static str,
) -> (Outcome, String) {
    use busbar_contract::abi::plane::{reason_code, RefusalIn, RefusalOut, REFUSAL_KERNEL};
    let mut reply = vec![0u8; 1024];
    let mut fields: Vec<OutField> = vec![z(); 4];
    let mut arena = vec![0u8; 256];
    let mut f: Frame<RefusalIn, RefusalOut> = Frame::new(z(), z());
    (f.input.head, f.out.head) = (in_head(), out_head());
    f.input.cause = REFUSAL_KERNEL;
    f.input.status = status;
    f.input.reason = reason_code(reason);
    f.input.text = text(words);
    f.input.target = text(target);
    (f.input.reply_buf, f.input.reply_cap) = (reply.as_mut_ptr(), reply.len());
    (f.input.fields_buf, f.input.fields_cap) = (fields.as_mut_ptr(), fields.len());
    (f.input.arena_buf, f.input.arena_cap) = (arena.as_mut_ptr(), arena.len());
    let c = p.call(slot::REFUSAL, &mut f);
    let written = usize::try_from(f.out.reply_written).expect("a length");
    (
        c.outcome,
        String::from_utf8_lossy(&reply[..written.min(reply.len())]).into_owned(),
    )
}

/// AN AUTHENTICATION REFUSAL READS IN THE DIALECTS' OWN VENDOR TERMS (spec Part 3, "Refusals": a
/// refusal with no unit is rendered by the plane's path rule, the 1.5.5 residual's answer; oracle
/// cell `streams|mint|no-provider`): the kernel's `Unauthenticated` on the mint door answers as a bad
/// key does at the vendor, never with the kernel's own word, which is a busbar tell. Every other
/// reason keeps the kernel's text, a 401 included (a hook's veto states its own). Both ways.
#[test]
fn an_authentication_refusal_reads_in_the_dialects_vendor_terms() {
    use busbar_contract::caps::ReasonCode;
    let vendor = serde_json::json!({"error": {
        "message": AUTH_FAILURE_MESSAGE,
        "type": "authentication_error",
        "param": null,
        "code": AUTH_FAILURE_CODE,
    }});
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, b"{}", Some(PUBLIC)).0, Outcome::Ready);
        let (outcome, body) = refused(
            p,
            401,
            ReasonCode::Unauthenticated,
            "unauthenticated",
            "/v1/realtime/client_secrets",
        );
        assert_eq!(outcome, Outcome::Ready, "{body}");
        let body: serde_json::Value = serde_json::from_str(&body).expect("JSON");
        assert_eq!(
            body, vendor,
            "the mint door's 401 is the vendor's bad-key answer"
        );

        let (outcome, body) = refused(
            p,
            401,
            ReasonCode::HookVeto,
            "the hook says no",
            "/v1/realtime/client_secrets",
        );
        assert_eq!(outcome, Outcome::Ready, "{body}");
        let body: serde_json::Value = serde_json::from_str(&body).expect("JSON");
        assert_eq!(
            body["error"]["message"], "the hook says no",
            "a veto keeps its words"
        );
        assert_eq!(body["error"]["code"], serde_json::Value::Null);
    }
}

/// The kernel's `refusal` of `reason` for `unit` in `dialect`: the outcome and the body.
fn refused_unit(
    p: &Plugin<Plane>,
    unit: u64,
    dialect: u32,
    reason: busbar_contract::caps::ReasonCode,
    words: &'static str,
) -> (Outcome, String) {
    use busbar_contract::abi::plane::{reason_code, RefusalIn, RefusalOut, REFUSAL_KERNEL};
    let mut reply = vec![0u8; 1024];
    let mut fields: Vec<OutField> = vec![z(); 4];
    let mut arena = vec![0u8; 256];
    let mut f: Frame<RefusalIn, RefusalOut> = Frame::new(z(), z());
    (f.input.head, f.out.head) = (in_head(), out_head());
    f.input.cause = REFUSAL_KERNEL;
    f.input.status = 429;
    f.input.dialect = dialect;
    f.input.unit = unit;
    f.input.reason = reason_code(reason);
    f.input.text = text(words);
    (f.input.reply_buf, f.input.reply_cap) = (reply.as_mut_ptr(), reply.len());
    (f.input.fields_buf, f.input.fields_cap) = (fields.as_mut_ptr(), fields.len());
    (f.input.arena_buf, f.input.arena_cap) = (arena.as_mut_ptr(), arena.len());
    let c = p.call(slot::REFUSAL, &mut f);
    let written = usize::try_from(f.out.reply_written).expect("a length");
    (
        c.outcome,
        String::from_utf8_lossy(&reply[..written.min(reply.len())]).into_owned(),
    )
}

/// A live session reports a budget cut in the dialect's stream when supported.
#[test]
fn a_budget_cut_of_a_live_session_is_told_in_the_dialects_stream() {
    use busbar_contract::caps::ReasonCode;
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::new(1 << 16);
        let audio = uplink(100);
        for (unit, location) in [
            (130, "/v1/realtime/calls/rtc_both0130"),
            (132, "/v1/realtime/calls/rtc_both0132"),
        ] {
            assert_eq!(attach_sideband(p, unit, location), Outcome::Ready);
        }
        let _ = b.call(p, &session(130, FROM_CALLER, 0, &audio));
        let _ = b.call(p, &session(132, FROM_CALLER, 0, &audio));
        let _ = b.call(p, &session(132, FROM_CALLER, PIECE_LAST, &[]));
        let gemini = Push {
            claim: 3,
            ..session(
                131,
                FROM_CALLER,
                0,
                br#"{"realtimeInput":{"audioStreamEnd":true}}"#,
            )
        };
        let _ = b.call(p, &gemini);

        for unit in [130, 132] {
            let (outcome, body) = refused_unit(
                p,
                unit,
                0,
                ReasonCode::OverBudget,
                "the budget is exhausted",
            );
            assert_eq!(outcome, Outcome::Ready, "{body}");
            let v: serde_json::Value = serde_json::from_str(&body).expect("JSON");
            assert_eq!(v["type"], "error", "an in-stream event: {body}");
            assert_eq!(v["error"]["code"], "budget_exhausted", "{body}");
            assert_eq!(v["error"]["message"], "the budget is exhausted", "{body}");
        }

        let (outcome, body) =
            refused_unit(p, 131, 1, ReasonCode::OverBudget, "the budget is exhausted");
        assert_eq!(outcome, Outcome::Ready, "{body}");
        assert!(body.is_empty(), "no invented Gemini frame: {body}");

        let (outcome, body) =
            refused_unit(p, 999, 0, ReasonCode::OverBudget, "the budget is exhausted");
        assert_eq!(outcome, Outcome::Ready, "{body}");
        assert_eq!(
            body.as_bytes(),
            br#"{"error":{"code":"insufficient_quota","message":"the budget is exhausted","param":null,"type":"insufficient_quota"}}"#,
            "before upgrade the REST envelope identifies exhausted quota, not retryable throttling"
        );
        let (outcome, body) = refused_unit(p, 1000, 0, ReasonCode::RateLimited, "slow down");
        assert_eq!(outcome, Outcome::Ready, "{body}");
        assert_eq!(
            body.as_bytes(),
            refusal_body(429, "slow down").as_slice(),
            "a throttle remains distinguishable from exhausted quota at the same HTTP status"
        );
    }
}

/// One ticket-less `tick` at `now_ns` (`Plugin::call`): its outcome and the next tick it asks for
/// (`0` = none).
fn tick(p: &Plugin<Plane>, now_ns: u64) -> (Outcome, u64) {
    let mut k: Frame<TickIn, TickOut> = Frame::new(z(), z());
    k.input.head = in_head();
    k.input.now_ns = now_ns;
    k.out.head = out_head();
    let c = p.call(life::TICK, &mut k);
    (c.outcome, k.out.next_tick_ns)
}

/// One `tick` at `now_ns` on `driver`, the ticket the door names when it has work of its own, as
/// the host ticks it: submitted on that ticket through the dispatcher, which crosses with it (a
/// ticket-less `Plugin::call` crosses with `Ticket::NONE`, whatever its frame names).
fn tick_on(
    d: &Dispatcher,
    p: &Plugin<Plane>,
    driver: busbar_contract::abi::mechanism::ticket::Ticket,
    now_ns: u64,
) -> (Outcome, u64) {
    let mut k: Frame<TickIn, TickOut> = Frame::new(z(), z());
    k.input.head = in_head();
    k.input.now_ns = now_ns;
    k.out.head = out_head();
    let done = d
        .submit(
            p,
            driver,
            life::TICK,
            k,
            busbar_contract::abi::mechanism::call::DeadlineClass::Call,
            0,
        )
        .wait(std::time::Duration::from_secs(10))
        .expect("the tick answers");
    let next = done.frame.as_ref().map_or(0, |f| f.out.next_tick_ns);
    (done.outcome, next)
}

/// Whether the host saw a wake land on a retired ticket within a few seconds of `before` (its
/// count of such wakes when the test started waiting): the door named the ticket it was ticked on.
fn woke_retired(d: &Dispatcher, before: u64) -> bool {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < until {
        if d.stats().stale_wakes > before {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

/// `cancel` of `unit` by its key, as the host cancels a unit itself.
fn cancel_unit(p: &Plugin<Plane>, unit: u64) -> Outcome {
    let mut x: Frame<
        busbar_contract::abi::plane::PlaneCancelIn,
        busbar_contract::abi::plane::PlaneCancelOut,
    > = Frame::new(z(), z());
    x.input.cancel.head = in_head();
    x.input.unit = unit;
    x.out.cancel.head = out_head();
    p.call(life::CANCEL, &mut x).outcome
}

/// `cancel` of `unit` by its key: the disposition the door answered.
fn cancel_disposition(p: &Plugin<Plane>, unit: u64) -> u32 {
    let mut x: Frame<
        busbar_contract::abi::plane::PlaneCancelIn,
        busbar_contract::abi::plane::PlaneCancelOut,
    > = Frame::new(z(), z());
    x.input.cancel.head = in_head();
    x.input.unit = unit;
    x.out.cancel.head = out_head();
    assert_eq!(p.call(life::CANCEL, &mut x).outcome, Outcome::Ready);
    x.out.cancel.disposition
}

/// Cancellation after a far-end answer is recorded as partial work.
#[test]
fn a_cancel_after_the_far_end_answered_is_partial() {
    let d = host();
    for door in [linked as fn(&Dispatcher) -> Plugin<Plane>, dropped] {
        let p = door(&d);
        assert_eq!(open(&p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::new(1);
        let short = b.call(&p, &push(7, 0, FROM_CALLER, PIECE_LAST, &[]));
        assert!(
            short.contains("more=1") && short.contains("done=false"),
            "mint reply remains live: {short}"
        );
        assert_eq!(
            cancel_disposition(&p, 7),
            CANCEL_ABORTED,
            "cut before any far end answered: aborted"
        );
        assert_eq!(
            attach_sideband(&p, 17, "/v1/realtime/calls/rtc_both0017"),
            Outcome::Ready
        );
        let mut b = Bufs::with_units(1 << 18, 8);
        b.call(&p, &session(17, FROM_CALLER, 0, &uplink(100)));
        b.call(
            &p,
            &Push {
                attempt_no: 1,
                ..session(17, FROM_KERNEL, 0, &[])
            },
        );
        b.call(
            &p,
            &Push {
                status: 101,
                ..session(17, FROM_FAR_END, PIECE_HAS_STATUS, USAGE_DONE)
            },
        );
        assert_eq!(
            cancel_disposition(&p, 17),
            CANCEL_OK_PARTIAL,
            "cut after the far end answered: partial"
        );
    }
}

/// The brokered calls' published lifetime, in nanoseconds (OpenAI Realtime's 60 minutes).
const CALL_LIFETIME_NS: u64 = 60 * 60 * 1_000_000_000;

/// A brokered row expires on the tick clock even when no session ceiling is configured.
#[test]
fn a_brokered_row_expires_on_the_tick_clock_with_no_ceiling() {
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let driver = d.mint(0).expect("a ticket on worker 0");
        assert_eq!(
            tick_on(&d, p, driver, 5),
            (Outcome::Ready, 0),
            "nothing held, no ceiling: no tick is asked for"
        );
        d.recycle(driver);
        let before = d.stats().stale_wakes;
        let id = broker(p, 300, "/v1/realtime/calls/rtc_tick0300", CALLER_REF);
        let target = sideband_to(&id);
        assert!(
            woke_retired(&d, before),
            "recording the call wakes the driver ticket the door was ticked on"
        );

        let (outcome, next) = tick(p, 1_000_000_000);
        assert_eq!(outcome, Outcome::Ready);
        assert_eq!(
            next,
            1_000_000_000 + CALL_LIFETIME_NS,
            "the row's age starts at this tick, and the door asks to be ticked when it expires"
        );
        let mut b = Bufs::with_units(1 << 18, 8);
        assert_eq!(arrive_sideband(p, 301, target), Outcome::Ready);
        let live = b.call(p, &session(301, FROM_CALLER, 0, &uplink(100)));
        assert!(live.contains("far=true"), "inside its lifetime: {live}");
        assert_eq!(
            tick(p, next),
            (Outcome::Ready, 0),
            "the expired row is freed, and nothing is left to tick for"
        );
        assert_eq!(
            arrive_sideband(p, 302, target),
            Outcome::Refused,
            "past its lifetime, busbar holds the call no longer"
        );
        let expired = b.call(p, &session(302, FROM_CALLER, 0, &uplink(100)));
        assert!(
            expired.starts_with("Refused"),
            "past its lifetime: {expired}"
        );
    }
}

/// A sideband may reconnect to a live call after its earlier connection closes.
#[test]
fn a_sideband_reconnect_after_a_sideband_close_is_admitted() {
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let target = sideband_to(&broker(
            p,
            310,
            "/v1/realtime/calls/rtc_back0310",
            CALLER_REF,
        ));
        let mut b = Bufs::with_units(1 << 18, 8);

        assert_eq!(arrive_sideband(p, 311, target), Outcome::Ready);
        let first = b.call(p, &session(311, FROM_CALLER, 0, &uplink(100)));
        assert!(first.contains("far=true"), "{first}");
        let ended = b.call(p, &session(311, FROM_CALLER, PIECE_LAST, &[]));
        assert!(ended.contains("done=true"), "the caller ends it: {ended}");

        assert_eq!(arrive_sideband(p, 312, target), Outcome::Ready);
        let second = b.call(p, &session(312, FROM_CALLER, 0, &uplink(100)));
        assert!(
            second.contains("far=true"),
            "reconnects after the caller's end: {second}"
        );
        assert_eq!(cancel_unit(p, 312), Outcome::Ready);

        assert_eq!(arrive_sideband(p, 313, target), Outcome::Ready);
        let third = b.call(p, &session(313, FROM_CALLER, 0, &uplink(100)));
        assert!(
            third.contains("far=true"),
            "reconnects after a cancel: {third}"
        );
    }
}

/// `arrive` of `unit` on `claim` at `target`: its outcome.
fn arrive_at(p: &Plugin<Plane>, claim: u32, unit: u64, target: &'static str) -> Outcome {
    let mut a: Frame<ArriveIn, ArriveOut> = Frame::new(z(), z());
    a.input.head = in_head();
    (a.input.unit, a.input.claim) = (unit, claim);
    a.input.target = text(target);
    a.out.head = out_head();
    p.call(slot::ARRIVE, &mut a).outcome
}

/// The plain OpenAI Realtime socket's claim (`GET /v1/realtime`), after the five call-scoped doors.
const PLAIN_CLAIM: u32 = 5;

/// A plain Realtime session opens on its own door without a brokered call.
#[test]
fn a_plain_realtime_session_opens_on_its_own_door_with_no_call() {
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    let mut transcripts = Vec::new();
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::with_units(1 << 18, 8);
        let audio = uplink(100);

        assert_eq!(
            arrive_at(p, PLAIN_CLAIM, 50, "/v1/realtime?model=gpt-realtime"),
            Outcome::Ready,
            "the plain socket is a published door"
        );
        let initial = b.call(p, &live_push(50, PLAIN_CLAIM, FROM_CALLER, 0, &[]));
        assert!(
            initial.contains("session.update")
                && initial.contains("far=true")
                && initial.contains("verb=GET target=/v1/realtime?model=m-cap "),
            "{initial}"
        );
        let plain = b.call(
            p,
            &Push {
                stream: 50,
                ..push(50, PLAIN_CLAIM, FROM_CALLER, 0, &audio)
            },
        );
        assert!(
            plain.starts_with("Ready")
                && plain.contains("far=true")
                && plain.contains("verb=GET target=/v1/realtime?model=m-cap ")
                && plain.contains("input_audio_buffer.append"),
            "a plain session opens with no call and dials the realtime socket naming the \
             session's model (never the caller's own `?model=`): {plain}"
        );

        assert_eq!(
            arrive_at(p, 2, 51, "/v1/realtime/sideband/rtc_plain0051"),
            Outcome::Refused,
            "a call no offer placed is no door's"
        );
        let sideband = b.call(p, &session(51, FROM_CALLER, 0, &audio));
        assert!(
            sideband.starts_with("Refused"),
            "the sideband stays call-attached: a call no offer placed is refused: {sideband}"
        );
        transcripts.push([plain, sideband]);
    }
    assert_eq!(
        transcripts[0], transcripts[1],
        "the two doors must answer identically"
    );
}

/// Consumed sections arrive as one object keyed by section name, including streams and models.
const KEYED_UPSTREAM: &[u8] = br#"{"streams":{"session":{"model":"m-cap"}},"models":{"m-cap":{"provider":"p","upstream_model":"gpt-realtime-wire"}}}"#;
/// The same, the model entry naming no override.
const KEYED_PLAIN: &[u8] =
    br#"{"streams":{"session":{"model":"m-cap"}},"models":{"m-cap":{"provider":"p"}}}"#;

/// A plain session dials the configured session model's upstream name.
#[test]
fn a_plain_session_dials_the_session_models_upstream_name() {
    for (settings, want) in [
        (
            KEYED_UPSTREAM,
            "verb=GET target=/v1/realtime?model=gpt-realtime-wire ",
        ),
        (KEYED_PLAIN, "verb=GET target=/v1/realtime?model=m-cap "),
    ] {
        let d = Dispatcher::new(DispatchConfig::default());
        let doors = vec![linked(&d), dropped(&d)];
        let mut transcripts = Vec::new();
        for p in &doors {
            assert_eq!(
                open(p, 1, settings, Some(PUBLIC)).0,
                Outcome::Ready,
                "the door opens on the keyed settings"
            );
            let mut b = Bufs::with_units(1 << 18, 8);
            assert_eq!(
                arrive_at(p, PLAIN_CLAIM, 60, "/v1/realtime?model=m-cap"),
                Outcome::Ready
            );
            let plain = b.call(
                p,
                &Push {
                    stream: 60,
                    ..push(60, PLAIN_CLAIM, FROM_CALLER, 0, &[])
                },
            );
            assert!(
                plain.starts_with("Ready") && plain.contains(want),
                "the dial names the resolved upstream model ({want}): {plain}"
            );
            transcripts.push(plain);
        }
        assert_eq!(
            transcripts[0], transcripts[1],
            "the two doors must answer identically"
        );
    }
}

/// Every upstream-facing model field names the upstream model.
#[test]
fn every_upstream_facing_model_field_names_the_upstream_model() {
    let (d, host) = ticket_host();
    let doors = vec![linked(&d), dropped(&d)];
    let mut transcripts = Vec::new();
    for p in &doors {
        assert_eq!(open(p, 1, KEYED_UPSTREAM, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::with_units(1 << 18, 8);
        let ticket = b.call(p, &push(69, 0, FROM_CALLER, PIECE_LAST, &[]));
        assert!(ticket.contains("status=200"), "{ticket}");
        let record = String::from_utf8(
            host.minted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
        .expect("utf-8");
        assert!(
            record.contains(r#""model":"gpt-realtime-wire""#) && !record.contains("m-cap"),
            "the ticket locks the upstream model: {record}"
        );
        let _ = b.call(
            p,
            &Push {
                stream: 70,
                ..push(70, 1, FROM_CALLER, 0, &[])
            },
        );
        let mint = b.call(
            p,
            &Push {
                stream: 70,
                ..push(70, 1, FROM_KERNEL, 0, &[])
            },
        );
        assert!(
            mint.contains(r#""model":"gpt-realtime-wire""#) && !mint.contains("m-cap"),
            "busbar's call names the upstream model: {mint}"
        );

        assert_eq!(
            arrive_at(p, PLAIN_CLAIM, 71, "/v1/realtime?model=m-cap"),
            Outcome::Ready
        );
        let initial = b.call(p, &live_push(71, PLAIN_CLAIM, FROM_CALLER, 0, &[]));
        assert!(initial.contains("session.update"), "{initial}");
        let update = br#"{"type":"session.update","session":{"type":"realtime"}}"#;
        let spliced = b.call(
            p,
            &Push {
                stream: 71,
                ..push(71, PLAIN_CLAIM, FROM_CALLER, 0, update)
            },
        );
        assert!(
            spliced.contains("far=true")
                && spliced.contains(r#""model":"gpt-realtime-wire""#)
                && !spliced.contains(r#""model":"m-cap""#),
            "the caller's session.update reaches the provider with the upstream model: {spliced}"
        );

        assert_eq!(
            arrive_at(p, 3, 72, "/v1/realtime/gemini/call-72"),
            Outcome::Ready
        );
        let setup = br#"{"setup":{"model":"models/whatever"}}"#;
        let gemini = b.call(
            p,
            &Push {
                stream: 72,
                claim: 3,
                ..push(72, 3, FROM_CALLER, 0, setup)
            },
        );
        assert!(
            gemini.contains("far=true")
                && gemini.contains("gpt-realtime-wire")
                && !gemini.contains("m-cap"),
            "the Gemini Live setup reaches the provider with the upstream model: {gemini}"
        );
        transcripts.push([mint, spliced, gemini]);
    }
    assert_eq!(
        transcripts[0], transcripts[1],
        "the two doors must answer identically"
    );
}

/// The SDP door returns 201 and the ticket-bound call in `Location` for sideband attachment.
#[test]
fn the_sdp_door_places_busbars_call_locked_to_its_ticket_and_the_sideband_resolves_it() {
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::with_units(1 << 16, 8);

        let opened = b.call(
            p,
            &Push {
                stream: 50,
                ..push(50, 1, FROM_CALLER, 0, &[])
            },
        );
        let id = opened
            .split("location=/v1/realtime/calls/")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_else(|| panic!("the open names busbar's call: {opened}"))
            .to_string();
        assert!(
            id.starts_with("rtc_") && id.len() == 36,
            "busbar's own opaque call id: {opened}"
        );
        assert_eq!(
            opened,
            format!(
                "Ready emitted= more=0 far=false done=false status=201 verb= target= \
                 fields=[\"location=/v1/realtime/calls/{id}\", \"content-type=application/sdp\"]"
            )
        );

        let call = b.call(
            p,
            &Push {
                stream: 50,
                ..push(50, 1, FROM_KERNEL, 0, &[])
            },
        );
        assert!(call.contains("far=true"), "{call}");
        assert!(
            call.contains("verb=POST target=/v1/realtime/calls "),
            "{call}"
        );
        assert!(
            call.contains("content-type=multipart/form-data; boundary=busbar-call-form"),
            "{call}"
        );
        assert!(
            call.contains(&format!("OpenAI-Safety-Identifier={CALLER_REF}")),
            "the caller is named for the provider's attribution: {call}"
        );
        assert!(
            call.contains("from-ticket") && call.contains(r#""type":"realtime""#),
            "the session the ticket locked, in the GA shape: {call}"
        );
        assert_eq!(b.last_need, rides("webrtc", "bearer"));

        assert_eq!(
            b.last_flags & PIECE_OUT_TEXT,
            0,
            "busbar's call form is the open's body, never a text message: {call}"
        );

        let upstream = [Field {
            name: text("location"),
            value: text("/v1/realtime/calls/rtc_upstream0001"),
        }];
        let answered = b.call(
            p,
            &Push {
                stream: 50,
                status: 201,
                head: &upstream,
                ..push(50, 1, FROM_FAR_END, PIECE_HAS_STATUS, &[])
            },
        );
        assert!(answered.starts_with("Ready"), "{answered}");
        assert_eq!(b.last_units, "8:1=1", "the call's one fee: {answered}");

        let sideband = sideband_to(&id);
        assert_eq!(arrive_at(p, 2, 60, sideband), Outcome::Ready);
        let control = b.call(p, &session(60, FROM_CALLER, 0, &uplink(100)));
        assert!(
            control.contains("target=/v1/realtime?call_id=rtc_upstream0001 "),
            "{control}"
        );

        assert_eq!(arrive_at(p, 2, 62, sideband), Outcome::Ready);
        b.caller = "beef";
        let foreign = b.call(p, &session(62, FROM_CALLER, 0, &uplink(100)));
        assert!(foreign.starts_with("Refused"), "{foreign}");

        assert_eq!(
            arrive_at(p, 2, 61, "/v1/realtime/sideband/rtc_unknown"),
            Outcome::Refused
        );

        b.caller = CALLER_REF;
        let ended = b.call(
            p,
            &Push {
                stream: 50,
                ..push(50, 1, FROM_CALLER, PIECE_LAST, &[])
            },
        );
        assert!(ended.contains("done=true"), "{ended}");
        assert_eq!(arrive_at(p, 2, 63, sideband), Outcome::Refused);
    }
}

/// THE MINT DOOR'S TICKET, in the shape SDKs read: busbar mints it, dials nothing and reports no
/// units (row 132). Both ways.
#[test]
fn the_mint_door_answers_the_ticket_busbar_minted() {
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::new(256);
        let answer = b.call(p, &push(70, 0, FROM_CALLER, PIECE_LAST, &[]));
        let emitted = answer
            .strip_prefix("Ready emitted=")
            .and_then(|rest| rest.split(" more=").next())
            .expect("answered");
        let read: serde_json::Value = serde_json::from_str(emitted).expect("json");
        assert_eq!(
            read,
            serde_json::json!({"value": TICKET, "expires_at_unix": TICKET_EXPIRES})
        );
        assert!(
            answer.ends_with(&format!(
                "more=0 far=false done=true status=200 verb= target= \
                 fields=[\"content-type=application/json\", \"content-length={}\"]",
                emitted.len()
            )),
            "{answer}"
        );
        assert_eq!(b.last_units, "", "a ticket bills nothing");
        assert_eq!(b.last_need, 0, "and dials nothing");
    }
}

/// A mint outside the session model's grant is refused without issuing a ticket.
#[test]
fn a_mint_for_a_key_outside_the_session_models_grant_is_refused() {
    let (d, host) = ticket_host_granting(&["pool:another-model"]);
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::new(512);
        let answer = b.call(p, &push(71, 0, FROM_CALLER, PIECE_LAST, &[]));
        assert!(answer.contains("status=403"), "{answer}");
        assert!(answer.contains("not_granted"), "{answer}");
        assert!(answer.contains("m-cap"), "{answer}");
        assert!(!answer.contains(TICKET), "no ticket: {answer}");
        assert_eq!(b.last_units, "", "a refusal bills nothing");
        assert_eq!(b.last_need, 0, "and dials nothing");

        assert_eq!(
            b.last_audit,
            format!("rejected {MINT_AUDIT_ACTION} pool:m-cap"),
            "{answer}"
        );
        assert!(!b.last_audit.contains(CALLER_REF), "the key is never named");
    }
    assert!(
        host.minted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "nothing was minted"
    );
}

/// An SDP offer arriving as a plain request receives an explained 501 refusal.
#[test]
fn an_sdp_offer_arriving_as_a_plain_request_is_refused_501_explained() {
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        for first in [
            push(81, 1, FROM_CALLER, PIECE_LAST, b"v=0\r\n"),
            attempt(82, 1),
        ] {
            let mut b = Bufs::new(512);
            let answer = b.call(p, &first);
            assert_eq!(
                answer,
                format!(
                    "Ready emitted={SDP_PLAIN_REQUEST} more=0 far=false done=true status=501 \
                     verb= target= fields=[\"content-type=application/json\", \"content-length={}\"]",
                    SDP_PLAIN_REQUEST.len()
                )
            );
            assert_eq!(b.last_units, "", "a refusal bills nothing");
            assert_eq!(b.last_need, 0, "and dials nothing");
            assert_eq!(
                b.last_audit,
                format!("rejected {OFFER_AUDIT_ACTION} pool:m-cap")
            );
        }
    }
}

/// An SDP offer arriving at the call bound is refused without evicting live work.
#[test]
fn an_sdp_offer_at_the_call_bound_is_refused_at_its_arrival() {
    const MAX_BROKERED_CALLS: u64 = 4096;
    const MAX_CALLS_PER_CALLER: u64 = MAX_BROKERED_CALLS / 64;
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let first = broker(p, 1_000, "/v1/realtime/calls/rtc_bound0000", "caller-0");
        for i in 1..MAX_BROKERED_CALLS {
            let caller = leak(format!("caller-{}", i / MAX_CALLS_PER_CALLER));
            let _ = broker(p, 1_000 + i, "/v1/realtime/calls/rtc_bound0001", caller);
        }
        let mut a: Frame<ArriveIn, ArriveOut> = Frame::new(z(), z());
        a.input.head = in_head();
        (a.input.unit, a.input.claim) = (900, 1);
        a.input.target = text("/v1/realtime/calls");
        a.out.head = out_head();
        let c = p.call(slot::ARRIVE, &mut a);
        assert_eq!(c.outcome, Outcome::Refused);
        assert_eq!(a.out.refusal_status, 429);
        assert_eq!(
            read(a.out.head.error),
            "busbar is holding as many realtime calls as it admits at once; retry once one has ended"
        );

        assert_eq!(cancel_unit(p, 1_001), Outcome::Ready);
        assert_eq!(
            arrive_sideband(p, 901, sideband_to(&first)),
            Outcome::Ready,
            "the oldest caller's call is kept"
        );
    }
}

/// A request arriving at a full unit table receives an explained 429 refusal.
#[test]
fn a_request_unit_at_the_unit_bound_is_refused_at_its_arrival_in_words() {
    const MAX_UNITS: u64 = 4096;
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::new(256);
        for unit in 1..=MAX_UNITS {
            assert!(
                b.call(p, &attempt(unit, 0)).starts_with("Ready "),
                "unit {unit}"
            );
        }
        let arrive_mint = |unit: u64| {
            let mut a: Frame<ArriveIn, ArriveOut> = Frame::new(z(), z());
            a.input.head = in_head();
            (a.input.unit, a.input.claim) = (unit, 0);
            a.input.target = text("/v1/realtime/client_secrets");
            a.out.head = out_head();
            let c = p.call(slot::ARRIVE, &mut a);
            (
                c.outcome,
                a.out.refusal,
                a.out.refusal_status,
                read(a.out.head.error),
            )
        };
        assert_eq!(
            arrive_mint(MAX_UNITS + 1),
            (
                Outcome::Refused,
                4,
                429,
                "busbar is holding as many realtime calls as it admits at once; retry once one has ended"
                    .to_string()
            )
        );
        let caller = push(1, 0, FROM_CALLER, PIECE_LAST, &[]);
        assert!(b.call(p, &caller).contains("done=true"), "unit 1 ends");
        assert_eq!(
            arrive_mint(MAX_UNITS + 1).0,
            Outcome::Ready,
            "its room is free"
        );
    }
}

/// The host's envelope sink, keeping the text of every diagnostic and log record a call carried:
/// what the host writes to the plugin's own log file.
#[derive(Default)]
struct HostLog(
    std::sync::Mutex<Vec<String>>,
    std::sync::Mutex<Vec<Vec<u8>>>,
    std::sync::atomic::AtomicBool,
);

impl busbar_plugin_loader::dispatch::EnvelopeSink for HostLog {
    fn metric(&self, metric: busbar_plugin_loader::dispatch::Metric<'_>) {
        self.1
            .lock()
            .unwrap()
            .extend(metric.labels.iter().map(|value| value.to_vec()));
    }
    fn diag(&self, d: busbar_plugin_loader::dispatch::Diagnostic<'_>) {
        self.1
            .lock()
            .unwrap()
            .extend([d.name.to_vec(), d.text.to_vec()]);
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(String::from_utf8_lossy(d.text).into_owned());
    }
    fn dropped(&self, _: busbar_plugin_loader::dispatch::Dropped) {
        self.2.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A far end's usage report whose input count is a string, not a count.
const USAGE_UNREADABLE: &[u8] =
    br#"{"type":"response.done","response":{"usage":{"input_tokens":"1500","output_tokens":9}}}"#;

/// Unreadable usage bills the floor and reports the reason to the host log.
#[test]
fn an_unreadable_usage_bills_the_floor_and_its_line_reaches_the_host_log() {
    let d = host();
    let floor = (USAGE_UNREADABLE.len() as u64 / 4).max(1);
    let stated = row().statement;
    let path = plugin_cdylib();
    for dropped_in in [false, true] {
        let log = Arc::new(HostLog::default());
        let bound = Bind {
            sink: log.clone(),
            ..bind(
                &d,
                if dropped_in {
                    "dropped-log"
                } else {
                    "linked-log"
                },
            )
        };
        let p = if dropped_in {
            load_dropped::<Plane>(&path, &stated, bound).expect("the dropped streaming door loads")
        } else {
            load_linked::<Plane>(&row(), bound).expect("the linked streaming door loads")
        };
        assert_eq!(open(&p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        assert_eq!(
            arrive_at(&p, PLAIN_CLAIM, 70, "/v1/realtime?model=m-cap"),
            Outcome::Ready
        );
        let mut b = Bufs::with_units(1 << 18, 8);
        b.call(
            &p,
            &live_push(70, PLAIN_CLAIM, FROM_CALLER, 0, &uplink(100)),
        );
        b.call(
            &p,
            &Push {
                attempt_no: 1,
                ..live_push(70, PLAIN_CLAIM, FROM_KERNEL, 0, &[])
            },
        );
        let far = Push {
            status: 101,
            ..live_push(
                70,
                PLAIN_CLAIM,
                FROM_FAR_END,
                PIECE_HAS_STATUS,
                USAGE_UNREADABLE,
            )
        };
        let answer = b.call(&p, &far);
        assert!(
            answer.contains("done=false"),
            "dropped-in {dropped_in}: the session goes on: {answer}"
        );
        assert!(
            b.last_units
                .split(' ')
                .any(|u| u == format!("3:{}={floor}", busbar_contract::abi::plane::UNITS_FLOOR)),
            "dropped-in {dropped_in}: the turn's output tokens are the floor: {}",
            b.last_units
        );
        let lines = log
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.contains("usage is unreadable"))
                .count(),
            1,
            "dropped-in {dropped_in}: one line reaches the host log: {lines:?}"
        );
        assert!(
            log.1.lock().unwrap().iter().any(|record| record
                .windows(b"usage is unreadable".len())
                .any(|part| part == b"usage is unreadable")),
            "both actual images positively exercise the complete envelope byte capture"
        );
        assert!(
            !log.2.load(std::sync::atomic::Ordering::SeqCst),
            "the diagnostic capture must not discard any envelope record"
        );
    }
}

/// A refused session upgrade reports no session fee and cancels as aborted.
#[test]
fn a_refused_upgrade_reports_no_session_fee_and_cancels_aborted() {
    const REFUSAL: &[u8] =
        br#"{"error":{"message":"Incorrect API key provided.","type":"invalid_request_error"}}"#;
    let d = host();
    for door in [linked as fn(&Dispatcher) -> Plugin<Plane>, dropped] {
        let p = door(&d);
        assert_eq!(open(&p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        for (stream, claim, status, fee, disposition) in [
            (31, 5, 401, false, CANCEL_ABORTED),
            (32, 5, 429, false, CANCEL_ABORTED),
            (33, 3, 503, false, CANCEL_ABORTED),
            (34, 5, 101, true, CANCEL_OK_PARTIAL),
            (35, 3, 101, true, CANCEL_OK_PARTIAL),
        ] {
            let piece = |from, flags, bytes| Push {
                stream,
                ..push(stream, claim, from, flags, bytes)
            };
            let mut b = Bufs::with_units(1 << 18, 8);
            b.call(&p, &piece(FROM_CALLER, 0, &[]));
            let body = if fee { &[][..] } else { REFUSAL };
            let answered = b.call(
                &p,
                &Push {
                    status,
                    ..piece(FROM_FAR_END, PIECE_HAS_STATUS, body)
                },
            );
            assert!(
                answered.contains("done=false"),
                "cancel must target a live session: {answered}"
            );
            assert_eq!(
                b.last_units.contains("8:1=1"),
                fee,
                "status {status} on claim {claim}: {answered} units=[{}]",
                b.last_units
            );
            assert_eq!(
                cancel_disposition(&p, stream),
                disposition,
                "status {status} on claim {claim}"
            );
        }
    }
}

/// A short SDP open answer retains its 201 status and `Location` on recall.
#[test]
fn an_sdp_open_answered_short_answers_its_201_and_location_on_the_re_call() {
    let d = host();
    for door in [linked as fn(&Dispatcher) -> Plugin<Plane>, dropped] {
        let p = door(&d);
        assert_eq!(open(&p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let offer = Push {
            stream: 81,
            ..push(81, 1, FROM_CALLER, 0, &[])
        };
        let mut b = Bufs::with_units(1 << 16, 8);
        b.fields_cap = 0;
        let short = b.call(&p, &offer);
        assert!(short.starts_with("Failed"), "{short}");
        b.fields_cap = 4;
        let whole = b.call(&p, &offer);
        assert!(
            whole.starts_with("Ready") && whole.contains("status=201"),
            "{whole}"
        );
        assert!(
            whole.contains("location=/v1/realtime/calls/rtc_"),
            "busbar's call in Location: {whole}"
        );
        assert!(
            whole.contains("content-type=application/sdp"),
            "the SDP response type survives a short-output recall: {whole}"
        );
    }
}

/// Recalling a short SDP projection preserves the ticket's locked session.
#[test]
fn a_short_sdp_projection_re_called_keeps_the_tickets_session() {
    for door in [linked as fn(&Dispatcher) -> Plugin<Plane>, dropped] {
        let host = Arc::new(TicketHost {
            draws: std::sync::atomic::AtomicU64::new(0xa0),
            minted: std::sync::Mutex::new(Vec::new()),
            granted: &["pool:m-cap"],
            once: true,
            took: std::sync::atomic::AtomicBool::new(false),
        });
        let d = Dispatcher::with_services(DispatchConfig::default(), host);
        let p = door(&d);
        assert_eq!(open(&p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let rewrite: &[u8] = br#"{"voice":"marin"}"#;
        let (short, _, _) = project_at(&p, 1, 50, rewrite, 0);
        assert_eq!(short, Outcome::Failed, "answered short");
        let (outcome, body, rewritten) = project_at(&p, 1, 50, rewrite, 4096);
        assert_eq!(outcome, Outcome::Ready);
        assert!(rewritten);
        let view: serde_json::Value = serde_json::from_str(&body).expect("the projected params");
        assert_eq!(view["voice"], "marin", "{body}");
        assert_eq!(
            view["instructions"], "from-ticket",
            "the ticket's session, under the rewrite: {body}"
        );
    }
}

/// A session arriving at the session bound is refused with an explanation.
#[test]
fn a_session_at_the_session_bound_is_refused_at_its_arrival_in_words() {
    const MAX_SESSIONS: u64 = 4096;
    const PLAIN: u32 = 5;
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::new(256);
        let piece = |stream: u64, flags: u32| Push {
            stream,
            ..push(stream, PLAIN, FROM_CALLER, flags, &[])
        };
        for stream in 1..=MAX_SESSIONS {
            assert!(
                b.call(p, &piece(stream, 0)).starts_with("Ready "),
                "{stream}"
            );
        }
        let arrive_plain = |unit: u64| {
            let mut a: Frame<ArriveIn, ArriveOut> = Frame::new(z(), z());
            a.input.head = in_head();
            (a.input.unit, a.input.claim) = (unit, PLAIN);
            a.input.target = text("/v1/realtime?model=gpt-realtime");
            a.out.head = out_head();
            let c = p.call(slot::ARRIVE, &mut a);
            (
                c.outcome,
                a.out.refusal,
                a.out.refusal_status,
                read(a.out.head.error),
            )
        };
        assert_eq!(
            arrive_plain(MAX_SESSIONS + 1),
            (
                Outcome::Refused,
                5,
                429,
                "busbar is holding as many realtime calls as it admits at once; retry once one has ended"
                    .to_string()
            )
        );
        assert!(
            b.call(p, &piece(1, PIECE_LAST)).contains("done=true"),
            "session 1 ends"
        );
        assert_eq!(arrive_plain(MAX_SESSIONS + 1).0, Outcome::Ready);
    }
}

/// An inapplicable session-open rewrite leaves the original open unchanged.
#[test]
fn a_session_open_rewrite_that_cannot_be_applied_leaves_the_open_unmodified() {
    let d = Dispatcher::new(DispatchConfig::default());
    for p in &[linked(&d), dropped(&d)] {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let (_, locked, _) = project(p, 5, b"");
        for rewrite in [&br#"{"voice":7}"#[..], b"not json", b"[1,2]"] {
            assert_eq!(
                project(p, 5, rewrite),
                (Outcome::Ready, locked.clone(), false),
                "{}",
                String::from_utf8_lossy(rewrite)
            );
        }
    }
}

#[test]
fn an_unapplied_rewrite_is_logged_once_when_projection_needs_more_room() {
    let d = Dispatcher::new(DispatchConfig::default());
    for dropped_in in [false, true] {
        let log = Arc::new(HostLog::default());
        let bound = Bind {
            sink: log.clone(),
            ..bind(
                &d,
                if dropped_in {
                    "dropped-short-rewrite"
                } else {
                    "linked-short-rewrite"
                },
            )
        };
        let p = if dropped_in {
            dropped_bound(bound)
        } else {
            load_linked::<Plane>(&row(), bound).expect("the linked streaming door loads")
        };
        assert_eq!(open(&p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let rewrite = br#"{"voice":7}"#;
        let (outcome, _, _, recall) = project_recall_at(&p, 5, 732, rewrite, 0, None);
        assert_eq!(outcome, Outcome::Failed);
        assert!(
            !log.0
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.contains("session-open hook")),
            "a short projection has not completed the hook stage"
        );
        let recall = recall.expect("the short answer grants one recall");
        let (outcome, _, rewritten, recall) =
            project_recall_at(&p, 5, 732, rewrite, 4096, Some(recall));
        assert_eq!(outcome, Outcome::Ready);
        assert!(!rewritten);
        assert!(recall.is_none());
        let lines = log.0.lock().unwrap();
        let warnings: Vec<_> = lines
            .iter()
            .filter(|line| line.contains("session-open hook"))
            .collect();
        assert_eq!(warnings.len(), 1, "dropped-in {dropped_in}: {lines:?}");
        assert!(warnings[0].contains("not a session config"), "{warnings:?}");
    }
}

#[test]
fn a_later_session_rewrite_keeps_prior_patches_through_short_recall_and_refusal() {
    let d = Dispatcher::new(DispatchConfig::default());
    for make in [linked as fn(&Dispatcher) -> Plugin<Plane>, dropped] {
        let p = make(&d);
        assert_eq!(open(&p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        assert_eq!(
            project_at(&p, 5, 733, br#"{"voice":"marin"}"#, 4096).0,
            Outcome::Ready
        );
        let patch = br#"{"instructions":"from-second-hook"}"#;
        let (outcome, _, _, recall) = project_recall_at(&p, 5, 733, patch, 0, None);
        assert_eq!(outcome, Outcome::Failed);
        let (outcome, body, rewritten, _) = project_recall_at(
            &p,
            5,
            733,
            patch,
            4096,
            Some(recall.expect("short grants recall")),
        );
        assert_eq!(outcome, Outcome::Ready);
        assert!(rewritten);
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["voice"], "marin", "the first hook survives: {body}");
        assert_eq!(view["instructions"], "from-second-hook");
        let (outcome, body, rewritten) = project_at(&p, 5, 733, br#"{"voice":7}"#, 4096);
        assert_eq!(outcome, Outcome::Ready);
        assert!(!rewritten);
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            view["voice"], "marin",
            "invalid later rewrite keeps prior patches"
        );
        assert_eq!(view["instructions"], "from-second-hook");
    }
}

/// A native client can wait for session.created. The host's initial empty crossing must dial
/// and install governance before any audio; a short buffer replays that control exactly once.
#[test]
fn native_initialization_survives_short_buffers_before_any_provider_work() {
    const LOCK: &[u8] = br#"{"session":{"model":"m-cap","instructions":"operator lock","input_audio_format":"g711_ulaw"},"max_output_tokens":127,"context_window_tokens":2048}"#;
    let d = Dispatcher::new(DispatchConfig::default());
    let mut transcripts = Vec::new();
    for p in [linked(&d), dropped(&d)] {
        assert_eq!(open(&p, 1, LOCK, Some(PUBLIC)).0, Outcome::Ready);
        assert_eq!(
            arrive_at(&p, PLAIN_CLAIM, 301, "/v1/realtime?model=m-cap"),
            Outcome::Ready
        );
        let mut b = Bufs::with_units(1, 9);
        let short = b.call(&p, &live_push(301, PLAIN_CLAIM, FROM_CALLER, 0, &[]));
        assert!(
            short.starts_with("Ready") && short.contains("more=1"),
            "{short}"
        );
        assert!(
            b.last_units.is_empty(),
            "initialization incurs no audio or accepted-session fee"
        );
        b.reply.resize(1 << 18, 0);
        let initial = b.call(&p, &live_push(301, PLAIN_CLAIM, FROM_CALLER, 0, &[]));
        assert!(
            initial.contains("session.update")
                && initial.contains("operator lock")
                && initial.contains("audio/pcmu")
                && initial.contains(r#""max_output_tokens":127"#)
                && initial.contains(r#""post_instructions":1921"#),
            "{initial}"
        );
        assert_eq!(
            b.last_flags & (EMIT_TO_FAR_END | PIECE_OUT_TEXT),
            EMIT_TO_FAR_END | PIECE_OUT_TEXT
        );
        let quiet = b.call(&p, &live_push(301, PLAIN_CLAIM, FROM_KERNEL, 0, &[]));
        assert!(quiet.contains("emitted= more=0 far=false"), "{quiet}");
        let sample = br#"{"type":"input_audio_buffer.append","audio":"AA=="}"#;
        let audio = b.call(&p, &live_push(301, PLAIN_CLAIM, FROM_CALLER, 0, sample));
        assert!(
            audio.contains(std::str::from_utf8(sample).unwrap()),
            "{audio}"
        );
        assert_eq!(
            b.last_units, "4:1=1",
            "fractional millisecond uses the installed mu-law format"
        );
        let end = b.call(
            &p,
            &live_push(301, PLAIN_CLAIM, FROM_CALLER, PIECE_LAST, &[]),
        );
        assert!(end.contains("done=true"), "{end}");
        assert_eq!(
            b.last_units, "4:1=1",
            "settlement does not add a second rounded turn"
        );
        transcripts.push([short, initial, quiet, audio, end]);
    }
    assert_eq!(transcripts[0], transcripts[1]);
}

#[test]
fn gemini_original_proto_audio_names_relay_exact_bytes_and_bill_fractional_samples() {
    let d = Dispatcher::new(DispatchConfig::default());
    let mut transcripts = Vec::new();
    for p in [linked(&d), dropped(&d)] {
        assert_eq!(open(&p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        assert_eq!(
            arrive_at(&p, 3, 302, "/v1/realtime/gemini/aliases"),
            Outcome::Ready
        );
        let mut b = Bufs::with_units(1 << 18, 9);
        let setup = b.call(&p, &live_push(302, 3, FROM_CALLER, 0, br#"{"setup":{}}"#));
        assert!(setup.starts_with("Ready"), "{setup}");
        let frame = br#"{ "realtime_input" : {"media_chunks":[{"mime_type":"audio/pcm;rate=16000","data":"AAA="}]}, "opaque-key" : "untouched" }"#;
        let audio = b.call(&p, &live_push(302, 3, FROM_CALLER, 0, frame));
        assert!(
            audio.contains(std::str::from_utf8(frame).unwrap()),
            "{audio}"
        );
        assert_eq!(b.last_units, "4:1=1");
        let end = b.call(&p, &live_push(302, 3, FROM_CALLER, PIECE_LAST, &[]));
        assert!(end.contains("done=true"), "{end}");
        assert_eq!(b.last_units, "4:1=1");
        transcripts.push([setup, audio, end]);
    }
    assert_eq!(transcripts[0], transcripts[1]);
}

#[test]
fn cancelling_a_short_initial_lock_retires_the_unaccepted_session() {
    let d = Dispatcher::new(DispatchConfig::default());
    for p in [linked(&d), dropped(&d)] {
        assert_eq!(open(&p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        assert_eq!(
            arrive_at(&p, PLAIN_CLAIM, 303, "/v1/realtime?model=m-cap"),
            Outcome::Ready
        );
        let mut b = Bufs::with_units(1, 9);
        let short = b.call(&p, &live_push(303, PLAIN_CLAIM, FROM_CALLER, 0, &[]));
        assert!(short.contains("more=1"), "{short}");
        assert!(b.last_units.is_empty());
        assert_eq!(cancel_disposition(&p, 303), CANCEL_ABORTED);
        b.reply.resize(1 << 18, 0);
        let collect = b.call(&p, &live_push(303, PLAIN_CLAIM, FROM_KERNEL, 0, &[]));
        assert!(
            collect.starts_with("Refused") && !collect.contains("session.update"),
            "{collect}"
        );
    }
}

#[test]
fn native_initialization_uses_the_completed_session_open_hook_rewrite() {
    let d = Dispatcher::new(DispatchConfig::default());
    for p in [linked(&d), dropped(&d)] {
        assert_eq!(open(&p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        assert_eq!(
            arrive_at(&p, PLAIN_CLAIM, 304, "/v1/realtime?model=m-cap"),
            Outcome::Ready
        );
        let (outcome, _, rewritten) = project_at(
            &p,
            PLAIN_CLAIM,
            304,
            br#"{"instructions":"hook instruction","voice":"marin"}"#,
            4096,
        );
        assert_eq!(outcome, Outcome::Ready);
        assert!(rewritten);
        let mut b = Bufs::with_units(1 << 18, 9);
        let initial = b.call(&p, &live_push(304, PLAIN_CLAIM, FROM_CALLER, 0, &[]));
        assert!(
            initial.contains("hook instruction") && initial.contains(r#""voice":"marin""#),
            "{initial}"
        );
    }
}

#[path = "support/artifact.rs"]
mod artifact;
fn plugin_cdylib() -> std::path::PathBuf {
    artifact::plugin_cdylib()
}
#[path = "support/pinned_cases.rs"]
mod pinned_cases;

/// Caller-bound text is marked complete only after the final reply-window fragment.
#[test]
fn caller_bound_text_flags_follow_reply_window_completion() {
    let d = host();
    for door in [linked as fn(&Dispatcher) -> Plugin<Plane>, dropped] {
        let p = door(&d);
        assert_eq!(open(&p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::with_units(1 << 18, 8);
        let initial = b.call(&p, &live_push(410, PLAIN_CLAIM, FROM_CALLER, 0, &[]));
        assert!(initial.starts_with("Ready"), "{initial}");
        let wire = serde_json::to_vec(
            &serde_json::json!({"type":"response.output_text.delta","delta":"x".repeat(128)}),
        )
        .unwrap();
        b.reply.resize(17, 0);
        let mut input = Push {
            status: 101,
            ..live_push(410, PLAIN_CLAIM, FROM_FAR_END, PIECE_HAS_STATUS, &wire)
        };
        let mut received = Vec::new();
        let mut complete = false;
        for _ in 0..64 {
            let mut frame = b.frame(&input);
            assert_eq!(p.call(slot::ON_PIECE, &mut frame).outcome, Outcome::Ready);
            let out = frame.out;
            assert_eq!(out.flags & EMIT_TO_FAR_END, 0);
            assert!(out.emitted > 0);
            received.extend_from_slice(&b.reply[..out.emitted as usize]);
            if out.more != 0 {
                assert_eq!(out.flags & PIECE_OUT_TEXT, 0);
            } else {
                assert_ne!(out.flags & PIECE_OUT_TEXT, 0);
                complete = true;
                break;
            }
            input = live_push(410, PLAIN_CLAIM, FROM_FAR_END, 0, &[]);
        }
        assert!(complete, "the final text fragment was never emitted");
        assert_eq!(received, wire);
    }
}

#[test]
fn a_sideband_projects_no_new_session_parameters_and_rejects_session_patches() {
    let d = Dispatcher::new(DispatchConfig::default());
    for p in &[linked(&d), dropped(&d)] {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        assert_eq!(project(p, 2, b""), (Outcome::Ready, String::new(), false));
        assert_eq!(project(p, 2, br#"{"voice":"marin"}"#).0, Outcome::Refused);
        assert_eq!(
            project(
                p,
                2,
                br#"{"messages":[{"role":"user","content":"replace"}],"tools":[]}"#
            ),
            (Outcome::Ready, String::new(), false)
        );
    }
}
