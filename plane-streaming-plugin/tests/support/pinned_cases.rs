// SPDX-License-Identifier: Apache-2.0
//! Ordinary streaming-specific cases formerly driven by the published plane script.
use super::*;
use busbar_contract::abi::mechanism::call::OutHead;
use busbar_contract::abi::plane::{RefusalIn, RefusalOut, ServeIn, ServeOut};
use std::sync::atomic::Ordering;
fn arrival(p: &Plugin<Plane>, claim: u32, unit: u64, target: &'static str) -> (Outcome, u32) {
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
        assert!(
            !String::from_utf8_lossy(error).contains("bb-canary-streaming-5c1e7a"),
            "arrival leaked credential"
        );
    }
    assert!(
        !read(f.out.pool).contains("bb-canary-streaming-5c1e7a"),
        "arrival route leaked credential"
    );
    (c.outcome, f.out.refusal_status)
}
fn cases(p: &Plugin<Plane>, d: &Dispatcher) -> Vec<String> {
    let first =
        || p.inner.crossings.load(Ordering::SeqCst) - p.inner.resumes.load(Ordering::SeqCst);
    let mut answers = Vec::new();

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
        assert_eq!(
            validate(p, br#"{"streams":{"session":{"model":"m-cap"}}}"#),
            Outcome::Ready
        );
        answers.push("validate Ready".into());
        assert_eq!(first() - before, 1, "validate: exact first crossings");
    }

    {
        let before = first();
        assert_eq!(arrival(p, 1, 7, "/v1/realtime/calls").0, Outcome::Refused);
        answers.push("arrive unopened Refused".into());
        assert_eq!(
            first() - before,
            0,
            "arrive unopened: exact first crossings"
        );
    }

    {
        let before = first();
        let (o, snap) = open(p, 1, SESSION, Some(PUBLIC));
        assert_eq!(o, Outcome::Ready);
        assert_eq!(format!("open {o:?} {snap}"), expected(true)[6]);
        answers.push(format!("open {o:?} {snap}"));
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
        assert_eq!(open(p, 1, SESSION, Some(PUBLIC)).0, Outcome::Refused);
        answers.push("open again Refused".into());
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
        assert_eq!(c.outcome, Outcome::Ready);
        answers.push(format!("start {:?}", c.outcome));
        assert_eq!(first() - before, 1, "start: exact first crossings");
    }

    {
        let before = first();
        assert_eq!(arrival(p, 1, 7, "/v1/realtime/calls").0, Outcome::Ready);
        answers.push("arrive claimed Ready".into());
        assert_eq!(first() - before, 1, "arrive claimed: exact first crossings");
    }

    {
        let before = first();
        assert_eq!(arrival(p, 9, 8, "/v1/unpublished"), (Outcome::Refused, 404));
        answers.push("arrive unclaimed Refused status=404".into());
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
        assert_eq!(
            arrival(p, 5, 20, "/v1/realtime?model=m-cap").0,
            Outcome::Ready
        );
        answers.push("live arrive Ready".into());
        assert_eq!(first() - before, 1, "live arrive: exact first crossings");
    }
    let mut b = Bufs::with_units(262144, 16);
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
        let answer = b.call(p, &g);
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
    let mut gemini = Bufs::with_units(65536, 16);
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
        let answer = gemini.call(p, &g);
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
        assert_eq!(c.outcome, Outcome::Ready);
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
        let mut f: Frame<ServeIn, ServeOut> = Frame::new(z(), z());
        let mut reply = [0u8; 512];
        let mut arena = [0u8; 256];
        let mut fields = [z::<OutField>(); 2];
        f.input.head = in_head();
        f.out.head = out_head();
        f.input.reply_buf = reply.as_mut_ptr();
        f.input.reply_cap = reply.len();
        f.input.arena_buf = arena.as_mut_ptr();
        f.input.arena_cap = arena.len();
        f.input.fields_buf = fields.as_mut_ptr();
        f.input.fields_cap = fields.len();
        let c = p.call(slot::SERVE, &mut f);
        assert_ne!(c.outcome, Outcome::Fault);
        answers.push(format!(
            "serve {:?} status={} reply_written={}",
            c.outcome, f.out.status, f.out.reply_written
        ));
        assert_eq!(first() - before, 1, "serve: exact first crossings");
    }

    {
        let before = first();
        let answer = project_at(p, 1, 7, b"", 4096);
        assert_ne!(answer.0, Outcome::Fault);
        answers.push(format!("project {answer:?}"));
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
        let next = done.frame.as_ref().map_or(0, |f| f.out.next_tick_ns);
        answers.push(format!("tick {:?} next={next}", done.outcome));
        assert_eq!(first() - before, 1, "tick: exact first crossings");
    }

    {
        let before = first();
        let outcome = cancel_unit(p, 0);
        assert_eq!(outcome, Outcome::Ready);
        answers.push(format!("cancel {outcome:?}"));
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
        assert_eq!(c.outcome, Outcome::Ready);
        answers.push(format!("retire {:?}", c.outcome));
        assert_eq!(first() - before, 1, "retire: exact first crossings");
    }

    {
        let before = first();
        let mut f = Frame::new(in_head(), out_head());
        let c = p.call(life::CLOSE, &mut f);
        assert_eq!(c.outcome, Outcome::Ready);
        answers.push(format!("close {:?}", c.outcome));
        assert_eq!(first() - before, 1, "close: exact first crossings");
    }

    {
        let before = first();
        assert_eq!(arrival(p, 1, 7, "/v1/realtime/calls").0, Outcome::Fault);
        answers.push("arrive after close Fault".into());
        assert_eq!(
            first() - before,
            0,
            "arrive after close: exact first crossings"
        );
    }
    d.recycle(driver);
    for line in &answers {
        assert!(
            !line.contains("bb-canary-streaming-5c1e7a"),
            "configured credential escaped: {line}"
        );
    }
    answers
}
#[test]
fn published_streaming_cases_keep_exact_crossings_and_both_actual_deliveries() {
    let d = Dispatcher::new(DispatchConfig::default());
    let a = cases(&linked(&d), &d);
    let d = Dispatcher::new(DispatchConfig::default());
    let b = cases(&dropped(&d), &d);
    assert_eq!(
        a, b,
        "streaming's pinned lifecycle and session cases match through both actual doors"
    );
}
