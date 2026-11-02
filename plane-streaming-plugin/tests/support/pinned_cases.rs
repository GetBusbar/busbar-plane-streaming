// SPDX-License-Identifier: Apache-2.0
//! Ordinary streaming-specific cases formerly driven by the published plane script.
use super::*;
use busbar_contract::abi::mechanism::call::OutHead;
use busbar_contract::abi::plane::{RefusalIn, RefusalOut, ServeIn, ServeOut};
use std::sync::atomic::Ordering;

/// The configured streaming credential's original seven supported spellings.
fn credential_spellings() -> Vec<Vec<u8>> {
    let inputs: serde_json::Value =
        serde_json::from_str(include_str!("../conformance.json")).unwrap();
    let credentials = inputs["credentials"].as_array().unwrap();
    assert_eq!(credentials.len(), 1);
    let secret = credentials[0].as_str().unwrap().as_bytes();
    assert_eq!(secret, b"bb-canary-streaming-5c1e7a");
    let percent = |upper: bool| -> Vec<u8> {
        secret
            .iter()
            .flat_map(|byte| {
                if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'.' | b'_' | b'~') {
                    vec![*byte]
                } else if upper {
                    format!("%{byte:02X}").into_bytes()
                } else {
                    format!("%{byte:02x}").into_bytes()
                }
            })
            .collect()
    };
    let base64 = |alphabet: &[u8]| -> Vec<u8> {
        let mut encoded = Vec::new();
        for block in secret.chunks(3) {
            let a = block[0];
            let b = block.get(1).copied().unwrap_or(0);
            let c = block.get(2).copied().unwrap_or(0);
            encoded.push(alphabet[usize::from(a >> 2)]);
            encoded.push(alphabet[usize::from(((a & 3) << 4) | (b >> 4))]);
            if block.len() > 1 {
                encoded.push(alphabet[usize::from(((b & 15) << 2) | (c >> 6))]);
            }
            if block.len() > 2 {
                encoded.push(alphabet[usize::from(c & 63)]);
            }
        }
        encoded
    };
    let mut spellings = vec![
        secret.to_vec(),
        percent(true),
        percent(false),
        base64(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"),
        base64(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"),
        secret
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            .into_bytes(),
        secret
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<String>()
            .into_bytes(),
    ];
    spellings.sort();
    spellings.dedup();
    spellings
}
pub(super) fn assert_credential_absent(bytes: &[u8]) {
    for spelling in credential_spellings() {
        assert!(
            !bytes.windows(spelling.len()).any(|part| part == spelling),
            "configured streaming credential escaped in an original output sink"
        );
    }
}

fn arrival(
    p: &Plugin<Plane>,
    claim: u32,
    unit: u64,
    target: &'static str,
) -> (Outcome, u32, String) {
    let mut f: Frame<ArriveIn, ArriveOut> = Frame::new(z(), z());
    f.input.head = in_head();
    f.input.claim = claim;
    f.input.unit = unit;
    f.input.target = text(target);
    f.input.method = text(if claim == 1 { "POST" } else { "GET" });
    let canary = [Field {
        name: text("authorization"),
        value: text("Bearer bb-canary-streaming-5c1e7a"),
    }];
    if claim == 1 {
        f.input.fields = canary.as_ptr();
        f.input.fields_len = 1;
    }
    let mut units = [z::<UnitCount>(); 4];
    f.input.units_buf = units.as_mut_ptr();
    f.input.units_cap = units.len();
    f.out.head = out_head();
    let c = p.call(slot::ARRIVE, &mut f);
    if let Some(error) = &c.error {
        assert_credential_absent(error);
    }
    assert_credential_absent(read(f.out.pool).as_bytes());
    (c.outcome, f.out.refusal_status, format!(
        "lease={} error={:?} refusal={} status={} op_class={} principal_required={} dialect={} units={} route_flags={} route={} entry={}",
        c.lease, c.error, f.out.refusal, f.out.refusal_status, f.out.op_class,
        f.out.principal_need == busbar_contract::abi::plane::PRINCIPAL_REQUIRED,
        f.out.dialect, f.out.units_written, f.out.route_flags, f.out.route, read(f.out.pool)))
}
fn observed_open(p: &Plugin<Plane>) -> (Outcome, String, String) {
    let mut f: Frame<PlaneOpenIn, PlaneOpenOut> = Frame::new(z(), z());
    f.input.open.head = in_head();
    f.out.open.head = out_head();
    f.input.open.generation = 1;
    f.input.open.settings = json(SESSION);
    f.input.public_url = text(PUBLIC);
    let c = p.call(life::OPEN, &mut f);
    if let Some(error) = &c.error {
        assert_credential_absent(error);
    }
    let snap = if c.outcome == Outcome::Ready {
        snapshot(f.out.snapshot)
    } else {
        String::new()
    };
    (
        c.outcome,
        snap,
        format!("{:?} lease={} error={:?}", c.outcome, c.lease, c.error),
    )
}
/// Memory lent only to this streaming session's real submitted ON_PIECE.
/// The dispatcher owns a clone until completion, including cancellation/hung-call cleanup.
struct StreamingPiece {
    buffers: std::cell::UnsafeCell<Bufs>,
    bytes: Vec<u8>,
}
// SAFETY: bytes are immutable. Only the dispatched crossing writes output buffers; the test
// accesses them before submission or after Done returns the frame, never while it can run.
unsafe impl Send for StreamingPiece {}
// SAFETY: the same exclusive-crossing ownership applies to every raw output pointer.
unsafe impl Sync for StreamingPiece {}
fn submitted_piece(
    p: &Plugin<Plane>,
    d: &Dispatcher,
    ticket: busbar_contract::abi::mechanism::ticket::Ticket,
    buffers: &mut Bufs,
    push: &Push<'_>,
) -> String {
    assert!(
        push.head.is_empty(),
        "these exact pinned session cases lend no head fields"
    );
    let memory = Arc::new(StreamingPiece {
        buffers: std::cell::UnsafeCell::new(std::mem::replace(buffers, Bufs::new(0))),
        bytes: push.bytes.to_vec(),
    });
    let owned_push = Push {
        bytes: &memory.bytes,
        ..*push
    };
    // SAFETY: no crossing exists yet, so output storage is exclusively ours.
    let mut frame = unsafe { (&mut *memory.buffers.get()).frame(&owned_push) };
    frame.input.member = text("m-cap");
    let done = d
        .submit_lent(
            p,
            ticket,
            slot::ON_PIECE,
            frame,
            busbar_contract::abi::mechanism::call::DeadlineClass::Stream,
            busbar_plugin_loader::dispatch::now_ns().saturating_add(30_000_000_000),
            memory.clone(),
        )
        .wait_done();
    assert!(
        !done.detached,
        "the actual streaming op completes under its lent owner"
    );
    let out = done.frame.as_ref().expect("completed streaming frame").out;
    let called = busbar_plugin_loader::dispatch::Called {
        outcome: done.outcome,
        error: done.error,
        lease: done.lease,
        recall: None,
    };
    // SAFETY: Done has returned the frame; the dispatcher no longer writes these buffers.
    let rendered = unsafe { (&mut *memory.buffers.get()).answer(&called, out) };
    // SAFETY: the joined frame proves this op cannot access its lent pointers again.
    // Reclaim without assuming when the worker drops its final owner clone.
    *buffers = unsafe { std::mem::replace(&mut *memory.buffers.get(), Bufs::new(0)) };
    format!(
        "{rendered} lease={} error={:?} need={} lane={}",
        called.lease,
        called.error,
        out.need,
        at(&buffers.arena, out.lane)
    )
}
fn cases(p: &Plugin<Plane>, d: &Dispatcher) -> Vec<String> {
    let first =
        || p.inner.crossings.load(Ordering::SeqCst) - p.inner.resumes.load(Ordering::SeqCst);
    let mut answers = Vec::new();
    let mut call_answers = Vec::new();

    {
        let before = first();
        answers.push(format!(
            "{:?} {} max_inflight={}",
            p.kind(),
            p.name(),
            p.max_inflight()
        ));
        assert_eq!(first() - before, 0, "facts: exact first crossings");
    }
    for settings in [
        br#"{"streams":{"nonsense_key":1}}"#.as_slice(),
        br#"{"streams":{"session_max_secs":0}}"#,
    ] {
        let before = first();
        let mut error = [0u8; 512];
        let mut input: ValidateIn = z();
        input.head = in_head();
        input.settings = unsafe {
            Blob::from_raw_parts(
                settings.as_ptr(),
                settings.len(),
                busbar_contract::abi::mechanism::call::BLOB_JSON,
                0,
            )
        };
        input.err_buf = error.as_mut_ptr();
        input.err_cap = error.len();
        let mut f = Frame::new(input, out_head());
        let c = p.call(life::VALIDATE, &mut f);
        call_answers.push(format!(
            "{:?} lease={} error={:?}",
            c.outcome, c.lease, c.error
        ));
        if let Some(error) = &c.error {
            assert_credential_absent(error);
        }
        assert_ne!(c.outcome, Outcome::Ready);
        assert!(
            c.error.as_ref().is_some_and(|s| !s.is_empty()),
            "bad settings state why"
        );
        assert_eq!(first() - before, 1, "bad settings validation crossing");
        answers.push(format!("validate bad {:?} {}", c.outcome, words(c.error)));
    }

    {
        let before = first();
        let mut f = Frame::new(
            ValidateIn {
                head: in_head(),
                settings: json(br#"{"streams":{"session":{"model":"m-cap"}}}"#),
                err_buf: std::ptr::null_mut(),
                err_cap: 0,
            },
            out_head(),
        );
        let c = p.call(life::VALIDATE, &mut f);
        assert_eq!(c.outcome, Outcome::Ready);
        if let Some(error) = &c.error {
            assert_credential_absent(error);
        }
        answers.push(format!(
            "validate {:?} lease={} error={:?}",
            c.outcome, c.lease, c.error
        ));
        assert_eq!(first() - before, 1, "validate: exact first crossings");
    }

    {
        let before = first();
        let got = arrival(p, 1, 7, "/v1/realtime/calls");
        assert_eq!(got.0, Outcome::Refused);
        answers.push(format!("arrive unopened Refused {}", got.2));
        assert_eq!(
            first() - before,
            0,
            "arrive unopened: exact first crossings"
        );
    }

    {
        let before = first();
        let (o, snap, called) = observed_open(p);
        assert_eq!(o, Outcome::Ready);
        assert_eq!(format!("open {o:?} {snap}"), expected(true)[6]);
        answers.push(format!("open {o:?} {snap} {called}"));
        assert_eq!(first() - before, 1, "open: exact first crossings");
    }
    {
        let before = first();
        let pinned = u64::from(p.has_ready());
        let ready = p.ready(d, std::time::Duration::from_secs(30));
        assert!(ready.is_ok());
        assert_eq!(first() - before, pinned, "ready crossings");
        answers.push(format!("ready has_ready={} {ready:?}", p.has_ready()));
    }

    {
        let before = first();
        let (outcome, _, called) = observed_open(p);
        assert_eq!(outcome, Outcome::Refused);
        answers.push(format!("open again Refused {called}"));
        assert_eq!(first() - before, 0, "open again: exact first crossings");
    }
    let driver = d.driver(p, 0).expect("streaming declares its driver");

    {
        let before = first();
        let mut f = Frame::new(
            GenIn {
                head: in_head(),
                generation: 1,
            },
            out_head(),
        );
        let c = p.call(slot::HYDRATE, &mut f);
        call_answers.push(format!(
            "{:?} lease={} error={:?}",
            c.outcome, c.lease, c.error
        ));
        if let Some(error) = &c.error {
            assert_credential_absent(error);
        }
        assert_eq!(c.outcome, Outcome::Ready);
        answers.push(format!("hydrate {:?}", c.outcome));
        assert_eq!(first() - before, 1, "hydrate: exact first crossings");
    }

    {
        let before = first();
        let mut f = Frame::new(
            GenIn {
                head: in_head(),
                generation: 1,
            },
            out_head(),
        );
        let c = p.call(slot::START, &mut f);
        call_answers.push(format!(
            "{:?} lease={} error={:?}",
            c.outcome, c.lease, c.error
        ));
        if let Some(error) = &c.error {
            assert_credential_absent(error);
        }
        assert_eq!(c.outcome, Outcome::Ready);
        answers.push(format!("start {:?}", c.outcome));
        assert_eq!(first() - before, 1, "start: exact first crossings");
    }

    {
        let before = first();
        let got = arrival(p, 1, 7, "/v1/realtime/calls");
        assert_eq!(got.0, Outcome::Ready);
        answers.push(format!("arrive claimed Ready {}", got.2));
        assert_eq!(first() - before, 1, "arrive claimed: exact first crossings");
    }

    {
        let before = first();
        let got = arrival(p, 9, 8, "/v1/unpublished");
        assert_eq!((got.0, got.1), (Outcome::Refused, 404));
        answers.push(format!("arrive unclaimed Refused status=404 {}", got.2));
        assert_eq!(
            first() - before,
            1,
            "arrive unclaimed: exact first crossings"
        );
    }
    let mut reported = std::collections::BTreeSet::new();
    // The exact live OpenAI cases from conformance.json; every piece crosses once.

    {
        let before = first();
        let got = arrival(p, 5, 20, "/v1/realtime?model=m-cap");
        assert_eq!(got.0, Outcome::Ready);
        answers.push(format!("live arrive Ready {}", got.2));
        assert_eq!(first() - before, 1, "live arrive: exact first crossings");
    }
    let live_ticket = d.mint(0).expect("actual OpenAI session ticket");
    let mut b = Bufs::with_units(262144, 8);
    b.fields.resize(8, z());
    b.fields_cap = 8;
    b.records.resize(8, z());
    b.arena.resize(1024, 0);
    let audio = uplink(2000);
    let tool=br#"{"type":"response.output_item.added","item":{"type":"function_call","call_id":"c1","name":"lookup"}}"#;
    let usage=br#"{"type":"response.done","response":{"usage":{"input_token_details":{"audio_tokens":10,"text_tokens":3,"image_tokens":2},"output_token_details":{"audio_tokens":20,"text_tokens":4}}}}"#;
    for (label, g) in [
        (
            "initial provider lock before caller input",
            live_push(20, 5, FROM_CALLER, 0, b""),
        ),
        ("caller audio", live_push(20, 5, FROM_CALLER, 0, &audio)),
        (
            "turn attempt",
            Push {
                attempt_no: 1,
                ..live_push(20, 5, FROM_KERNEL, 0, b"")
            },
        ),
        (
            "far tool call",
            Push {
                status: 101,
                ..live_push(20, 5, FROM_FAR_END, PIECE_HAS_STATUS, tool)
            },
        ),
        ("far usage", live_push(20, 5, FROM_FAR_END, 0, usage)),
        ("caller end", live_push(20, 5, FROM_CALLER, PIECE_LAST, b"")),
    ] {
        let before = first();
        let answer = submitted_piece(p, d, live_ticket, &mut b, &g);
        assert_eq!(first() - before, 1, "live {label} crossing");
        assert!(answer.starts_with("Ready"), "live {label}: {answer}");
        if matches!(
            label,
            "initial provider lock before caller input" | "caller audio"
        ) {
            assert_ne!(b.last_flags & EMIT_TO_FAR_END, 0, "{label}: {answer}");
            let route = answer.rsplit_once(" verb=").expect("rendered route").1;
            let (verb, rest) = route.split_once(" target=").unwrap();
            let target = rest.rsplit_once(" fields=").unwrap().0;
            assert_eq!(
                (verb, target),
                ("GET", "/v1/realtime?model=m-cap"),
                "{label}"
            );
        }
        if label == "caller end" {
            assert_ne!(b.last_flags & EMIT_DONE, 0, "{answer}");
        }
        for count in b.last_units.split_whitespace() {
            reported.insert(count.split(':').next().unwrap().parse::<usize>().unwrap());
        }
        answers.push(format!(
            "live {label} {answer} units=[{}] audit=[{}]",
            b.last_units, b.last_audit
        ));
    }
    d.recycle(live_ticket);
    let gemini_ticket = d.mint(0).expect("actual Gemini session ticket");
    let mut gemini = Bufs::with_units(65536, 8);
    gemini.fields.resize(8, z());
    gemini.fields_cap = 8;
    gemini.records.resize(8, z());
    gemini.arena.resize(1024, 0);
    const AUDIO_END: &[u8] = br#"{"realtimeInput":{"audioStreamEnd":true}}"#;
    const GEMINI_USAGE:&[u8]=br#"{"usageMetadata":{"promptTokenCount":30,"responseTokenCount":5,"promptTokensDetails":[{"modality":"VIDEO","tokenCount":20},{"modality":"TEXT","tokenCount":10}]}}"#;
    for (label, g) in [
        (
            "caller audio end",
            live_push(21, 3, FROM_CALLER, 0, AUDIO_END),
        ),
        (
            "far usage",
            Push {
                status: 101,
                ..live_push(21, 3, FROM_FAR_END, PIECE_HAS_STATUS, GEMINI_USAGE)
            },
        ),
        ("caller end", live_push(21, 3, FROM_CALLER, PIECE_LAST, b"")),
    ] {
        let before = first();
        let answer = submitted_piece(p, d, gemini_ticket, &mut gemini, &g);
        assert_eq!(first() - before, 1, "gemini {label} crossing");
        assert!(answer.starts_with("Ready"), "gemini {label}: {answer}");
        if label == "far usage" {
            assert_eq!(gemini.last_units, "2:1=10 3:1=5 7:1=20 8:1=1");
        }
        if label == "caller end" {
            assert_ne!(gemini.last_flags & EMIT_DONE, 0, "{answer}");
        }
        for count in gemini.last_units.split_whitespace() {
            reported.insert(count.split(':').next().unwrap().parse::<usize>().unwrap());
        }
        answers.push(format!(
            "gemini {label} {answer} units=[{}] audit=[{}]",
            gemini.last_units, gemini.last_audit
        ));
    }
    d.recycle(gemini_ticket);
    assert_eq!(
        reported,
        (0..p.served().billable_classes.len()).collect(),
        "every declared streaming billable class is emitted by a real case"
    );

    {
        let before = first();
        let mut f: Frame<RefusalIn, RefusalOut> = Frame::new(z(), z());
        let mut reply = [0u8; 512];
        let mut arena = [0u8; 256];
        let mut fields = [z::<OutField>(); 2];
        f.input.head = in_head();
        f.out.head = out_head();
        f.input.cause = busbar_contract::abi::plane::REFUSAL_GATE;
        f.input.status = 403;
        f.input.text = text("denied");
        f.input.reply_buf = reply.as_mut_ptr();
        f.input.reply_cap = reply.len();
        f.input.arena_buf = arena.as_mut_ptr();
        f.input.arena_cap = arena.len();
        f.input.fields_buf = fields.as_mut_ptr();
        f.input.fields_cap = fields.len();
        let c = p.call(slot::REFUSAL, &mut f);
        call_answers.push(format!(
            "{:?} lease={} error={:?}",
            c.outcome, c.lease, c.error
        ));
        if let Some(error) = &c.error {
            assert_credential_absent(error);
        }
        assert_eq!(c.outcome, Outcome::Ready);
        assert_credential_absent(&reply);
        assert_credential_absent(&arena);
        answers.push(format!(
            "refusal {:?} status={} reply={} fields={:?}",
            c.outcome,
            f.out.status,
            String::from_utf8_lossy(&reply[..f.out.reply_written as usize]),
            fields[..f.out.fields_written as usize]
                .iter()
                .map(|f| format!("{}={}", at(&arena, f.name), at(&arena, f.value)))
                .collect::<Vec<_>>()
        ));
        assert_eq!(first() - before, 1, "refusal: exact first crossings");
    }

    {
        let before = first();
        // The real request ticket lends the exact buffers until the dispatcher returns Done.
        let memory = Arc::new(StreamingPiece {
            buffers: std::cell::UnsafeCell::new(Bufs::with_units(512, 0)),
            bytes: Vec::new(),
        });
        let mut f: Frame<ServeIn, ServeOut> = Frame::new(z(), z());
        f.input.head = in_head();
        f.out.head = out_head();
        // SAFETY: no crossing exists yet; the owner is not inspected again until Done.
        let storage = unsafe { &mut *memory.buffers.get() };
        f.input.reply_buf = storage.reply.as_mut_ptr();
        f.input.reply_cap = storage.reply.len();
        f.input.arena_buf = storage.arena.as_mut_ptr();
        f.input.arena_cap = storage.arena.len();
        f.input.fields_buf = storage.fields.as_mut_ptr();
        f.input.fields_cap = 2;
        let ticket = d.mint(0).expect("actual SERVE request ticket");
        let done = d
            .submit_lent(
                p,
                ticket,
                slot::SERVE,
                f,
                busbar_contract::abi::mechanism::call::DeadlineClass::Call,
                busbar_plugin_loader::dispatch::now_ns().saturating_add(30_000_000_000),
                memory.clone(),
            )
            .wait_done();
        d.recycle(ticket);
        assert!(!done.detached);
        let f = done.frame.expect("completed SERVE frame");
        if let Some(error) = &done.error {
            assert_credential_absent(error);
        }
        // SAFETY: Done returned the joined frame, so output can now be inspected.
        let storage = unsafe { &*memory.buffers.get() };
        assert_credential_absent(&storage.reply);
        assert_credential_absent(&storage.arena);
        assert_ne!(done.outcome, Outcome::Fault);
        answers.push(format!(
            "serve {:?} lease={} error={:?} status={} reply_written={}",
            done.outcome, done.lease, done.error, f.out.status, f.out.reply_written
        ));
        assert_eq!(first() - before, 1, "serve: exact first crossings");
    }

    {
        use busbar_contract::abi::hook::SignalEntry;
        use busbar_contract::abi::plane::{ProjectIn, ProjectOut};
        let before = first();
        let mut arena = [0_u8; 256];
        let mut signals = [z::<SignalEntry>(); 4];
        let mut f: Frame<ProjectIn, ProjectOut> = Frame::new(z(), z());
        f.input.head = in_head();
        f.out.head = out_head();
        f.input.claim = 1;
        f.input.target = text("/v1/realtime/calls");
        f.input.body = octets(b"");
        f.input.signals_buf = signals.as_mut_ptr();
        f.input.signals_cap = signals.len();
        f.input.arena_buf = arena.as_mut_ptr();
        f.input.arena_cap = arena.len();
        let c = p.call(slot::PROJECT, &mut f);
        if let Some(error) = &c.error {
            assert_credential_absent(error);
        }
        assert_credential_absent(&arena);
        assert_ne!(c.outcome, Outcome::Fault);
        answers.push(format!(
            "project {:?} lease={} error={:?} signals={}",
            c.outcome, c.lease, c.error, f.out.view.signals_len
        ));
        assert_eq!(first() - before, 1, "project: exact first crossings");
    }

    {
        let before = first();
        let seen = p.inner.driven.answers();
        busbar_plugin_loader::dispatch::ticket::host_wake(p.inner.ctx(), driver);
        let outcome = p
            .inner
            .driven
            .answered_after(seen, std::time::Duration::from_secs(30))
            .expect("woken driver answers");
        assert_eq!(outcome, Outcome::Ready);
        let sessions = p.inner.driven.drain();
        answers.push(format!("drive {outcome:?} sessions={}", sessions.len()));
        assert_eq!(first() - before, 1, "drive: exact first crossings");
    }

    {
        let before = first();
        let mut f: Frame<busbar_contract::abi::mechanism::lifecycle::ReleaseIn, OutHead> =
            Frame::new(z(), out_head());
        f.input.head = in_head();
        let c = p.call(life::RELEASE, &mut f);
        call_answers.push(format!(
            "{:?} lease={} error={:?}",
            c.outcome, c.lease, c.error
        ));
        if let Some(error) = &c.error {
            assert_credential_absent(error);
        }
        assert!(
            c.outcome == Outcome::Ready
                || (c.outcome == Outcome::Refused
                    && c.error.as_deref() == Some(b"release: no such lease".as_slice()))
        );
        answers.push(format!("release none {:?} {:?}", c.outcome, c.error));
        assert_eq!(first() - before, 1, "release none: exact first crossings");
    }

    {
        let before = first();
        let done = d.tick(p, driver, 1000).wait_done();
        assert_eq!(done.outcome, Outcome::Ready);
        if let Some(error) = &done.error {
            assert_credential_absent(error);
        }
        call_answers.push(format!(
            "{:?} lease={} error={:?}",
            done.outcome, done.lease, done.error
        ));
        let next = done.frame.as_ref().map_or(0, |f| f.out.next_tick_ns);
        answers.push(format!("tick {:?} next={next}", done.outcome));
        assert_eq!(first() - before, 1, "tick: exact first crossings");
    }

    {
        let before = first();
        let mut f: Frame<
            busbar_contract::abi::plane::PlaneCancelIn,
            busbar_contract::abi::plane::PlaneCancelOut,
        > = Frame::new(z(), z());
        f.input.cancel.head = in_head();
        f.out.cancel.head = out_head();
        let c = p.call(life::CANCEL, &mut f);
        if let Some(error) = &c.error {
            assert_credential_absent(error);
        }
        assert_eq!(c.outcome, Outcome::Ready);
        answers.push(format!(
            "cancel {:?} lease={} error={:?} disposition={}",
            c.outcome, c.lease, c.error, f.out.cancel.disposition
        ));
        assert_eq!(first() - before, 1, "cancel: exact first crossings");
    }

    {
        let before = first();
        let mut f: Frame<RefreshIn, PlaneRefreshOut> = Frame::new(z(), z());
        f.input.head = in_head();
        f.input.generation = 2;
        f.input.settings = json(br#"{"session":{"model":"m-cap"},"session_max_secs":1800}"#);
        f.out.head = out_head();
        let c = p.call(life::REFRESH, &mut f);
        call_answers.push(format!(
            "{:?} lease={} error={:?}",
            c.outcome, c.lease, c.error
        ));
        if let Some(error) = &c.error {
            assert_credential_absent(error);
        }
        assert_eq!(c.outcome, Outcome::Ready);
        let snap = snapshot(f.out.snapshot);
        assert_eq!(format!("refresh {:?} {snap}", c.outcome), expected(true)[7]);
        answers.push(format!("refresh {:?} {snap}", c.outcome));
        assert_eq!(first() - before, 1, "refresh: exact first crossings");
    }

    {
        let before = first();
        let mut f = Frame::new(
            GenIn {
                head: in_head(),
                generation: 1,
            },
            out_head(),
        );
        let c = p.call(life::RETIRE, &mut f);
        call_answers.push(format!(
            "{:?} lease={} error={:?}",
            c.outcome, c.lease, c.error
        ));
        if let Some(error) = &c.error {
            assert_credential_absent(error);
        }
        assert_eq!(c.outcome, Outcome::Ready);
        answers.push(format!("retire {:?}", c.outcome));
        assert_eq!(first() - before, 1, "retire: exact first crossings");
    }

    {
        let before = first();
        let mut f = Frame::new(in_head(), out_head());
        let c = p.call(life::CLOSE, &mut f);
        call_answers.push(format!(
            "{:?} lease={} error={:?}",
            c.outcome, c.lease, c.error
        ));
        if let Some(error) = &c.error {
            assert_credential_absent(error);
        }
        assert_eq!(c.outcome, Outcome::Ready);
        answers.push(format!("close {:?}", c.outcome));
        assert_eq!(first() - before, 1, "close: exact first crossings");
    }

    {
        let before = first();
        let got = arrival(p, 1, 7, "/v1/realtime/calls");
        assert_eq!(got.0, Outcome::Fault);
        answers.push(format!("arrive after close Fault {}", got.2));
        assert_eq!(
            first() - before,
            0,
            "arrive after close: exact first crossings"
        );
    }
    d.recycle(driver);
    answers.extend(call_answers);
    for line in &answers {
        assert_credential_absent(line.as_bytes());
    }
    answers
}
#[test]
fn published_streaming_cases_keep_exact_crossings_and_both_actual_deliveries() {
    assert_credential_absent(&row().statement);
    assert_credential_absent(
        &busbar_plugin_loader::dispatch::rendering_of_library(&plugin_cdylib())
            .unwrap()
            .expect("actual streaming image renders"),
    );
    let mut deliveries = Vec::new();
    for dropped_in in [false, true] {
        let d = Dispatcher::new(DispatchConfig::default());
        let log = Arc::new(HostLog::default());
        let bound = Bind {
            sink: log.clone(),
            ..bind(
                &d,
                if dropped_in {
                    "dropped-private"
                } else {
                    "linked-private"
                },
            )
        };
        let p = if dropped_in {
            dropped_bound(bound)
        } else {
            load_linked::<Plane>(&row(), bound).unwrap()
        };
        let before_resumes = p.inner.resumes.load(Ordering::SeqCst);
        deliveries.push(cases(&p, &d));
        eprintln!(
            "actual streaming dropped={dropped_in} resumes={}",
            p.inner.resumes.load(Ordering::SeqCst) - before_resumes
        );
        assert!(
            !log.2.load(Ordering::SeqCst),
            "all envelope bytes must be captured"
        );
        for record in log.1.lock().unwrap().iter() {
            assert_credential_absent(record);
        }
    }
    let [a, b]: [Vec<String>; 2] = deliveries.try_into().unwrap();
    assert_eq!(
        a, b,
        "streaming's pinned lifecycle and session cases match through both actual doors"
    );
}
