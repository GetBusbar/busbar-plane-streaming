// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE STREAMING PLANE'S DOOR, BOTH WAYS (#2 (4)/(5): the plugin's own both-ways witness, over the
//! real loader): the same door compiled in (`busbar_plane_streaming::door`, through
//! `load_linked`) and dropped in (this crate's cdylib, `busbar_plane_streaming_plugin`, which `cargo
//! test` builds beside the test, through `load_dropped`), driven by ONE script over the plane kind.
//! It moved here with the crate from busbar's `crates/busbar-plane-streaming/tests/conformance.rs`;
//! the published suite is this crate's `tests/conformance.rs`. Every answer is judged
//! by the kind's own checks on the way; the two transcripts must be identical, and each is pinned.

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
        // These rows drive the door's slots and never dial: its needs are not declared, bound
        // as a probe.
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

/// The mint door's 403 when the host does not entitle the key to the session's model (R10 grant
/// ruling): a host that serves no `entitlement.check` entitles nothing, so no ticket is asked for
/// (the logic crate's `request_unit_tests` pins the envelope).
/// The audit action a mint door's refusal is recorded under.
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
    let exe = std::env::current_exe().expect("the test binary has a path");
    let deps = exe.parent().expect("target/<profile>/deps");
    let profile = deps.parent().expect("target/<profile>");
    let file = busbar_plugin_loader::plugin_library_filename("busbar_plane_streaming_plugin");
    let path = [profile.join(&file), deps.join(&file)]
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| panic!("this crate's cdylib ({file}) is not built"));
    let stated = row().statement;
    load_dropped::<Plane>(&path, &stated, bind(d, "dropped"))
        .expect("the dropped streaming door loads")
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
    // The stage-3g deal's shape, `{streams: <section>}` (the boot's validate hands this).
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
        // A fresh instance of each door per script: the script opens its instance, and an open
        // instance refuses a second open.
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
    // The billable classes a session counts in, the per-session fee and the `session` grant kind:
    // the served plane's own declaration, read off the tail the door states.
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
    // The fee unit is one of the billable classes (ARCHITECT Q-L5-FEE (A)+(C)): the plane can
    // report it, and the loader's bind-time check admits it.
    // SAFETY: as above.
    let (fee_units, billable) = unsafe {
        (
            std::slice::from_raw_parts(t.fee_units_ptr(), t.fee_units_len()),
            std::slice::from_raw_parts(t.billable_classes_ptr(), t.billable_classes_len()),
        )
    };
    busbar_contract::abi::plane::check::check_fee_units(fee_units, billable)
        .expect("every fee unit is a billable class");
}

// ── THE ONE-REQUEST DOORS' PIECES ───────────────────────────────────────────────────────────────

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
    fields: [OutField; 4],
    arena: [u8; 256],
    /// The units the last answer wrote (class, source, amount), or, short, the room it needed.
    last_units: String,
    /// The need the last answer named for its far request (`0` = none).
    last_need: u32,
    records: [RecordWrite; 2],
    /// The flags the last answer stated (`EMIT_*`, `PIECE_OUT_TEXT`).
    last_flags: u32,
    /// The audit rows the last answer wrote (`outcome action resource`), or, short, the room it
    /// needed.
    last_audit: String,
    /// The caller reference the kernel lends each piece.
    caller: &'static str,
}

/// One piece pushed to a unit: whose it is, its flags, bytes, status and kept head fields.
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
            fields: [z(); 4],
            arena: [0; 256],
            last_units: String::new(),
            last_need: 0,
            records: [z(); 2],
            last_flags: 0,
            last_audit: String::new(),
            caller: CALLER_REF,
        }
    }

    /// `push` answered: the outcome and what it wrote, as text.
    fn call(&mut self, p: &Plugin<Plane>, push: &Push<'_>) -> String {
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
        (i.fields_buf, i.fields_cap) = (self.fields.as_mut_ptr(), self.fields.len());
        (i.arena_buf, i.arena_cap) = (self.arena.as_mut_ptr(), self.arena.len());
        (i.records_buf, i.records_cap) = (self.records.as_mut_ptr(), self.records.len());
        let mut o: OnPieceOut = z();
        o.head = out_head();
        let mut f = Frame::new(i, o);
        let c = p.call(slot::ON_PIECE, &mut f);
        let o = f.out;
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

/// The ATTEMPT that opens `unit`'s first attempt.
const fn attempt(unit: u64, claim: u32) -> Push<'static> {
    Push {
        attempt_no: 1,
        ..push(unit, claim, FROM_KERNEL, 0, &[])
    }
}

/// THE UNIT SCRIPT: the mint (busbar mints the ticket itself and dials nothing; the dispatcher here
/// serves no ticket, so the mint is the served plane's 502), the SDP door's request piece (it opens a
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
    let d = Dispatcher::new(DispatchConfig::default());
    assert_eq!(unit_script(&linked(&d)), unit_expected());
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

/// A mint answered through a reply buffer of 16 bytes: the ticket busbar minted (row 132), a
/// buffer at a time, its `more` re-calls (no bytes, no flags) paying the rest, `done` only with the
/// last byte.
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
    // The sealed answer's members go out in name order.
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

// ── LIVE SESSIONS THROUGH THE DOOR (K6) ──────────────────────────────────────────────────────────

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

// ── BUSBAR TERMINATES THE BROWSER'S MEDIA (row 132) ──────────────────────────────────────────────

/// A host that mints and takes tickets and draws randomness: unit 50 was admitted by a ticket whose
/// record locks `instructions: from-ticket`; every other unit by a key. Each draw of randomness is
/// a fresh count, so every call busbar places has its own id.
struct TicketHost {
    draws: std::sync::atomic::AtomicU64,
    /// The record the last mint stored beside its ticket.
    minted: std::sync::Mutex<Vec<u8>>,
    /// The `pool:<entry>` targets the key is granted (`entitlement.check`).
    granted: &'static [&'static str],
}

/// The record unit 50's ticket locks: a session config naming its instructions.
const TICKET_RECORD: &[u8] = br#"{"instructions":"from-ticket"}"#;

impl busbar_contract::services::double::ServicesDouble for TicketHost {
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
        busbar_contract::services::Stored {
            bytes: TICKET.as_bytes().to_vec(),
            ..busbar_contract::services::Stored::ready(TICKET_EXPIRES)
        }
    }

    fn ticket_take(
        &self,
        _caller: &busbar_contract::services::Caller,
        unit: Option<u64>,
    ) -> busbar_contract::services::Stored {
        use busbar_contract::abi::host::service::{TICKET_NONE, TICKET_TAKEN};
        if unit == Some(50) {
            busbar_contract::services::Stored {
                bytes: TICKET_RECORD.to_vec(),
                ..busbar_contract::services::Stored::ready(TICKET_TAKEN)
            }
        } else {
            busbar_contract::services::Stored::ready(TICKET_NONE)
        }
    }

    fn random_fill(&self, len: u64) -> busbar_contract::services::Stored {
        let draw = self
            .draws
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_le_bytes();
        busbar_contract::services::Stored {
            bytes: draw
                .iter()
                .copied()
                .cycle()
                .take(usize::try_from(len).expect("a length"))
                .collect(),
            ..busbar_contract::services::Stored::ready(0)
        }
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
    let mut narrow = Bufs::with_units(1 << 18, 2);
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
        "the caller's frame is one turn toward the far end, its audio unchanged, and the open \
         turn's two running seconds are reported with it: {caller}"
    );
    assert_eq!(
        line("turn attempt"),
        "turn attempt Ready emitted= more=0 far=false done=false status=0 verb= target= \
         fields=[] units=[4:1=2]",
        "the turn's request is the caller side's answer"
    );
    let far = line("far usage");
    assert!(
        far.contains("far=false done=false")
            && far.ends_with("units=[0:1=10 1:1=20 2:1=3 3:1=4 4:1=2]"),
        "the closed turn's tokens and its two admitted seconds, and no fee unit: the call's fee \
         was the SDP offer's: {far}"
    );
    assert!(line("collect ").contains("emitted= more=0 far=false done=false"));
    assert!(
        line("short ").starts_with("short Failed") && line("short ").ends_with("units=[needed 5]"),
        "{}",
        line("short ")
    );
    let recall = line("short re-call");
    assert!(
        recall.contains("far=true") && recall.ends_with("units=[0:1=10 1:1=20 2:1=3 3:1=4 4:1=4]"),
        "the re-call answers the same piece once, its audio not counted twice (the open turn's \
         two running seconds over the closed turn's two): {recall}"
    );
    let end = line("caller end");
    assert!(
        end.contains("done=true") && end.ends_with("units=[0:1=10 1:1=20 2:1=3 3:1=4 4:1=4]"),
        "the caller's end settles the open turn's two seconds once: {end}"
    );
    assert!(line("request unit on a session door")
        .starts_with("request unit on a session door Refused"));
}

/// RED-BEFORE-GREEN (the provider's sideband connection, OpenAI's "Webhooks and server-side
/// controls" guide: a server attaches to a WebRTC call's own session at
/// `wss://api.openai.com/v1/realtime?call_id=rtc_xxxxx`, the call id from the call's `Location`):
/// the sideband door dials the provider's call that busbar's call its arrival's `{call_id}` names
/// resolves to (row 132), and a session whose arrival named no call is refused, never dialled as a
/// fresh session; an arrival naming no call busbar holds is no door's (a 404). RED: the
/// `{call_id}` was never read and every sideband dialled `/v1/realtime`, an unrelated session.
/// Both ways.
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

/// RED-BEFORE-GREEN (THE DESIGN #45's browser trust boundary; owner ruling: a sideband attaches only
/// to a call its own caller's SDP offer created): a call another caller's offer placed is
/// refused, however well-formed its id, and so is a call no offer through this door created; the
/// caller whose offer created it attaches. RED: any well-formed id was dialled, so one caller could
/// take control of another's call on the operator's account. Both ways.
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

/// RED-BEFORE-GREEN (MONEY: one session fee per call): the SDP offer whose call busbar placed
/// reports the call's fee unit, and the sideband that joins that call reports its tokens and no second fee.
/// RED: the joined session reported its own `per_session` once its far end answered, so one call
/// was charged the fee twice. Both ways.
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
        assert!(
            b.last_units.contains("0:1=10") && !b.last_units.contains("8:1="),
            "the joined call's tokens, and no second fee: {}",
            b.last_units
        );
    }
}

/// RED-BEFORE-GREEN (MONEY; THE DESIGN: the plane reports the units it did, and the session fee is
/// charged once per opened session): a browser session whose call busbar placed on the operator's
/// account reports its fee unit with the provider's success, as a socket session does once its far
/// end answers; a call the provider refused placed nothing, reports none, and ends the session. The
/// ticket busbar mints places no call and reports nothing (row 132). RED: the SDP door reported no
/// unit at all, so a caller ran realtime sessions on the operator's key with a budget that never
/// moved. Both ways.
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

// ── THE ROUTE A DOOR NAMES (ARCHITECT Q-L5B-ROUTE) ───────────────────────────────────────────────

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
    use busbar_contract::abi::plane::{ROUTE_COUNTED, ROUTE_DIRECT, ROUTE_LOCAL, ROUTE_POOL};
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
            (Outcome::Ready, ROUTE_LOCAL, String::new()),
            "the mint door dials nothing"
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
    let bare = linked(&d);
    assert_eq!(open(&bare, 1, b"{}", Some(PUBLIC)).0, Outcome::Ready);
    assert_eq!(
        arrive_route(&bare, 3),
        (Outcome::Ready, ROUTE_POOL, String::new()),
        "no model, no route named"
    );
}

// ── THE SESSION OPEN'S HOOK VIEW (ARCHITECT Q-L5B-PROJECT; K5's `project`) ───────────────────────

/// `project` of unit 31 on `claim` with `rewrite` (empty = none): its outcome, the projected body
/// and whether it was answered as rewritten.
fn project(p: &Plugin<Plane>, claim: u32, rewrite: &'static [u8]) -> (Outcome, String, bool) {
    use busbar_contract::abi::hook::{MessageView, SignalEntry};
    use busbar_contract::abi::plane::{ProjectIn, ProjectOut, SPAN_ABSENT};
    let mut arena = vec![0u8; 4096];
    let mut signals: Vec<SignalEntry> = vec![z(); 4];
    let mut turns: Vec<MessageView> = vec![z(); 4];
    let mut j: Frame<ProjectIn, ProjectOut> = Frame::new(z(), z());
    (j.input.head, j.out.head) = (in_head(), out_head());
    j.input.claim = claim;
    j.input.unit = 31;
    j.input.arena_buf = arena.as_mut_ptr();
    j.input.arena_cap = arena.len();
    j.input.signals_buf = signals.as_mut_ptr();
    j.input.signals_cap = signals.len();
    j.input.messages_buf = turns.as_mut_ptr();
    j.input.messages_cap = turns.len();
    if !rewrite.is_empty() {
        j.input.rewrite = json(rewrite);
    }
    let c = p.call(slot::PROJECT, &mut j);
    let body = if j.out.body.offset == SPAN_ABSENT {
        String::new()
    } else {
        at(&arena, j.out.body)
    };
    (c.outcome, body, j.out.rewritten.offset != SPAN_ABSENT)
}

/// A session door projects the session-open params it locks as the view's body; a hook's rewrite is
/// patched over them and THAT is answered (kept for the session the unit opens); a rewrite that does
/// not read back refuses; a one-request door projects no body and takes no rewrite. Both ways.
#[test]
fn a_session_open_is_projected_for_the_hooks_and_a_rewrite_patches_it() {
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let (outcome, body, rewritten) = project(p, 2, b"");
        assert_eq!(outcome, Outcome::Ready);
        let view: serde_json::Value = serde_json::from_str(&body).expect("the locked params");
        assert_eq!(view["model"], "m-cap", "{body}");
        assert!(!rewritten);
        let (outcome, body, rewritten) = project(p, 2, br#"{"voice":"marin"}"#);
        assert_eq!(outcome, Outcome::Ready);
        let view: serde_json::Value = serde_json::from_str(&body).expect("the patched params");
        assert_eq!(view["voice"], "marin", "the hook's key wins: {body}");
        assert_eq!(
            view["model"], "m-cap",
            "the unnamed key keeps its value: {body}"
        );
        assert!(rewritten, "answered as rewritten");
        assert_eq!(project(p, 2, br#"{"voice":7}"#).0, Outcome::Refused);
        assert_eq!(project(p, 0, b""), (Outcome::Ready, String::new(), false));
        assert_eq!(project(p, 0, br#"{"x":1}"#).0, Outcome::Refused);
    }
}

// ── THE NEED A FAR REQUEST RIDES (ARCHITECT Q-L5B-NEEDS) ─────────────────────────────────────────

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
///
/// RED ARM: the kernel's word rendered as the message (the door before this rule) is not the
/// vendor's answer.
#[test]
fn an_authentication_refusal_reads_in_the_dialects_vendor_terms() {
    use busbar_contract::caps::ReasonCode;
    let vendor = serde_json::json!({"error": {
        "message": AUTH_FAILURE_MESSAGE,
        "type": "authentication_error",
        "param": null,
        "code": AUTH_FAILURE_CODE,
    }});
    let kernel_word: serde_json::Value =
        serde_json::from_slice(&refusal_body(401, "unauthenticated")).expect("JSON");
    assert_ne!(
        kernel_word, vendor,
        "RED ARM: the kernel's word is not the vendor's answer"
    );

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

/// RED-BEFORE-GREEN (THE DESIGN section 7: "Where the protocol carries a place to say so, a cut ends
/// the stream WITH AN ERROR FRAME naming budget exhaustion"; Part 3 section 12: "the plane renders
/// the error frame through `refusal`"): the kernel's budget cut of a live OpenAI Realtime session,
/// or of one its cut just ended, is told in-stream as the dialect's `error` event naming the
/// budget; a Gemini Live session, whose dialect has no in-band error, is told nothing invented; a
/// refusal before any session opened keeps the REST envelope byte for byte. RED: every cut was the
/// REST envelope, which no Realtime client reads as an event. Both ways.
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
            refusal_body(429, "the budget is exhausted").as_slice(),
            "a refusal before the upgrade keeps the REST envelope"
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

/// A CANCEL SAYS HOW FAR THE UNIT GOT (owner billing rule R21: the ledger records what the dialect
/// did): cut before the far end answered it is ABORTED (billed nothing); cut after the far end began
/// its answer it is PARTIAL, so the units it reported bill when its answer was streamed, as the llm
/// and decisions planes answer; both doors alike. The mint unit dials nothing (row 132), so its
/// cancel is always ABORTED; a session whose far end answered is PARTIAL. RED before: every cancel
/// was FAILED.
#[test]
fn a_cancel_after_the_far_end_answered_is_partial() {
    let d = host();
    for door in [linked as fn(&Dispatcher) -> Plugin<Plane>, dropped] {
        let p = door(&d);
        assert_eq!(open(&p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::new(256);
        b.call(&p, &push(7, 0, FROM_CALLER, PIECE_LAST, &[]));
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

/// RED-BEFORE-GREEN (review rounds 4 and 5, B1): an instance with no session ceiling and no row
/// asks for no tick, so the kernel ticks it again only when its driver ticket is woken. Recording
/// a brokered call wakes that ticket: the host OBSERVES the wake (the door is ticked on a ticket
/// the host has since retired, so the wake the record names lands as a late wake the host counts),
/// and nothing ticks the door by hand in between. The tick that follows starts the row's age and
/// asks for the tick it expires at; a tick past the lifetime expires the row, a sideband to the
/// call is refused, and the door asks for no further tick. RED (at fb7f530): the record woke
/// nothing, so an idle SDP-only instance was never ticked again and its rows never expired; RED
/// (at d7aa995): `tick` asked for no next tick at all. Both ways, through `on_piece` and `tick`.
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
        // The host drives the woken instance and polls its `tick`.
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

/// RED-BEFORE-GREEN (lead ruling: a sideband may reconnect to a live call; review round 4, B2): a
/// sideband the caller ends (its last piece) and one the host cancels by its unit both leave their
/// call's row, so the caller's next sideband to the same call attaches. RED (at 7027da0, the
/// sideband-close rule): closing the first sideband freed the call's row, and every later sideband
/// to the live call was refused. Both ways, through `on_piece` and `cancel`.
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

/// RED-BEFORE-GREEN (busbar row 159; ARCHITECT ruling 2026-10-23 on Part 2 #45, "OpenAI Realtime
/// over WS and WebRTC"): plain server-to-server OpenAI Realtime over WS is its own door, a WS
/// upgrade on `GET /v1/realtime` as the provider serves it (the caller's `?model=` on its query),
/// and the sideband is call-attached only. A session on the plain door opens with no call brokered
/// and dials the provider's realtime socket bare; the same caller's sideband naming a call no
/// offer here placed is still refused. RED: no `GET /v1/realtime` claim was published, so its
/// arrival was refused and its session never opened. Both ways.
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

/// The settings the host hands a plane that consumes sections (spec Part 1: one object keyed
/// by section; architect ruling 2026-10-23 R-6): the plane's own `streams:` beside the `models`
/// map, whose entry for the session model names the provider's wire model.
const KEYED_UPSTREAM: &[u8] = br#"{"streams":{"session":{"model":"m-cap"}},"models":{"m-cap":{"provider":"p","upstream_model":"gpt-realtime-wire"}}}"#;
/// The same, the model entry naming no override.
const KEYED_PLAIN: &[u8] =
    br#"{"streams":{"session":{"model":"m-cap"}},"models":{"m-cap":{"provider":"p"}}}"#;

/// RED-BEFORE-GREEN (busbar row 159; ARCHITECT rulings 2026-10-23 on Part 2 #51 and R-6): the
/// provider dial names the session model's RESOLVED upstream model, the catalog entry's
/// `upstream_model` when it sets one, else the catalog name. The plane reads it from the `models`
/// section the host hands it keyed beside `streams:`. RED: the door refused the keyed settings
/// (an unknown `streams` key at the section's top level), so no session opened. Both ways.
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
                    ..push(60, PLAIN_CLAIM, FROM_CALLER, 0, &uplink(100))
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

/// RED-BEFORE-GREEN (busbar row 159; ARCHITECT ruling 2026-10-23 on Part 2 #51): EVERY
/// upstream-facing model field carries the resolved upstream model, never the catalog alias: the
/// locked `session.model` of busbar's own call for an SDP door's session (and of the ticket the
/// mint locks it with), the locked `model` a caller's `session.update` is spliced
/// with on its way to the provider, and the `model` of a Gemini Live caller's `setup`. RED: each
/// named the catalog name (`m-cap`), which reaches the provider as an unknown model. Both ways.
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

/// THE SDP DOOR'S OPEN (row 132): the 201 and busbar's call in `Location`, first; then busbar's own
/// call to the provider, one form carrying the session the ticket locked and naming the caller for
/// the provider's attribution; the provider's answer names its call, which the sideband door
/// resolves busbar's call id to, for the offer's own caller. RED: a call busbar does not know is no
/// door's. Both ways.
#[test]
fn the_sdp_door_places_busbars_call_locked_to_its_ticket_and_the_sideband_resolves_it() {
    let d = host();
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::with_units(1 << 16, 8);
        // The open: the association's empty piece.
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
                 fields=[\"location=/v1/realtime/calls/{id}\"]"
            )
        );
        // The collection: busbar's own call, owed the far end before anything else.
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
        // The form is the body of the composed open busbar dials (the description of the upstream
        // leg completes it), not a text message on a socket: a text message rides no opening.
        assert_eq!(
            b.last_flags & PIECE_OUT_TEXT,
            0,
            "busbar's call form is the open's body, never a text message: {call}"
        );
        // The provider's answer to busbar's call names its own call.
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
        // The sideband door resolves busbar's call id to the provider's.
        let sideband = sideband_to(&id);
        assert_eq!(arrive_at(p, 2, 60, sideband), Outcome::Ready);
        let control = b.call(p, &session(60, FROM_CALLER, 0, &uplink(100)));
        assert!(
            control.contains("target=/v1/realtime?call_id=rtc_upstream0001 "),
            "{control}"
        );
        // Another caller naming the same call is refused.
        assert_eq!(arrive_at(p, 2, 62, sideband), Outcome::Ready);
        b.caller = "beef";
        let foreign = b.call(p, &session(62, FROM_CALLER, 0, &uplink(100)));
        assert!(foreign.starts_with("Refused"), "{foreign}");
        // RED: a call busbar does not know is no door's.
        assert_eq!(
            arrive_at(p, 2, 61, "/v1/realtime/sideband/rtc_unknown"),
            Outcome::Refused
        );
        // The session's end frees busbar's call: a sideband to it is no door's.
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

/// A KEY OUTSIDE THE SESSION MODEL'S GRANT GETS NO TICKET (R10 grant ruling): the mint door asks
/// `entitlement.check` for `pool:<the session's model>` before it mints, and a key the host does not
/// entitle is refused 403 in the OpenAI error envelope: nothing minted, no record stored, nothing
/// dialled, no units. Both ways.
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
        // THE REFUSAL IS AUDITED WITH ITS REASON (R10 grant ruling 2): the unit's audit row names
        // the pool the key is not granted, never the key.
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

/// AN SDP OFFER THAT ARRIVES AS A PLAIN REQUEST (coordinator ruling 3, R10): the SDP door opens a
/// call session over the WebRTC carrier; an offer reaching it as a one-request unit (no session)
/// is refused on its first piece with 501 Not Implemented (RFC 9110 section 15.6.2) and a body that says
/// why, never an empty answer the host reads as the plane's panic. The refusal is audited, nothing
/// is dialled and no units are reported. RED: the door answered the piece Refused, with nothing.
/// Both ways, on the caller's piece and on an ATTEMPT piece alike.
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

/// RED-BEFORE-GREEN (THE DESIGN: admission bounds live work, nothing evicts it): an instance holding
/// as many live calls as it admits refuses the next SDP offer at its arrival, before any ticket is
/// taken or call placed: a 429 worded by the door, and no other caller's call is dropped for it.
/// RED: the offer was taken and placed, and the call it displaced was another caller's. Both ways.
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
        assert_eq!(
            arrive_sideband(p, 901, sideband_to(&first)),
            Outcome::Ready,
            "the oldest caller's call is kept"
        );
    }
}

/// THE UNIT TABLE AT ITS BOUND (sibling rule R3: a unit table full is refused at `arrive`, 429, in
/// the plane's words): an instance holding as many request units as it keeps refuses the next mint
/// at its arrival, before any piece, with the plane's code, a 429 and the words the SDP bound
/// wears; no unit in flight is dropped for it, and once one ends the next arrival is admitted.
/// RED: the arrival was admitted and its first piece answered a bare REFUSED, no words, no status.
/// Both ways.
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

// ── AN UNREADABLE USAGE, AND THE HOST LOG ─────────────────────────────────────────────────────────

/// The host's envelope sink, keeping the text of every diagnostic and log record a call carried:
/// what the host writes to the plugin's own log file.
#[derive(Default)]
struct HostLog(std::sync::Mutex<Vec<String>>);

impl busbar_plugin_loader::dispatch::EnvelopeSink for HostLog {
    fn metric(&self, _: busbar_plugin_loader::dispatch::Metric<'_>) {}
    fn diag(&self, d: busbar_plugin_loader::dispatch::Diagnostic<'_>) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(String::from_utf8_lossy(d.text).into_owned());
    }
    fn dropped(&self, _: busbar_plugin_loader::dispatch::Dropped) {}
}

/// A far end's usage report whose input count is a string, not a count.
const USAGE_UNREADABLE: &[u8] =
    br#"{"type":"response.done","response":{"usage":{"input_tokens":"1500","output_tokens":9}}}"#;

/// RED (owner ruling 2026-10-27: spend is always attributed and nothing is silent): a usage report
/// the reader cannot read bills the llm plane's floor (output tokens, one per four bytes of the
/// report, at least one; `UNITS_FLOOR`), the session goes on, and one line saying so reaches the
/// HOST's log from the DROPPED-IN door as from the linked one (the door's call capture). RED
/// before: the session ended unmetered and billed nothing for the turn.
#[test]
fn an_unreadable_usage_bills_the_floor_and_its_line_reaches_the_host_log() {
    // The sideband attaches to a call busbar placed: its host serves tickets and randomness.
    let d = host();
    let floor = (USAGE_UNREADABLE.len() as u64 / 4).max(1);
    let stated = row().statement;
    let path = {
        let exe = std::env::current_exe().expect("the test binary has a path");
        let deps = exe.parent().expect("target/<profile>/deps").to_path_buf();
        let profile = deps.parent().expect("target/<profile>").to_path_buf();
        let file = busbar_plugin_loader::plugin_library_filename("busbar_plane_streaming_plugin");
        [profile.join(&file), deps.join(&file)]
            .into_iter()
            .find(|p| p.exists())
            .unwrap_or_else(|| panic!("this crate's cdylib ({file}) is not built"))
    };
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
            attach_sideband(&p, 70, "/v1/realtime/calls/rtc_both0070"),
            Outcome::Ready
        );
        let mut b = Bufs::with_units(1 << 18, 8);
        b.call(&p, &session(70, FROM_CALLER, 0, &uplink(100)));
        b.call(
            &p,
            &Push {
                attempt_no: 1,
                ..session(70, FROM_KERNEL, 0, &[])
            },
        );
        let far = Push {
            status: 101,
            ..session(70, FROM_FAR_END, PIECE_HAS_STATUS, USAGE_UNREADABLE)
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
    }
}

/// RED (audit plane-streaming round 52b0f93ac9, S1, MONEY): a far end that REFUSES the session's
/// upgrade (a 401 for a bad operator key, a 429, a 5xx) never answered the session: no fee unit is
/// reported (the kernel refunds it) and a cancel is ABORTED. One that accepts it (101) incurs the
/// fee, and a cancel is PARTIAL. On the plain realtime socket (claim 5) and the Gemini Live socket
/// (claim 3), both ways. RED: every far-end piece incurred the fee, whatever its status.
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
            let last = if fee { 0 } else { PIECE_LAST };
            let body = if fee { &[][..] } else { REFUSAL };
            let answered = b.call(
                &p,
                &Push {
                    status,
                    ..piece(FROM_FAR_END, PIECE_HAS_STATUS | last, body)
                },
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
