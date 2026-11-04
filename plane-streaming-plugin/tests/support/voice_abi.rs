// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Concrete streaming ABI frames used by the ordinary voice cases.
//! No runner, case interpreter, simulated host, or expected-answer engine lives here.

use busbar_contract::abi::mechanism::call::{AbiStr, Blob, Outcome, Span, BLOB_JSON, BLOB_OCTETS};
use busbar_contract::abi::mechanism::lifecycle::{slot as life, TickIn, TickOut};
use busbar_contract::abi::plane::{
    slot, ArriveIn, ArriveOut, OnPieceIn, OnPieceOut, OutField, PlaneDriveIn, PlaneDriveOut,
    PlaneOpenIn, PlaneOpenOut, RecordWrite, RefusalIn, RefusalOut, UnitCount, CLAIM_EXACT,
    CLAIM_OPEN, EMIT_DONE, EMIT_TO_FAR_END, REFUSAL_KERNEL,
};
use busbar_plugin_loader::dispatch::kinds::plane::Plane;
use busbar_plugin_loader::dispatch::{
    in_head, load_dropped, load_linked, out_head, Bind, Dispatcher, Frame, LinkedRow, NoSink,
    Plugin,
};
use std::sync::Arc;
#[path = "artifact.rs"]
mod artifact;

pub const PUBLIC: &str = "https://gw.example.com/ignored?q=1#f";
pub const SESSION: &[u8] = br#"{"session":{"model":"m-cap"}}"#;
pub const MODEL: &str = "m-cap";
pub const CALLER_REF: &str = "c0ffee";

pub fn load(dispatcher: &Dispatcher, instance: &str, dropped: bool) -> Plugin<Plane> {
    let row = LinkedRow::of(busbar_plane_streaming::door)
        .expect("the actual streaming door states itself");
    let bind = Bind {
        instance: Arc::from(instance),
        max_inflight_cap: 8,
        sink: Arc::new(NoSink),
        dispatcher: dispatcher.adopter(),
        conns: busbar_plugin_loader::dispatch::ConnTable::Probe,
    };
    if dropped {
        load_dropped::<Plane>(&artifact::plugin_cdylib(), &row.statement, bind)
            .expect("the fresh actual streaming image loads")
    } else {
        load_linked::<Plane>(&row, bind).expect("the actual linked streaming door loads")
    }
}

fn json(b: &'static [u8]) -> Blob {
    Blob {
        ptr: b.as_ptr(),
        len: b.len(),
        fmt: BLOB_JSON,
        flags: 0,
    }
}
fn octets(b: &[u8]) -> Blob {
    Blob {
        ptr: b.as_ptr(),
        len: b.len(),
        fmt: BLOB_OCTETS,
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
    if s.len == 0 {
        return String::new();
    }
    assert!(!s.ptr.is_null(), "nonempty actual ABI text has a pointer");
    // SAFETY: the actual plugin owns the stated string until its live generation is retired.
    String::from_utf8(unsafe { std::slice::from_raw_parts(s.ptr, s.len) }.to_vec())
        .expect("ABI text is UTF-8")
}
fn at(buf: &[u8], span: Span) -> String {
    let start = span.offset as usize;
    let end = start.checked_add(span.len as usize).expect("span end fits");
    String::from_utf8(
        buf.get(start..end)
            .expect("actual span is inside the lent buffer")
            .to_vec(),
    )
    .expect("ABI text is UTF-8")
}
pub fn dials_socket(target: &str, socket: &str) -> bool {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    path == socket
        && (query.is_empty()
            || query
                .strip_prefix("model=")
                .is_some_and(|m| !m.is_empty() && !m.contains(['&', '='])))
}

/// One claim a snapshot publishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claimed {
    /// `VERB TARGET CARRIER[ exact][ open]`.
    pub line: String,
    /// The dialect a refusal on it is rendered in.
    pub refusal_dialect: u16,
}

/// What an `open` answered: its outcome, and the snapshot it published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    /// The outcome.
    pub outcome: Outcome,
    /// The published claims.
    pub claims: Vec<Claimed>,
    /// The audience a caller's token must carry.
    pub audience: String,
}

/// `open` generation 1 over `settings`, under `public_url`.
pub fn open(
    p: &Plugin<Plane>,
    settings: &'static [u8],
    public_url: Option<&'static str>,
) -> Opened {
    // SAFETY: these concrete ABI frames have zero-valid scalar/pointer fields. All input bytes
    // are static and the output snapshot is copied before any generation can be retired.
    let mut i: PlaneOpenIn = unsafe { std::mem::zeroed() };
    i.open.head = in_head();
    i.open.generation = 1;
    i.open.settings = json(settings);
    if let Some(u) = public_url {
        i.public_url = text(u);
    }
    let mut o: PlaneOpenOut = unsafe { std::mem::zeroed() };
    o.open.head = out_head();
    let mut f = Frame::new(i, o);
    let c = p.call(life::OPEN, &mut f);
    let mut opened = Opened {
        outcome: c.outcome,
        claims: Vec::new(),
        audience: String::new(),
    };
    let snap = f.out.snapshot;
    if c.outcome != Outcome::Ready || snap.is_null() {
        return opened;
    }
    // SAFETY: a READY open publishes its generation's snapshot, valid until that generation's
    // `retire` (never called here).
    let s = unsafe { &*snap };
    for n in 0..s.claims_len {
        // SAFETY: the snapshot names `claims_len` claims.
        let c = unsafe { &*s.claims.add(n) };
        let exact = if c.flags & CLAIM_EXACT != 0 {
            " exact"
        } else {
            ""
        };
        let open = if c.flags & CLAIM_OPEN != 0 {
            " open"
        } else {
            ""
        };
        opened.claims.push(Claimed {
            line: format!(
                "{} {} {}{exact}{open}",
                read(c.verb),
                read(c.target),
                read(c.carrier)
            ),
            refusal_dialect: c.refusal_dialect,
        });
    }
    opened.audience = read(s.audience);
    opened
}

/// What `arrive` answered for a claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arrived {
    /// The outcome.
    pub outcome: Outcome,
    /// The dialect index.
    pub dialect: u32,
    /// The op class index.
    pub op_class: u32,
    /// The principal need.
    pub principal: u32,
    /// The route class and the entry it names.
    pub route: (u8, String),
    /// The admission route flags, including counted admission.
    pub route_flags: u8,
    /// The plane's refusal code and status.
    pub refusal: (u32, u32),
    /// The units the admission was told to expect.
    pub units: u32,
}

/// `arrive` on `claim`.
pub fn arrive(p: &Plugin<Plane>, claim: u32) -> Arrived {
    // SAFETY: these concrete C ABI frames and output elements contain only zero-valid scalars
    // and pointers. Their pointed-to buffers remain owned by this synchronous call.
    let mut units: Vec<UnitCount> = (0..8).map(|_| unsafe { std::mem::zeroed() }).collect();
    let mut a: Frame<ArriveIn, ArriveOut> =
        Frame::new(unsafe { std::mem::zeroed::<ArriveIn>() }, unsafe {
            std::mem::zeroed::<ArriveOut>()
        });
    a.input.head = in_head();
    a.input.claim = claim;
    a.input.unit = 900 + u64::from(claim);
    a.input.units_buf = units.as_mut_ptr();
    a.input.units_cap = units.len();
    a.out.head = out_head();
    let c = p.call(slot::ARRIVE, &mut a);
    Arrived {
        outcome: c.outcome,
        dialect: a.out.dialect,
        op_class: a.out.op_class,
        principal: a.out.principal_need,
        route: (a.out.route, read(a.out.pool)),
        route_flags: a.out.route_flags,
        refusal: (a.out.refusal, a.out.refusal_status),
        units: a.out.units_written,
    }
}

/// ONE RECORD WRITE a door's answer made, read back: its op (`RECORD_PUT`, a record of the door's
/// own, or `RECORD_AUDIT`, the unit's audit row on the kernel's chain: the one fixed record), its
/// kind (a put's record kind; an audit row's outcome), its key (an audit row's action) and its
/// value (an audit row's resource).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// `RECORD_PUT` | `RECORD_AUDIT`.
    pub op: u32,
    /// A put's record kind; an audit row's outcome (`AUDIT_*`).
    pub kind: u32,
    /// The key: an audit row's action.
    pub key: String,
    /// The value: an audit row's resource.
    pub value: String,
}

/// The first `written` record writes in `records`, read back over `arena`.
fn rows(records: &[RecordWrite], written: u32, arena: &[u8]) -> Vec<Row> {
    let n = written as usize;
    assert!(
        n <= records.len(),
        "actual record count exceeds the lent capacity"
    );
    records[..n]
        .iter()
        .map(|w| Row {
            op: w.op,
            kind: w.kind,
            key: at(arena, w.key),
            value: at(arena, w.value),
        })
        .collect()
}

/// What `refusal` rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    /// The outcome.
    pub outcome: Outcome,
    /// The rendered body.
    pub body: String,
    /// Its head fields.
    pub fields: Vec<(String, String)>,
    /// The status it answered (`0` = the kernel's).
    pub status: u32,
    /// The record writes it made.
    pub records: u32,
    /// Each of them, read back.
    pub rows: Vec<Row>,
}

/// The kernel refuses unit `unit` on `dialect` with `status` and `words`: the door renders it.
pub fn refusal(
    p: &Plugin<Plane>,
    unit: u64,
    dialect: u32,
    status: u32,
    words: &'static str,
) -> Rendered {
    refusal_for(p, unit, dialect, 0, status, words)
}

/// Render the kernel refusal for `unit` on `dialect`, using its ABI reason, status and words.
pub fn refusal_for(
    p: &Plugin<Plane>,
    unit: u64,
    dialect: u32,
    reason: u32,
    status: u32,
    words: &'static str,
) -> Rendered {
    // SAFETY: the concrete ABI frames/elements are zero-valid. Every mutable buffer remains
    // exclusively owned here and all spans are copied while the synchronous call is live.
    let mut reply = vec![0u8; 4096];
    let mut fields: Vec<OutField> = (0..4).map(|_| unsafe { std::mem::zeroed() }).collect();
    let mut arena = vec![0u8; 512];
    let mut records: Vec<RecordWrite> = (0..4).map(|_| unsafe { std::mem::zeroed() }).collect();
    let mut i: RefusalIn = unsafe { std::mem::zeroed() };
    i.head = in_head();
    i.cause = REFUSAL_KERNEL;
    i.status = status;
    i.reason = reason;
    i.dialect = dialect;
    i.text = text(words);
    i.unit = unit;
    (i.reply_buf, i.reply_cap) = (reply.as_mut_ptr(), reply.len());
    (i.fields_buf, i.fields_cap) = (fields.as_mut_ptr(), fields.len());
    (i.arena_buf, i.arena_cap) = (arena.as_mut_ptr(), arena.len());
    (i.records_buf, i.records_cap) = (records.as_mut_ptr(), records.len());
    let mut o: RefusalOut = unsafe { std::mem::zeroed() };
    o.head = out_head();
    let mut f = Frame::new(i, o);
    let c = p.call(slot::REFUSAL, &mut f);
    if c.outcome != Outcome::Ready {
        return Rendered {
            outcome: c.outcome,
            body: String::new(),
            fields: Vec::new(),
            status: 0,
            records: 0,
            rows: Vec::new(),
        };
    }
    let out = f.out;
    let written = out.reply_written as usize;
    let n = out.fields_written as usize;
    assert!(
        written <= reply.len() && n <= fields.len(),
        "actual refusal exceeds its buffers"
    );
    Rendered {
        outcome: c.outcome,
        body: String::from_utf8_lossy(&reply[..written]).into_owned(),
        fields: fields[..n]
            .iter()
            .map(|f| (at(&arena, f.name), at(&arena, f.value)))
            .collect(),
        status: out.status,
        records: out.records_written,
        rows: rows(&records, out.records_written, &arena),
    }
}

/// `drive`: the streams whose sessions owe output of their own.
pub fn drive(p: &Plugin<Plane>) -> (Outcome, Vec<u64>) {
    // SAFETY: these concrete frames contain zero-valid fields; the session buffer is local.
    let mut sessions = [0_u64; 16];
    let mut f: Frame<PlaneDriveIn, PlaneDriveOut> =
        Frame::new(unsafe { std::mem::zeroed::<PlaneDriveIn>() }, unsafe {
            std::mem::zeroed::<PlaneDriveOut>()
        });
    f.input.drive.head = in_head();
    (f.input.sessions_buf, f.input.sessions_cap) = (sessions.as_mut_ptr(), sessions.len());
    f.out.head = out_head();
    let c = p.call(life::DRIVE, &mut f);
    if c.outcome != Outcome::Ready {
        return (c.outcome, Vec::new());
    }
    let n = f.out.sessions_written as usize;
    assert!(
        n <= sessions.len(),
        "actual DRIVE exceeds its session buffer"
    );
    (c.outcome, sessions[..n].to_vec())
}

/// `tick` at `now_ns`: its outcome and the next tick it asks for.
pub fn tick(p: &Plugin<Plane>, now_ns: u64) -> (Outcome, u64) {
    // SAFETY: these concrete ABI frames contain only zero-valid scalar/pointer fields.
    let mut k: Frame<TickIn, TickOut> =
        Frame::new(unsafe { std::mem::zeroed::<TickIn>() }, unsafe {
            std::mem::zeroed::<TickOut>()
        });
    k.input.head = in_head();
    k.input.now_ns = now_ns;
    k.out.head = out_head();
    let c = p.call(life::TICK, &mut k);
    if c.outcome != Outcome::Ready {
        return (c.outcome, 0);
    }
    (c.outcome, k.out.next_tick_ns)
}

/// One piece pushed to a unit.
#[derive(Clone, Copy, Debug)]
pub struct Piece<'a> {
    /// The kernel's unit key.
    pub unit: u64,
    /// A live session's stream (`0` = a request unit).
    pub stream: u64,
    /// The claim it arrived on.
    pub claim: u32,
    /// Who pushed it.
    pub from: u32,
    /// `PIECE_*`.
    pub flags: u32,
    /// An ATTEMPT's number (`0` = none).
    pub attempt_no: u32,
    /// Its bytes.
    pub bytes: &'a [u8],
    /// The far end's status, with `PIECE_HAS_STATUS`.
    pub status: u32,
    /// The capacity of the units buffer the host lends.
    pub units_cap: usize,
}

/// A piece of request unit `unit` on `claim`.
pub const fn request(unit: u64, claim: u32, from: u32, flags: u32, bytes: &[u8]) -> Piece<'_> {
    Piece {
        unit,
        stream: 0,
        claim,
        from,
        flags,
        attempt_no: 0,
        bytes,
        status: 0,
        units_cap: 16,
    }
}

/// A piece of the live session on stream `stream` (claim `claim`).
pub const fn session(stream: u64, claim: u32, from: u32, flags: u32, bytes: &[u8]) -> Piece<'_> {
    Piece {
        stream,
        ..request(stream, claim, from, flags, bytes)
    }
}

/// What the door answered one piece with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The outcome.
    pub outcome: Outcome,
    /// The bytes it emitted.
    pub emitted: Vec<u8>,
    /// `more`.
    pub more: u32,
    /// The emission is bound for the far end.
    pub to_far: bool,
    /// The unit (or session) is done.
    pub done: bool,
    /// The caller's status.
    pub status: u32,
    /// The far request's verb and target.
    pub verb: String,
    /// See `verb`.
    pub target: String,
    /// The head fields it wrote.
    pub fields: Vec<(String, String)>,
    /// The units it reported: (class, source, amount).
    pub units: Vec<(u32, u32, u64)>,
    /// A short answer's units room.
    pub units_needed: u32,
    /// The declared need a far request rides (`0` = none).
    pub need: u32,
    /// The record writes it made.
    pub records: u32,
    /// Each of them, read back.
    pub rows: Vec<Row>,
}

impl Answer {
    fn empty(outcome: Outcome, units_needed: u32) -> Self {
        Self {
            outcome,
            emitted: Vec::new(),
            more: 0,
            to_far: false,
            done: false,
            status: 0,
            verb: String::new(),
            target: String::new(),
            fields: Vec::new(),
            units: Vec::new(),
            units_needed,
            need: 0,
            records: 0,
            rows: Vec::new(),
        }
    }
    /// The units as `class:source=amount` words.
    pub fn units_line(&self) -> String {
        if self.units_needed != 0 {
            return format!("needed {}", self.units_needed);
        }
        self.units
            .iter()
            .map(|(c, s, a)| format!("{c}:{s}={a}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Push `x` and read the answer.
pub fn piece(p: &Plugin<Plane>, x: &Piece<'_>) -> Answer {
    // SAFETY: these concrete C ABI frames/elements are zero-valid. Input bytes and all output
    // buffers remain owned through the synchronous call, and no buffer is read before Ready.
    let mut reply = vec![0u8; 1 << 18];
    let mut units: Vec<UnitCount> = (0..x.units_cap)
        .map(|_| unsafe { std::mem::zeroed() })
        .collect();
    let mut records: Vec<RecordWrite> = (0..4).map(|_| unsafe { std::mem::zeroed() }).collect();
    let mut fields: Vec<OutField> = (0..8).map(|_| unsafe { std::mem::zeroed() }).collect();
    let mut arena = vec![0u8; 4096];
    let mut i: OnPieceIn = unsafe { std::mem::zeroed() };
    i.head = in_head();
    (i.unit, i.stream, i.claim, i.from, i.flags) = (x.unit, x.stream, x.claim, x.from, x.flags);
    (i.attempt_no, i.bytes, i.status_code) = (x.attempt_no, octets(x.bytes), x.status);
    i.caller_ref = text(CALLER_REF);
    (i.reply_buf, i.reply_cap) = (reply.as_mut_ptr(), reply.len());
    (i.units_buf, i.units_cap) = (units.as_mut_ptr(), units.len());
    (i.records_buf, i.records_cap) = (records.as_mut_ptr(), records.len());
    (i.fields_buf, i.fields_cap) = (fields.as_mut_ptr(), fields.len());
    (i.arena_buf, i.arena_cap) = (arena.as_mut_ptr(), arena.len());
    let mut o: OnPieceOut = unsafe { std::mem::zeroed() };
    o.head = out_head();
    let mut f = Frame::new(i, o);
    let c = p.call(slot::ON_PIECE, &mut f);
    let o = f.out;
    if c.outcome != Outcome::Ready {
        // Failed output exposes needed counts only; Pending/Refused/Fault expose no piece data.
        let needed = if c.outcome == Outcome::Failed {
            o.units_needed
        } else {
            0
        };
        return Answer::empty(c.outcome, needed);
    }
    let emitted = o.emitted as usize;
    let nu = o.units_written as usize;
    let nf = o.fields_written as usize;
    assert!(
        emitted <= reply.len() && nu <= units.len() && nf <= fields.len(),
        "actual piece output exceeds its lent buffers"
    );
    Answer {
        outcome: c.outcome,
        emitted: reply[..emitted].to_vec(),
        more: o.more,
        to_far: o.flags & EMIT_TO_FAR_END != 0,
        done: o.flags & EMIT_DONE != 0,
        status: o.reply_status,
        verb: at(&arena, o.verb),
        target: at(&arena, o.target),
        fields: fields[..nf]
            .iter()
            .map(|f| (at(&arena, f.name), at(&arena, f.value)))
            .collect(),
        units: units[..nu]
            .iter()
            .map(|u| (u.class, u.source, u.amount))
            .collect(),
        units_needed: o.units_needed,
        need: o.need,
        records: o.records_written,
        rows: rows(&records, o.records_written, &arena),
    }
}
