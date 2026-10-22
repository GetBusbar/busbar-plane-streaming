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
    PlaneRefreshOut, PlaneSnapshot, UnitCount, CANCEL_FAILED, CLAIM_EXACT, CLAIM_OPEN, EMIT_DONE,
    EMIT_TO_FAR_END, FROM_CALLER, FROM_FAR_END, FROM_KERNEL, PIECE_HAS_STATUS, PIECE_LAST,
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
    // SAFETY: `busbar_plane_streaming::door` is this crate's own door: it answers the `'static` door
    // of this plugin.
    #[allow(unused_unsafe)]
    let loaded = unsafe { LoadedDoor::from_raw(door) };
    loaded
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
        &*(*(*door()).statement).kind_tail().cast::<PlaneTail>()
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
/// `door_tests` pins the door's own constants to these words).
const AUTH_FAILURE_MESSAGE: &str = "Incorrect API key provided.";
const AUTH_FAILURE_CODE: &str = "invalid_api_key";

/// The realtime dialects' REST error envelope for a refusal at `status` worded `text`, built as the
/// door builds it (the logic crate's `door_tests` reads the door's `refusal_body` in this shape).
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

/// The served plane's answer to a minted secret, built as the plane builds it (the logic crate's
/// `driven_tests` pins `mint_reply`'s body to this shape).
fn minted_answer() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "value": "ek_oracle_0001",
        "expires_at_unix": 1_767_225_600,
    }))
    .expect("the answer serializes")
}

/// THE STATEMENT CARRIES THE DECLARES DOCUMENT (BUSBAR-1.6.0.md l.246, ARCHITECT ruling
/// 2026-10-21): the door's Statement, read back, holds `declares.json` byte for byte under
/// `EXT_DECLARES`.
#[test]
fn the_statement_carries_the_declares_document() {
    let row = LinkedRow::of_loaded(loaded_door()).unwrap();
    let read = busbar_contract::abi::mechanism::rendering::read(&row.statement).unwrap();
    assert_eq!(
        busbar_contract::abi::mechanism::extensions::declares(&read.extensions.bytes),
        Some(&include_bytes!("../../plane-streaming/declares.json")[..])
    );
}

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
                "{} {} {}{}{}",
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

    for claim in [3, 5, 9] {
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
        "cancel {:?} failed={}",
        c.outcome,
        x.out.cancel.disposition == CANCEL_FAILED
    ));
    t
}

const PUBLIC: &str = "https://gw.example.com/ignored?q=1#f";

fn expected(public: bool) -> Vec<String> {
    let doors = "POST /v1/realtime/client_secrets http exact, POST /v1/realtime/calls http exact, \
                 GET /v1/realtime/sideband/{call_id} ws, GET /v1/realtime/gemini/{call_id} ws, \
                 GET /twilio/{call_id} ws";
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
        "arrive 5 Refused dialect=0 op=0 principal=0".into(),
        "arrive 9 Refused dialect=0 op=0 principal=0".into(),
        "tick Ready next=0".into(),
        "cancel Ready failed=true".into(),
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
/// The body the served plane sent its far end for that mint (the recorded
/// `streams|mint|client-secrets` cell, `golden/1.6.0-pre`).
/// The session goes in the GA shape (`type: realtime`) under the section's two token ceilings (the
/// defaults, 4096 and 32768).
const MINT_BODY: &str = r#"{"expires_after":{"anchor":"created_at","seconds":600},"session":{"max_output_tokens":4096,"model":"m-cap","truncation":{"retention_ratio":1.0,"token_limits":{"post_instructions":28672},"type":"retention_ratio"},"type":"realtime"}}"#;
/// The far end's mint answer.
const MINTED: &[u8] = br#"{"value":"ek_oracle_0001","expires_at":1767225600}"#;
/// The caller's SDP offer, and the far end's answer and where its call lives.
const OFFER: &[u8] = b"v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=oracle-offer\r\nt=0 0\r\n";
const SDP_ANSWER: &[u8] = b"v=0\r\no=- 2 2 IN IP4 127.0.0.1\r\ns=oracle-answer\r\nt=0 0\r\n";
const LOCATION: &str = "/v1/realtime/calls/rtc_oracle0001";

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
        let mut o: OnPieceOut = z();
        o.head = out_head();
        let mut f = Frame::new(i, o);
        let c = p.call(slot::ON_PIECE, &mut f);
        let o = f.out;
        self.last_need = o.need;
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

/// THE UNIT SCRIPT: a mint, an SDP offer, a session door's piece and an unpublished claim's (the
/// metadata path among them: it is no claim, 1.5.5 parity), in the order the route pump pushes them.
fn unit_script(p: &Plugin<Plane>) -> Vec<String> {
    let mut t = vec![format!("open {:?}", open(p, 1, SESSION, Some(PUBLIC)).0)];
    let mut b = Bufs::new(256);
    let whole = PIECE_HAS_STATUS | PIECE_LAST;

    t.push(format!("mint attempt {}", b.call(p, &attempt(7, 0))));
    let caller = push(7, 0, FROM_CALLER, PIECE_LAST, &[]);
    t.push(format!("mint caller {}", b.call(p, &caller)));
    let far = Push {
        status: 200,
        ..push(7, 0, FROM_FAR_END, whole, MINTED)
    };
    t.push(format!("mint far_end {}", b.call(p, &far)));

    t.push(format!("sdp attempt {}", b.call(p, &attempt(8, 1))));
    let offer = push(8, 1, FROM_CALLER, PIECE_LAST, OFFER);
    t.push(format!("sdp caller {}", b.call(p, &offer)));
    let location = [Field {
        name: text("location"),
        value: text(LOCATION),
    }];
    let far = Push {
        status: 201,
        head: &location,
        ..push(8, 1, FROM_FAR_END, whole, SDP_ANSWER)
    };
    t.push(format!("sdp far_end {}", b.call(p, &far)));

    t.push(format!("sideband {}", b.call(p, &attempt(10, 2))));
    t.push(format!("unpublished {}", b.call(p, &attempt(11, 9))));
    t.push(format!("metadata path {}", b.call(p, &attempt(12, 5))));
    t
}

/// What the unit script must read: the served plane's answers, through the door.
fn unit_expected() -> Vec<String> {
    let none = "verb= target= fields=[]";
    let minted = minted_answer();
    let mut t = vec![
        "open Ready".to_string(),
        format!(
            "mint attempt Ready emitted={MINT_BODY} more=0 far=true done=false status=0 \
             verb=POST target=/v1/realtime/client_secrets \
             fields=[\"content-type=application/json\", \"OpenAI-Safety-Identifier={CALLER_REF}\"]"
        ),
        format!("mint caller Ready emitted= more=0 far=false done=false status=0 {none}"),
        format!(
            "mint far_end Ready emitted={} more=0 far=false done=true status=200 verb= target= \
             fields=[\"content-type=application/json\", \"content-length={}\"]",
            String::from_utf8_lossy(&minted),
            minted.len()
        ),
        "sdp attempt Ready emitted= more=0 far=true done=false status=0 verb=POST \
         target=/v1/realtime/calls fields=[\"content-type=application/sdp\"]"
            .to_string(),
        format!(
            "sdp caller Ready emitted={} more=0 far=true done=false status=0 {none}",
            String::from_utf8_lossy(OFFER)
        ),
        format!(
            "sdp far_end Ready emitted={} more=0 far=false done=true status=201 verb= target= \
             fields=[\"content-type=application/sdp\", \"location={LOCATION}\", \
             \"content-length={}\"]",
            String::from_utf8_lossy(SDP_ANSWER),
            SDP_ANSWER.len()
        ),
    ];
    t.push(format!(
        "sideband Refused emitted= more=0 far=false done=false status=0 {none}"
    ));
    t.push(format!(
        "unpublished Refused emitted= more=0 far=false done=false status=0 {none}"
    ));
    t.push(format!(
        "metadata path Refused emitted= more=0 far=false done=false status=0 {none}"
    ));
    t
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

/// The SDP offer of `unit` by `caller`, whose far end answers 201 with its call at `location`: the
/// call that caller's sideband may attach to. The far end's answer, as text.
fn broker(p: &Plugin<Plane>, unit: u64, location: &'static str, caller: &'static str) -> String {
    let mut b = Bufs::new(256);
    b.caller = caller;
    let _ = b.call(p, &attempt(unit, 1));
    let _ = b.call(p, &push(unit, 1, FROM_CALLER, PIECE_LAST, OFFER));
    let head = [Field {
        name: text("location"),
        value: text(location),
    }];
    let far = Push {
        status: 201,
        head: &head,
        ..push(
            unit,
            1,
            FROM_FAR_END,
            PIECE_HAS_STATUS | PIECE_LAST,
            SDP_ANSWER,
        )
    };
    b.call(p, &far)
}

/// The SDP offer the test caller makes for call `location` (on a unit of its own), then `arrive`
/// of `unit` on the sideband door at `target` (`/v1/realtime/sideband/{call_id}`): the provider
/// call the unit's session attaches to.
fn attach_sideband(
    p: &Plugin<Plane>,
    unit: u64,
    location: &'static str,
    target: &'static str,
) -> Outcome {
    let brokered = broker(p, unit + 100_000, location, CALLER_REF);
    assert!(brokered.contains("status=201"), "{brokered}");
    arrive_sideband(p, unit, target)
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
        attach_sideband(
            p,
            20,
            "/v1/realtime/calls/rtc_both0020",
            "/v1/realtime/sideband/rtc_both0020"
        )
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
    let d = Dispatcher::new(DispatchConfig::default());
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
/// the sideband door dials the call its arrival's `{call_id}` names, and a session whose arrival
/// named no call is refused, never dialled as a fresh session. RED: the `{call_id}` was never read
/// and every sideband dialled `/v1/realtime`, an unrelated session. Both ways.
#[test]
fn a_sideband_session_attaches_to_the_call_its_arrival_names() {
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::with_units(1 << 18, 8);
        let unnamed = b.call(p, &session(60, FROM_CALLER, 0, &uplink(100)));
        assert!(unnamed.starts_with("Refused"), "{unnamed}");
        assert_eq!(
            attach_sideband(
                p,
                61,
                "/v1/realtime/calls/rtc_both0061",
                "/v1/realtime/sideband/rtc_both0061"
            ),
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
            Outcome::Ready,
            "an id that is not the provider's shape names no call"
        );
        let forged = b.call(p, &session(62, FROM_CALLER, 0, &uplink(100)));
        assert!(forged.starts_with("Refused"), "{forged}");
    }
}

/// RED-BEFORE-GREEN (THE DESIGN #45's browser trust boundary; owner ruling: a sideband attaches only
/// to a call its own caller's SDP offer created): a call another caller's offer brokered is
/// refused, however well-formed its id, and so is a call no offer through this door created; the
/// caller whose offer created it attaches. RED: any well-formed id was dialled, so one caller could
/// take control of another's call on the operator's account. Both ways.
#[test]
fn a_sideband_attaches_only_to_its_own_callers_call() {
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let theirs = broker(p, 90, "/v1/realtime/calls/rtc_theirs0001", "beef");
        assert!(theirs.contains("status=201"), "{theirs}");
        let mut b = Bufs::with_units(1 << 18, 8);
        assert_eq!(
            arrive_sideband(p, 91, "/v1/realtime/sideband/rtc_theirs0001"),
            Outcome::Ready
        );
        let foreign = b.call(p, &session(91, FROM_CALLER, 0, &uplink(100)));
        assert!(
            foreign.starts_with("Refused"),
            "another caller's call: {foreign}"
        );
        assert_eq!(
            arrive_sideband(p, 92, "/v1/realtime/sideband/rtc_unbrokered01"),
            Outcome::Ready
        );
        let unknown = b.call(p, &session(92, FROM_CALLER, 0, &uplink(100)));
        assert!(
            unknown.starts_with("Refused"),
            "a call no offer here created: {unknown}"
        );
        b.caller = "beef";
        assert_eq!(
            arrive_sideband(p, 93, "/v1/realtime/sideband/rtc_theirs0001"),
            Outcome::Ready
        );
        let own = b.call(p, &session(93, FROM_CALLER, 0, &uplink(100)));
        assert!(
            own.contains("far=true")
                && own.contains("verb=GET target=/v1/realtime?call_id=rtc_theirs0001 "),
            "the offer's own caller attaches: {own}"
        );
    }
}

/// RED-BEFORE-GREEN (MONEY: one session fee per call): the SDP offer that brokers a call reports
/// the call's fee unit, and the sideband that joins that call reports its tokens and no second fee.
/// RED: the joined session reported its own `per_session` once its far end answered, so one call
/// was charged the fee twice. Both ways.
#[test]
fn a_sideband_joining_a_brokered_call_adds_no_second_fee() {
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let brokered = broker(p, 94, "/v1/realtime/calls/rtc_fee0001", CALLER_REF);
        assert!(brokered.contains("status=201"), "{brokered}");
        assert_eq!(
            arrive_sideband(p, 95, "/v1/realtime/sideband/rtc_fee0001"),
            Outcome::Ready
        );
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
/// charged once per opened session): a browser session the mint or the SDP offer brokers on the
/// operator's account reports its fee unit with the far end's answer, as a socket session does once
/// its far end answers; a pass the far end refused brokered nothing and reports none. A failed SDP
/// offer's error body keeps the far end's own type. RED: both doors reported no unit at all, so a
/// caller ran realtime sessions on the operator's key with a budget that never moved. Both ways.
#[test]
fn a_brokered_browser_session_reports_its_session_fee() {
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    let whole = PIECE_HAS_STATUS | PIECE_LAST;
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::new(256);

        let _ = b.call(p, &attempt(70, 0));
        let _ = b.call(p, &push(70, 0, FROM_CALLER, PIECE_LAST, &[]));
        let far = Push {
            status: 200,
            ..push(70, 0, FROM_FAR_END, whole, MINTED)
        };
        let minted = b.call(p, &far);
        assert!(minted.contains("done=true status=200"), "{minted}");
        assert_eq!(b.last_units, "8:1=1", "the minted session's fee: {minted}");

        let _ = b.call(p, &attempt(71, 1));
        let _ = b.call(p, &push(71, 1, FROM_CALLER, PIECE_LAST, OFFER));
        let far = Push {
            status: 201,
            ..push(71, 1, FROM_FAR_END, whole, SDP_ANSWER)
        };
        let answered = b.call(p, &far);
        assert!(answered.contains("done=true status=201"), "{answered}");
        assert_eq!(
            b.last_units, "8:1=1",
            "the offered session's fee: {answered}"
        );

        let _ = b.call(p, &attempt(72, 0));
        let _ = b.call(p, &push(72, 0, FROM_CALLER, PIECE_LAST, &[]));
        let far = Push {
            status: 500,
            ..push(72, 0, FROM_FAR_END, whole, b"{}")
        };
        let failed = b.call(p, &far);
        assert!(failed.contains("done=true status=502"), "{failed}");
        assert_eq!(b.last_units, "", "a failed mint brokered nothing: {failed}");

        let _ = b.call(p, &attempt(73, 1));
        let _ = b.call(p, &push(73, 1, FROM_CALLER, PIECE_LAST, OFFER));
        let json_type = [Field {
            name: text("content-type"),
            value: text("application/json"),
        }];
        let far = Push {
            status: 400,
            head: &json_type,
            ..push(
                73,
                1,
                FROM_FAR_END,
                whole,
                br#"{"error":{"message":"bad offer"}}"#,
            )
        };
        let refused = b.call(p, &far);
        assert!(
            refused.contains("done=true status=400")
                && refused.contains("\"content-type=application/json\""),
            "the far end's error keeps its own type: {refused}"
        );
        assert_eq!(
            b.last_units, "",
            "a refused offer brokered nothing: {refused}"
        );
    }
}

#[test]
fn the_dropped_door_answers_a_live_session_as_the_linked_door_does() {
    let d = Dispatcher::new(DispatchConfig::default());
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

/// Every door that reaches the far end names the DIRECT route `streams.session.model` (the entry the
/// composition folds from the top-level models catalog); claim 5 (the metadata path) is no claim
/// and arrives nowhere; with no model configured nothing is named (the door steps refuse an unnamed keyed route).
/// Both ways.
#[test]
fn a_door_that_reaches_the_far_end_names_the_session_model_as_its_direct_route() {
    use busbar_contract::abi::plane::{ROUTE_DIRECT, ROUTE_POOL};
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        for claim in [0, 1, 2, 3, 4] {
            assert_eq!(
                arrive_route(p, claim),
                (Outcome::Ready, ROUTE_DIRECT, "m-cap".to_string()),
                "claim {claim}"
            );
        }
        assert_ne!(
            arrive_route(p, 5).0,
            Outcome::Ready,
            "the metadata path is no claim of the plane (1.5.5 parity)"
        );
    }
    let bare = linked(&d);
    assert_eq!(open(&bare, 1, b"{}", Some(PUBLIC)).0, Outcome::Ready);
    assert_eq!(
        arrive_route(&bare, 2),
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
/// socket, the mint's request Realtime's one-shot pass (each a declared outbound need under the
/// dialect's style, held by the door's own compile-time check). Both ways.
#[test]
fn every_far_request_names_the_need_it_rides() {
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        assert_eq!(
            attach_sideband(
                p,
                40,
                "/v1/realtime/calls/rtc_both0040",
                "/v1/realtime/sideband/rtc_both0040"
            ),
            Outcome::Ready
        );
        let mut b = Bufs::with_units(1 << 18, 8);
        let answer = b.call(p, &session(40, FROM_CALLER, 0, &uplink(100)));
        assert!(answer.contains("far=true"), "{answer}");
        assert_eq!(b.last_need, rides("ws", "bearer"), "{answer}");
        let mut b = Bufs::new(1 << 16);
        let answer = b.call(p, &attempt(41, 0));
        assert!(answer.contains("far=true"), "{answer}");
        assert_eq!(b.last_need, rides("http", "bearer"), "{answer}");
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
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::new(1 << 16);
        let audio = uplink(100);
        for (unit, location, target) in [
            (
                130,
                "/v1/realtime/calls/rtc_both0130",
                "/v1/realtime/sideband/rtc_both0130",
            ),
            (
                132,
                "/v1/realtime/calls/rtc_both0132",
                "/v1/realtime/sideband/rtc_both0132",
            ),
        ] {
            assert_eq!(attach_sideband(p, unit, location, target), Outcome::Ready);
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

/// One `tick` at `now_ns`: its outcome and the next tick it asks for (`0` = none).
fn tick(p: &Plugin<Plane>, now_ns: u64) -> (Outcome, u64) {
    tick_on(
        p,
        busbar_contract::abi::mechanism::ticket::Ticket::NONE,
        now_ns,
    )
}

/// One `tick` at `now_ns` on `driver`, the ticket the door names when it has work of its own.
fn tick_on(
    p: &Plugin<Plane>,
    driver: busbar_contract::abi::mechanism::ticket::Ticket,
    now_ns: u64,
) -> (Outcome, u64) {
    let mut k: Frame<TickIn, TickOut> = Frame::new(z(), z());
    k.input.head = in_head();
    k.input.head.ticket = driver;
    k.input.now_ns = now_ns;
    k.out.head = out_head();
    let c = p.call(life::TICK, &mut k);
    (c.outcome, k.out.next_tick_ns)
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
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let driver = d.mint(0).expect("a ticket on worker 0");
        assert_eq!(
            tick_on(p, driver, 5),
            (Outcome::Ready, 0),
            "nothing held, no ceiling: no tick is asked for"
        );
        d.recycle(driver);
        let before = d.stats().stale_wakes;
        let brokered = broker(p, 300, "/v1/realtime/calls/rtc_tick0300", CALLER_REF);
        assert!(brokered.contains("status=201"), "{brokered}");
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
        assert_eq!(
            arrive_sideband(p, 301, "/v1/realtime/sideband/rtc_tick0300"),
            Outcome::Ready
        );
        let live = b.call(p, &session(301, FROM_CALLER, 0, &uplink(100)));
        assert!(live.contains("far=true"), "inside its lifetime: {live}");
        assert_eq!(
            tick(p, next),
            (Outcome::Ready, 0),
            "the expired row is freed, and nothing is left to tick for"
        );
        assert_eq!(
            arrive_sideband(p, 302, "/v1/realtime/sideband/rtc_tick0300"),
            Outcome::Ready
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
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let brokered = broker(p, 310, "/v1/realtime/calls/rtc_back0310", CALLER_REF);
        assert!(brokered.contains("status=201"), "{brokered}");
        let target = "/v1/realtime/sideband/rtc_back0310";
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
