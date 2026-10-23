// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE STREAMING PLANE'S DOOR, BOTH WAYS (#2 (4)/(5): the plugin's own both-ways witness, over the
//! real loader): the same door compiled in (`busbar_plane_streaming::door::door`, through
//! `load_linked`) and dropped in (this crate's cdylib, `busbar_plane_streaming_plugin`, which `cargo
//! test` builds beside the test, through `load_dropped`), driven by ONE script over the plane kind.
//! It moved here with the crate from busbar's `crates/busbar-plane-streaming/tests/conformance.rs`;
//! the published suite is this crate's `tests/conformance.rs`. Every answer is judged
//! by the kind's own checks on the way; the two transcripts must be identical, and each is pinned.

use std::mem::zeroed;
use std::sync::Arc;

use busbar_contract::abi::mechanism::call::{
    AbiStr, Blob, Field, Outcome, Span, BLOB_JSON, BLOB_OCTETS,
};
use busbar_contract::abi::mechanism::lifecycle::{
    slot as life, GenIn, RefreshIn, TickIn, TickOut, ValidateIn,
};
use busbar_contract::abi::plane::{
    slot, ArriveIn, ArriveOut, OnPieceIn, OnPieceOut, OutField, PlaneOpenIn, PlaneOpenOut,
    PlaneRefreshOut, PlaneSnapshot, UnitCount, CANCEL_FAILED, CLAIM_EXACT, CLAIM_OPEN, EMIT_DONE,
    EMIT_TO_FAR_END, FROM_CALLER, FROM_FAR_END, FROM_KERNEL, PIECE_HAS_STATUS, PIECE_LAST,
};
use busbar_plane_streaming::{door, driven};
use busbar_plugin_loader::dispatch::kinds::plane::Plane;
use busbar_plugin_loader::dispatch::{
    in_head, load_dropped, load_linked, out_head, Bind, DispatchConfig, Dispatcher, Frame,
    LinkedRow, NoSink, Plugin,
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

/// The compiled-in row: the door and the Statement rendering it states.
fn row() -> LinkedRow {
    LinkedRow::of(door::door).expect("the streaming door states itself")
}

fn json(b: &'static [u8]) -> Blob {
    Blob {
        ptr: b.as_ptr(),
        len: b.len(),
        fmt: BLOB_JSON,
        flags: 0,
    }
}

fn text(s: &'static str) -> AbiStr {
    AbiStr {
        ptr: s.as_ptr(),
        len: s.len(),
    }
}

fn read(s: AbiStr) -> String {
    if s.ptr.is_null() {
        return String::new();
    }
    // SAFETY: the plugin's text, valid while its generation is live.
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(s.ptr, s.len) }).into_owned()
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
    let t = door::TAIL;
    assert_eq!(read(t.scope), "session");
    assert_eq!(read(t.audit_kind), "streaming_session");
    // SAFETY: the tail's `'static` lists.
    let classes: Vec<String> =
        unsafe { std::slice::from_raw_parts(t.billable_classes, t.billable_classes_len) }
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
            "per_session/count",
        ]
    );
    // SAFETY: as above.
    let fees: Vec<String> = unsafe { std::slice::from_raw_parts(t.fee_units, t.fee_units_len) }
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
            std::slice::from_raw_parts(t.fee_units, t.fee_units_len),
            std::slice::from_raw_parts(t.billable_classes, t.billable_classes_len),
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
const MINT_BODY: &str =
    r#"{"expires_after":{"anchor":"created_at","seconds":600},"session":{"model":"m-cap"}}"#;
/// The far end's mint answer.
const MINTED: &[u8] = br#"{"value":"ek_oracle_0001","expires_at":1767225600}"#;
/// The caller's SDP offer, and the far end's answer and where its call lives.
const OFFER: &[u8] = b"v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=oracle-offer\r\nt=0 0\r\n";
const SDP_ANSWER: &[u8] = b"v=0\r\no=- 2 2 IN IP4 127.0.0.1\r\ns=oracle-answer\r\nt=0 0\r\n";
const LOCATION: &str = "/v1/realtime/calls/rtc_oracle0001";

fn octets(b: &[u8]) -> Blob {
    Blob {
        ptr: b.as_ptr(),
        len: b.len(),
        fmt: BLOB_OCTETS,
        flags: 0,
    }
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
    /// The kernel's reference for the piece's caller.
    caller: &'static str,
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
        }
    }

    /// `push` answered: the outcome and what it wrote, as text.
    fn call(&mut self, p: &Plugin<Plane>, push: &Push<'_>) -> String {
        let mut i: OnPieceIn = z();
        i.head = in_head();
        (i.unit, i.claim, i.from, i.flags) = (push.unit, push.claim, push.from, push.flags);
        i.stream = push.stream;
        (i.attempt_no, i.bytes, i.status_code) = (push.attempt_no, octets(push.bytes), push.status);
        i.caller_ref = text(push.caller);
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
        caller: CALLER_REF,
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
    t.push(format!("metadata path {}", b.call(p, &attempt(12, 6))));
    t
}

/// What the unit script must read: the served plane's answers, through the door.
fn unit_expected() -> Vec<String> {
    let none = "verb= target= fields=[]";
    let minted = driven::mint_reply(200, MINTED);
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
            String::from_utf8_lossy(&minted.body),
            minted.body.len()
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

/// Where the call the live session attaches to lives, as the SDP offer's far end answered it.
const LIVE_CALL_LOCATION: &str = "/v1/realtime/calls/rtc_both0020";
/// The sideband path that names that call.
const LIVE_SIDEBAND: &str = "/v1/realtime/sideband/rtc_both0020";

/// `arrive` of `unit` on `claim` at `target`: its outcome.
fn arrive_at(p: &Plugin<Plane>, claim: u32, unit: u64, target: &'static str) -> Outcome {
    let mut a: Frame<ArriveIn, ArriveOut> = Frame::new(z(), z());
    a.input.head = in_head();
    (a.input.unit, a.input.claim) = (unit, claim);
    (a.input.method, a.input.target) = (text("GET"), text(target));
    a.out.head = out_head();
    p.call(slot::ARRIVE, &mut a).outcome
}

/// The SDP door brokers `unit`'s offer for `caller`, its far end answering that the call lives at
/// `location`: the far end's answer to the caller, as text.
fn broker(
    p: &Plugin<Plane>,
    b: &mut Bufs,
    unit: u64,
    caller: &'static str,
    location: &'static str,
) -> String {
    let _ = b.call(
        p,
        &Push {
            caller,
            ..attempt(unit, 1)
        },
    );
    let _ = b.call(
        p,
        &Push {
            caller,
            ..push(unit, 1, FROM_CALLER, PIECE_LAST, OFFER)
        },
    );
    let head = [Field {
        name: text("location"),
        value: text(location),
    }];
    b.call(
        p,
        &Push {
            status: 201,
            head: &head,
            caller,
            ..push(
                unit,
                1,
                FROM_FAR_END,
                PIECE_HAS_STATUS | PIECE_LAST,
                SDP_ANSWER,
            )
        },
    )
}

/// `ms` milliseconds of 24 kHz PCM16 uplink audio, as the caller's realtime frame.
fn uplink(ms: usize) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"type":"input_audio_buffer.append",
        "audio": busbar_contract::media::base64_encode(&vec![0u8; ms * 48])}))
    .expect("json")
}

const USAGE_DONE: &[u8] = br#"{"type":"response.done","response":{"usage":{"input_token_details":{"audio_tokens":10,"text_tokens":3},"output_token_details":{"audio_tokens":20,"text_tokens":4}}}}"#;

/// THE SESSION SCRIPT, in the order the driver pushes a session's pieces: the SDP offer that
/// brokers the call and the sideband arrival that names it, the caller's audio (a turn toward the
/// far end), the turn's ATTEMPT, the far end's usage, a collection with nothing
/// owed, an answer short of unit room and its re-call, and the caller's end.
fn session_script(p: &Plugin<Plane>) -> Vec<String> {
    let mut t = vec![format!("open {:?}", open(p, 1, SESSION, Some(PUBLIC)).0)];
    let mut b = Bufs::with_units(1 << 18, 6);
    let line = |t: &mut Vec<String>, b: &mut Bufs, what: &str, push: &Push<'_>| {
        let answer = b.call(p, push);
        t.push(format!("{what} {answer} units=[{}]", b.last_units));
    };
    t.push(format!(
        "broker {}",
        broker(p, &mut b, 19, CALLER_REF, LIVE_CALL_LOCATION)
    ));
    t.push(format!("arrive {:?}", arrive_at(p, 2, 20, LIVE_SIDEBAND)));
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
    assert!(
        line("broker").contains("done=true status=201"),
        "{}",
        line("broker")
    );
    assert_eq!(line("arrive"), "arrive Ready");
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
            && far.ends_with("units=[0:1=10 1:1=20 2:1=3 3:1=4 4:1=2 6:1=1]"),
        "the closed turn's tokens, its two admitted seconds and the answered session's fee \
         unit, reported: {far}"
    );
    assert!(line("collect ").contains("emitted= more=0 far=false done=false"));
    assert!(
        line("short ").starts_with("short Failed") && line("short ").ends_with("units=[needed 6]"),
        "{}",
        line("short ")
    );
    let recall = line("short re-call");
    assert!(
        recall.contains("far=true")
            && recall.ends_with("units=[0:1=10 1:1=20 2:1=3 3:1=4 4:1=4 6:1=1]"),
        "the re-call answers the same piece once, its audio not counted twice (the open turn's \
         two running seconds over the closed turn's two): {recall}"
    );
    let end = line("caller end");
    assert!(
        end.contains("done=true") && end.ends_with("units=[0:1=10 1:1=20 2:1=3 3:1=4 4:1=4 6:1=1]"),
        "the caller's end settles the open turn's two seconds once: {end}"
    );
    assert!(line("request unit on a session door")
        .starts_with("request unit on a session door Refused"));
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
/// composition folds from the top-level models catalog); claim 6 (the metadata path) is no claim
/// and arrives nowhere; with no model configured nothing is named (the door steps refuse an unnamed keyed route).
/// Both ways.
#[test]
fn a_door_that_reaches_the_far_end_names_the_session_model_as_its_direct_route() {
    use busbar_contract::abi::plane::{ROUTE_DIRECT, ROUTE_POOL};
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        for claim in [0, 1, 2, 3, 4, PLAIN_CLAIM] {
            assert_eq!(
                arrive_route(p, claim),
                (Outcome::Ready, ROUTE_DIRECT, "m-cap".to_string()),
                "claim {claim}"
            );
        }
        assert_ne!(
            arrive_route(p, 6).0,
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
        let mut b = Bufs::with_units(1 << 18, 8);
        // The sideband session attaches to a call the SDP door brokered for its caller.
        let _ = broker(p, &mut b, 39, CALLER_REF, LIVE_CALL_LOCATION);
        assert_eq!(arrive_at(p, 2, 40, LIVE_SIDEBAND), Outcome::Ready);
        let answer = b.call(p, &session(40, FROM_CALLER, 0, &uplink(100)));
        assert!(answer.contains("far=true"), "{answer}");
        assert_eq!(b.last_need, door::RIDES_REALTIME_SOCKET, "{answer}");
        let mut b = Bufs::new(1 << 16);
        let answer = b.call(p, &attempt(41, 0));
        assert!(answer.contains("far=true"), "{answer}");
        assert_eq!(b.last_need, door::RIDES_REALTIME_PASS, "{answer}");
    }
}

/// A refused open (a hook's veto, an admission's refusal) is answered in the dialects' error shape:
/// the kernel's status read as the error's type, its words as the message.
#[test]
fn a_refusal_is_rendered_in_the_dialects_error_shape() {
    for (status, kind) in [
        (401, "authentication_error"),
        (403, "permission_error"),
        (429, "rate_limit_error"),
        (400, "invalid_request_error"),
        (503, "server_error"),
    ] {
        let body: serde_json::Value =
            serde_json::from_slice(&door::refusal_body(status, "no sessions today")).expect("JSON");
        assert_eq!(body["error"]["type"], kind, "{status}");
        assert_eq!(body["error"]["message"], "no sessions today");
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
        "message": door::AUTH_FAILURE_MESSAGE,
        "type": "authentication_error",
        "param": null,
        "code": door::AUTH_FAILURE_CODE,
    }});
    assert_eq!(door::AUTH_FAILURE_MESSAGE, "Incorrect API key provided.");
    assert_eq!(door::AUTH_FAILURE_CODE, "invalid_api_key");
    let kernel_word: serde_json::Value =
        serde_json::from_slice(&door::refusal_body(401, "unauthenticated")).expect("JSON");
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
        let _ = broker(p, &mut b, 129, CALLER_REF, LIVE_CALL_LOCATION);
        for unit in [130, 132] {
            assert_eq!(arrive_at(p, 2, unit, LIVE_SIDEBAND), Outcome::Ready);
        }
        let audio = uplink(100);
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
            door::refusal_body(429, "the budget is exhausted").as_slice(),
            "a refusal before the upgrade keeps the REST envelope"
        );
    }
}

/// RED-BEFORE-GREEN (busbar row 159, `crates/busbar-plane-streaming/src` round 2: "The sideband
/// door attaches to no call"; Part 2 #45: OpenAI Realtime over WebRTC, the browser never holding
/// the real key; OpenAI's sideband connection is `wss://api.openai.com/v1/realtime?call_id=rtc_…`,
/// the call id read off the SDP answer's `Location`). A sideband session dials the provider call
/// its arrival's `{call_id}` names (any query on the arrival dropped), and only a call the SDP door
/// brokered for the same caller: a sideband with no arrival, one naming a call no offer here
/// created, one naming another caller's call, and one whose call id could carry another query
/// parameter are each refused, never dialled as a fresh session. RED: every sideband dialled bare
/// `/v1/realtime`, a new session unrelated to the browser's call. Both ways.
#[test]
fn a_sideband_session_attaches_to_the_call_its_arrival_names() {
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    let mut transcripts = Vec::new();
    for p in &doors {
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::with_units(1 << 18, 8);
        let audio = uplink(100);
        let brokered = broker(p, &mut b, 40, CALLER_REF, "/v1/realtime/calls/rtc_both0040");
        assert!(brokered.contains("done=true status=201"), "{brokered}");

        assert_eq!(
            arrive_at(p, 2, 41, "/v1/realtime/sideband/rtc_both0040"),
            Outcome::Ready
        );
        let attached = b.call(p, &session(41, FROM_CALLER, 0, &audio));
        assert!(
            attached.starts_with("Ready")
                && attached.contains("far=true")
                && attached.contains("verb=GET target=/v1/realtime?call_id=rtc_both0040 "),
            "the sideband dials the call its arrival names: {attached}"
        );

        assert_eq!(
            arrive_at(p, 2, 42, "/v1/realtime/sideband/rtc_both0040?model=other#f"),
            Outcome::Ready
        );
        let queried = b.call(p, &session(42, FROM_CALLER, 0, &audio));
        assert!(
            queried.contains("verb=GET target=/v1/realtime?call_id=rtc_both0040 "),
            "the arrival's own query never reaches the dial: {queried}"
        );

        let bare = b.call(p, &session(43, FROM_CALLER, 0, &audio));
        assert!(
            bare.starts_with("Refused"),
            "a sideband whose arrival named no call is refused: {bare}"
        );

        assert_eq!(
            arrive_at(p, 2, 44, "/v1/realtime/sideband/rtc_nobody0044"),
            Outcome::Ready
        );
        let unknown = b.call(p, &session(44, FROM_CALLER, 0, &audio));
        assert!(
            unknown.starts_with("Refused"),
            "a call no offer here brokered is refused: {unknown}"
        );

        assert_eq!(
            arrive_at(p, 2, 45, "/v1/realtime/sideband/rtc_both0040"),
            Outcome::Ready
        );
        let other = b.call(
            p,
            &Push {
                caller: "0ther",
                ..session(45, FROM_CALLER, 0, &audio)
            },
        );
        assert!(
            other.starts_with("Refused"),
            "another caller's call is refused: {other}"
        );

        assert_eq!(
            arrive_at(p, 2, 46, "/v1/realtime/sideband/rtc_both0040%26model%3Dx"),
            Outcome::Ready
        );
        let smuggled = b.call(p, &session(46, FROM_CALLER, 0, &audio));
        assert!(
            smuggled.starts_with("Refused"),
            "a call id that could carry another parameter is refused: {smuggled}"
        );
        transcripts.push([attached, queried, bare, unknown, other, smuggled]);
    }
    assert_eq!(
        transcripts[0], transcripts[1],
        "the two doors must answer identically"
    );
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
            Outcome::Ready
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
/// mint request's locked `session.model`, the locked `model` a caller's `session.update` is spliced
/// with on its way to the provider, and the `model` of a Gemini Live caller's `setup`. RED: each
/// named the catalog name (`m-cap`), which reaches the provider as an unknown model. Both ways.
#[test]
fn every_upstream_facing_model_field_names_the_upstream_model() {
    let d = Dispatcher::new(DispatchConfig::default());
    let doors = vec![linked(&d), dropped(&d)];
    let mut transcripts = Vec::new();
    for p in &doors {
        assert_eq!(open(p, 1, KEYED_UPSTREAM, Some(PUBLIC)).0, Outcome::Ready);
        let mut b = Bufs::with_units(1 << 18, 8);
        let mint = b.call(p, &attempt(70, 0));
        assert!(
            mint.contains(r#""model":"gpt-realtime-wire""#) && !mint.contains("m-cap"),
            "the mint request names the upstream model: {mint}"
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
