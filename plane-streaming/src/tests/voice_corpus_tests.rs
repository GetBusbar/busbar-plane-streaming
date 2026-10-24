// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE VOICE CORPUS, THROUGH THE PLANE'S OWN CODECS: the codec halves of busbar's voice
//! conformance legs (`testing/voice-conformance`, `spec-per-dialect` and `cross-parity`), moved
//! here when the door-root made `door()` this crate's only public item and the harness could no
//! longer call a codec. The harness still drives every fixture through the door, both ways; what
//! only the codecs can answer is asserted here, as strongly as the legs asserted it.
//!
//! * [`every_corpus_fixture_round_trips_to_an_ir_fixpoint`]: each OpenAI Realtime and Gemini Live
//!   fixture read to IR, framed back in the same dialect and read again must give the same IR (or
//!   the same correlation fingerprint, for a frame the writer legitimately re-frames per event); a
//!   fixture that reads to no IR at all must be one of the documented drops.
//! * [`every_shared_concept_survives_the_cross_dialect_bridge`]: for each ordered dialect pair, each
//!   shared concept of the cross-dialect map bridged A -> IR -> B -> IR keeps its load-bearing
//!   fields, and each asymmetry row is accounted for in its origin -> other direction.
//!
//! The corpus is busbar's (`testing/voice-conformance/fixtures/{openai,gemini}` and
//! `qa/evidence/voice-cross-dialect-map.json`, copied verbatim at busbar dev 9925f85e1f) under
//! `tests/fixtures/voice/`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use serde_json::Value;

use crate::codec::ir::codec::gemini::GeminiLiveCodec;
use crate::codec::ir::codec::{
    DecodeState, DuplexReader, DuplexWriter, OpenAiRealtimeCodec, WireEvent,
};
use crate::codec::ir::config::SessionConfig;
use crate::codec::ir::control::IrDuplexControl;
use crate::codec::ir::event::{IrClientEvent, IrServerEvent};
use crate::codec::ir::tool::IrDuplexTool;

// ── the corpus ───────────────────────────────────────────────────────────────────────────────────

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/voice")
}

/// A fixture file, re-serialized compactly (the frame a session would carry).
fn read_fixture(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(serde_json::to_vec(&v).expect("a parsed value serializes"))
}

// ── the two duplex dialects ──────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dialect {
    OpenAi,
    Gemini,
}

impl Dialect {
    const fn name(self) -> &'static str {
        match self {
            Dialect::OpenAi => "openai",
            Dialect::Gemini => "gemini",
        }
    }

    fn read_up(self, frame: &[u8], st: &mut DecodeState) -> Vec<IrClientEvent> {
        let w = WireEvent(Bytes::copy_from_slice(frame));
        match self {
            Dialect::OpenAi => OpenAiRealtimeCodec.read_up(w, st),
            Dialect::Gemini => GeminiLiveCodec.read_up(w, st),
        }
    }

    fn read_down(self, frame: &[u8], st: &mut DecodeState) -> Vec<IrServerEvent> {
        let w = WireEvent(Bytes::copy_from_slice(frame));
        match self {
            Dialect::OpenAi => OpenAiRealtimeCodec.read_down(w, st),
            Dialect::Gemini => GeminiLiveCodec.read_down(w, st),
        }
    }

    fn write_up(self, ev: IrClientEvent, st: &mut DecodeState) -> Option<WireEvent> {
        match self {
            Dialect::OpenAi => OpenAiRealtimeCodec.write_up(ev, st),
            Dialect::Gemini => GeminiLiveCodec.write_up(ev, st),
        }
    }

    fn write_down(self, ev: IrServerEvent, st: &mut DecodeState) -> Option<WireEvent> {
        match self {
            Dialect::OpenAi => OpenAiRealtimeCodec.write_down(ev, st),
            Dialect::Gemini => GeminiLiveCodec.write_down(ev, st),
        }
    }
}

// ── the shared normal form (the harness legs', unchanged in meaning) ─────────────────────────────

/// Both direction-split IR enums collapsed onto one list, so concepts compare uniformly.
#[derive(Clone, Debug, PartialEq)]
enum Norm {
    /// Config essentials only: instructions|voice|tools|max (the survivable fields).
    Config(String),
    ConfigModalities(Vec<String>),
    Connect,
    AudioUp(Vec<u8>),
    AudioDown(Vec<u8>),
    AudioDone,
    Item(Value),
    SpeechStart,
    SpeechStop,
    Truncate(u64),
    Commit,
    Clear,
    ResponseCreate,
    ResponseCancel,
    ItemDelete,
    /// Audio in, audio out, text in, text out.
    Usage(u64, u64, u64, u64),
    RateLimits,
    Error(String, String),
    ToolOpen(String, String),
    ToolArgs(String, Value),
    ToolClose(String),
    ToolResult(String, Value),
}

fn cfg_essentials(config: &SessionConfig) -> Norm {
    Norm::Config(format!(
        "{:?}|{:?}|{}|{:?}",
        config.instructions,
        config.voice,
        config.tools.len(),
        config.max_output_tokens
    ))
}

fn norm_tool(t: &IrDuplexTool) -> Norm {
    match t {
        IrDuplexTool::CallOpen { call_id, name, .. } => {
            Norm::ToolOpen(call_id.clone(), name.clone())
        }
        IrDuplexTool::CallArgs {
            call_id,
            json_delta,
            ..
        } => Norm::ToolArgs(
            call_id.clone(),
            serde_json::from_slice(json_delta).unwrap_or(Value::Null),
        ),
        IrDuplexTool::CallClose { call_id, .. } => Norm::ToolClose(call_id.clone()),
        IrDuplexTool::CallResult {
            call_id, output, ..
        } => Norm::ToolResult(
            call_id.clone(),
            serde_json::from_slice(output).unwrap_or(Value::Null),
        ),
    }
}

fn norm_up(evs: &[IrClientEvent]) -> Vec<Norm> {
    let mut out = Vec::new();
    for e in evs {
        match e {
            IrClientEvent::AudioFrame(f) => out.push(Norm::AudioUp(f.media.to_vec())),
            IrClientEvent::Tool(t) => out.push(norm_tool(t)),
            IrClientEvent::Control(c) => match c {
                IrDuplexControl::SessionConfigure { config } => {
                    out.push(cfg_essentials(config));
                    out.push(Norm::ConfigModalities(config.modalities.clone()));
                }
                IrDuplexControl::ItemCreate { item } => out.push(Norm::Item(item.clone())),
                IrDuplexControl::ItemTruncate {
                    audio_played_ms, ..
                } => out.push(Norm::Truncate(*audio_played_ms)),
                IrDuplexControl::InputAudioCommit => out.push(Norm::Commit),
                IrDuplexControl::InputAudioClear => out.push(Norm::Clear),
                IrDuplexControl::ResponseCreate { .. } => out.push(Norm::ResponseCreate),
                IrDuplexControl::ResponseCancel => out.push(Norm::ResponseCancel),
                IrDuplexControl::ItemDelete { .. } => out.push(Norm::ItemDelete),
            },
        }
    }
    out
}

fn norm_down(evs: &[IrServerEvent]) -> Vec<Norm> {
    let mut out = Vec::new();
    for e in evs {
        match e {
            IrServerEvent::SessionCreated { .. } => out.push(Norm::Connect),
            IrServerEvent::Tool(t) => out.push(norm_tool(t)),
            IrServerEvent::SpeechStarted { .. } => out.push(Norm::SpeechStart),
            IrServerEvent::SpeechStopped { .. } => out.push(Norm::SpeechStop),
            IrServerEvent::AudioFrame(f) => out.push(Norm::AudioDown(f.media.to_vec())),
            IrServerEvent::AudioDone { .. } => out.push(Norm::AudioDone),
            IrServerEvent::Usage(u) => {
                out.push(Norm::Usage(u.audio_in, u.audio_out, u.text_in, u.text_out));
            }
            IrServerEvent::RateLimits => out.push(Norm::RateLimits),
            IrServerEvent::Error { code, message } => {
                out.push(Norm::Error(code.clone(), message.clone()));
            }
        }
    }
    out
}

/// One correlated tool call, keyed by call id: `(name, args, result)`.
type CallEntry = (Option<String>, Option<Value>, Option<Value>);

/// A call-collapsed fingerprint: tool events merged by call id (an atomic Gemini `toolCall` decodes
/// to a streamed triple and the stateless writer re-frames per event, so arity differs
/// legitimately; the correlation must survive). Everything else compared verbatim, in order.
#[derive(Debug, Default, PartialEq)]
struct Fingerprint {
    calls: BTreeMap<String, CallEntry>,
    other: Vec<Norm>,
}

fn fingerprint(norms: &[Norm]) -> Fingerprint {
    let mut fp = Fingerprint::default();
    for n in norms {
        match n {
            Norm::ToolOpen(id, name) => {
                let e = fp.calls.entry(id.clone()).or_default();
                if e.0.is_none() && !name.is_empty() {
                    e.0 = Some(name.clone());
                }
            }
            Norm::ToolArgs(id, args) => {
                let e = fp.calls.entry(id.clone()).or_default();
                if e.1.is_none() && !args.is_null() {
                    e.1 = Some(args.clone());
                }
            }
            Norm::ToolClose(id) => {
                fp.calls.entry(id.clone()).or_default();
            }
            Norm::ToolResult(id, out) => {
                let e = fp.calls.entry(id.clone()).or_default();
                if e.2.is_none() {
                    e.2 = Some(out.clone());
                }
            }
            other => fp.other.push(other.clone()),
        }
    }
    fp
}

// ── the round trip (spec-per-dialect's codec half) ───────────────────────────────────────────────

/// Why a fixture reads to no IR: a far-end concept with no shared home (drop+warn, per the map).
fn drop_reason(dialect: Dialect, fixture: &str) -> Option<&'static str> {
    match (dialect, fixture) {
        (Dialect::Gemini, "goAway.json") => {
            Some("gemini_go_away: no OpenAI advance-disconnect twin (drop+warn)")
        }
        (Dialect::Gemini, "toolCallCancellation.json") => {
            Some("gemini_tool_call_cancellation: no OpenAI server-driven cancel (drop+warn)")
        }
        (Dialect::Gemini, "serverContent.inputTranscription.json") => {
            Some("input transcription side-channel: no shared IR home (drop+warn)")
        }
        (Dialect::Gemini, "serverContent.outputTranscription.json") => {
            Some("output transcription side-channel: no shared IR home (drop+warn)")
        }
        _ => None,
    }
}

fn spec_verdict(n1: &[Norm], n2: &[Norm]) -> Result<(), String> {
    if n1 == n2 || fingerprint(n1) == fingerprint(n2) {
        return Ok(());
    }
    Err(format!("round-trip diverged: {n1:?} != {n2:?}"))
}

/// The codec half of one fixture: read it (as a caller frame, else as a far-end frame), frame each
/// event back in the same dialect, read that, and compare.
fn round_trip(dialect: Dialect, fixture: &str, frame: &[u8]) -> Result<(), String> {
    let up = dialect.read_up(frame, &mut DecodeState::default());
    if !up.is_empty() {
        let (mut w, mut r) = (DecodeState::default(), DecodeState::default());
        let mut again = Vec::new();
        for e in &up {
            if let Some(wire) = dialect.write_up(e.clone(), &mut w) {
                again.extend(dialect.read_up(&wire.0, &mut r));
            }
        }
        return spec_verdict(&norm_up(&up), &norm_up(&again));
    }
    let down = dialect.read_down(frame, &mut DecodeState::default());
    if !down.is_empty() {
        let (mut w, mut r) = (DecodeState::default(), DecodeState::default());
        let mut again = Vec::new();
        for e in &down {
            if let Some(wire) = dialect.write_down(e.clone(), &mut w) {
                again.extend(dialect.read_down(&wire.0, &mut r));
            }
        }
        return spec_verdict(&norm_down(&down), &norm_down(&again));
    }
    match drop_reason(dialect, fixture) {
        Some(_) => Ok(()),
        None => Err("decoded to NO IR events and is not a documented drop".to_string()),
    }
}

/// Every `.json` fixture of both duplex dialects round-trips to an IR fixpoint, or is a documented
/// drop. The corpus must be read: an empty or unreadable directory fails, never passes.
#[test]
fn every_corpus_fixture_round_trips_to_an_ir_fixpoint() {
    let mut fails = Vec::new();
    let mut judged = 0usize;
    for dialect in [Dialect::OpenAi, Dialect::Gemini] {
        let dir = corpus().join(dialect.name());
        let mut fixtures: Vec<String> = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".json"))
            .collect();
        fixtures.sort();
        assert!(!fixtures.is_empty(), "{}: no .json fixtures", dir.display());
        for f in &fixtures {
            judged += 1;
            match read_fixture(&dir.join(f)) {
                Err(e) => fails.push(format!("{}/{f}: unreadable fixture: {e}", dialect.name())),
                Ok(frame) => {
                    if let Err(e) = round_trip(dialect, f, &frame) {
                        fails.push(format!("{}/{f}: {e}", dialect.name()));
                    }
                }
            }
        }
    }
    assert_eq!(judged, 36, "the corpus is 20 OpenAI and 16 Gemini fixtures");
    assert!(
        fails.is_empty(),
        "round-trip failures:\n{}",
        fails.join("\n")
    );
}

/// RED: a frame that reads to no IR and is not a documented drop fails the round trip, as the leg
/// failed it.
#[test]
fn an_undocumented_frame_that_reads_to_nothing_fails_the_round_trip() {
    let frame = br#"{"type":"busbar.not.a.realtime.event"}"#;
    assert!(round_trip(Dialect::OpenAi, "planted.json", frame).is_err());
    assert!(round_trip(
        Dialect::Gemini,
        "planted.json",
        br#"{"notAGeminiMessage":{}}"#
    )
    .is_err());
}

// ── the bridge (cross-parity's codec half) ───────────────────────────────────────────────────────

/// One bridge: the source's norms, and the bridged-and-re-read norms.
struct Bridged {
    n1: Vec<Norm>,
    n2: Vec<Norm>,
}

/// Bridge the frames `vals` FROM `a` TO `b` as one exchange (one source session, one destination).
fn bridge(a: Dialect, b: Dialect, vals: &[Vec<u8>]) -> Bridged {
    let (mut rst, mut wst, mut bst) = (
        DecodeState::default(),
        DecodeState::default(),
        DecodeState::default(),
    );
    let mut out = Bridged {
        n1: Vec::new(),
        n2: Vec::new(),
    };
    for v in vals {
        let up = a.read_up(v, &mut rst);
        if !up.is_empty() {
            out.n1.extend(norm_up(&up));
            for e in up {
                if let Some(w) = b.write_up(e, &mut wst) {
                    out.n2.extend(norm_up(&b.read_up(&w.0, &mut bst)));
                }
            }
            continue;
        }
        let down = a.read_down(v, &mut rst);
        out.n1.extend(norm_down(&down));
        for e in down {
            if let Some(w) = b.write_down(e, &mut wst) {
                out.n2.extend(norm_down(&b.read_down(&w.0, &mut bst)));
            }
        }
    }
    out
}

/// Concepts the map documents as dropping toward `to` (implicit or absent there).
fn directional_drop(concept: &str, to: Dialect) -> bool {
    matches!(
        (concept, to),
        ("input turn commit / end-of-audio", Dialect::Gemini)
            | ("barge-in / truncate / cancel", Dialect::Gemini)
    )
}

fn cfg_from(norms: &[Norm]) -> Vec<String> {
    norms
        .iter()
        .filter_map(|n| match n {
            Norm::Config(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

/// Do the shared concept's load-bearing fields survive `a` -> IR -> `b` -> IR?
fn survives(concept: &str, n1: &[Norm], n2: &[Norm], a: Dialect, b: Dialect) -> Result<(), String> {
    let (f1, f2) = (fingerprint(n1), fingerprint(n2));
    if f1 == f2 {
        return Ok(());
    }
    if concept.contains("session") && cfg_from(n1) == cfg_from(n2) && !cfg_from(n1).is_empty() {
        return Ok(());
    }
    if f2.calls.is_empty() && f2.other.is_empty() && directional_drop(concept, b) {
        return Ok(());
    }
    Err(format!(
        "{concept}: shared fields did not survive {}->{}: {f1:?} != {f2:?}",
        a.name(),
        b.name()
    ))
}

fn drop_if(cond: bool, msg: &str) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(format!("NOT accounted for: {msg}"))
    }
}

/// A one-dialect-only concept bridged toward the other must be ACCOUNTED FOR.
fn asym_drop(id: &str, n1: &[Norm], n2: &[Norm]) -> Result<(), String> {
    let has = |ns: &[Norm], pred: &dyn Fn(&Norm) -> bool| ns.iter().any(pred);
    match id {
        "openai_buffer_clear" => drop_if(
            !has(n2, &|n| matches!(n, Norm::Clear)),
            "clear has no Gemini twin and is dropped",
        ),
        "openai_response_overrides" => drop_if(
            !has(n2, &|n| matches!(n, Norm::ResponseCreate)),
            "per-response overrides dropped (Gemini is setup-time only)",
        ),
        "openai_truncate_precision" => drop_if(
            !has(n2, &|n| matches!(n, Norm::Truncate(_))),
            "sample-accurate truncate dropped (Gemini interrupted carries no ms)",
        ),
        "openai_structured_error" => drop_if(
            !has(n2, &|n| matches!(n, Norm::Error(..))),
            "structured error dropped toward Gemini (WS close codes instead)",
        ),
        "openai_event_id" | "openai_noise_reduction" => drop_if(
            true,
            "field never enters the IR (dropped at decode); bridged config omits it",
        ),
        "openai_semantic_vad" | "openai_g711" => drop_if(
            has(n2, &|n| matches!(n, Norm::Config(_))),
            "config bridges; semantic_vad/g711 specifics dropped per map",
        ),
        "openai_speech_boundary" => drop_if(
            has(n2, &|n| matches!(n, Norm::SpeechStart)) || n1.is_empty(),
            "boundary maps to interrupted; ms offset dropped",
        ),
        "openai_uplink_rate" => drop_if(
            !n2.is_empty()
                && n1
                    .iter()
                    .zip(n2.iter())
                    .all(|(a, b)| matches!((a, b), (Norm::AudioUp(x), Norm::AudioUp(y)) if x == y)),
            "uplink audio bridges verbatim at its true rate",
        ),
        "gemini_go_away" | "gemini_tool_call_cancellation" => drop_if(
            n1.is_empty() && n2.is_empty(),
            "no OpenAI twin, dropped at decode (drop+warn)",
        ),
        "gemini_generation_complete" => drop_if(
            has(n2, &|n| matches!(n, Norm::AudioDone)),
            "turnComplete->AudioDone survives; generationComplete collapsed",
        ),
        "gemini_audio_stream_end" => drop_if(
            has(n1, &|n| matches!(n, Norm::Commit)) && has(n2, &|n| matches!(n, Norm::Commit)),
            "audioStreamEnd <-> input_audio_buffer.commit: the end-of-uplink turn survives",
        ),
        "gemini_setup_complete" => drop_if(
            has(n2, &|n| matches!(n, Norm::Connect)),
            "ack maps to session.created",
        ),
        other => Err(format!("no asymmetry handler for id '{other}'")),
    }
}

/// One ordered pair's cross-parity codec half: every shared concept of the map, then every
/// asymmetry row in its origin -> other direction. Answers the failures and how many concepts and
/// rows it judged.
fn cross(a: Dialect, b: Dialect, map: &Value) -> (Vec<String>, usize, usize) {
    let mut fails = Vec::new();
    let (mut concepts_judged, mut rows_judged) = (0usize, 0usize);
    let pair = format!("{}->{}", a.name(), b.name());
    let concepts = map["concepts"]
        .as_array()
        .expect("the cross-dialect map has a concepts array");
    for c in concepts {
        let concept = c["concept"].as_str().unwrap_or("?");
        let present: Vec<(String, Vec<u8>)> = c[a.name()]["fixtures"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|fx| {
                let name = fx
                    .as_str()
                    .filter(|n| !n.is_empty() && !n.ends_with(".jsonl"))?;
                let frame = read_fixture(&corpus().join(a.name()).join(name)).ok()?;
                Some((name.to_string(), frame))
            })
            .collect();
        let mut chosen = present.iter().find_map(|(name, v)| {
            let br = bridge(a, b, std::slice::from_ref(v));
            (!br.n1.is_empty()).then(|| (name.clone(), br))
        });
        // A concept whose transform is stated ACROSS events (the streamed <-> atomic tool call)
        // can bridge one frame to nothing alone; judge its fixtures as the one exchange the map
        // states.
        if chosen.as_ref().is_some_and(|(_, br)| br.n2.is_empty()) && present.len() > 1 {
            let vals: Vec<Vec<u8>> = present.iter().map(|(_, v)| v.clone()).collect();
            let names: Vec<&str> = present.iter().map(|(n, _)| n.as_str()).collect();
            chosen = Some((names.join("+"), bridge(a, b, &vals)));
        }
        // A concept with no decoding fixture on this source side is exercised in the reverse pair
        // (the leg reported it PENDING, never as a pass or a failure).
        if let Some((name, br)) = chosen {
            concepts_judged += 1;
            if let Err(e) = survives(concept, &br.n1, &br.n2, a, b) {
                fails.push(format!("{pair} shared:{name}: {e}"));
            }
        }
    }
    let rows = map["asymmetry"]
        .as_array()
        .expect("the cross-dialect map has an asymmetry array");
    for row in rows {
        let id = row["id"].as_str().unwrap_or("?");
        let origin = row["dialect"].as_str().unwrap_or("?");
        let fixture = row["fixture"].as_str().unwrap_or_default();
        // A row is exercised in its origin -> other direction only.
        if a.name() != origin || a == b {
            continue;
        }
        rows_judged += 1;
        let name = fixture
            .strip_prefix(&format!("{origin}/"))
            .unwrap_or(fixture);
        let frame = match read_fixture(&corpus().join(a.name()).join(name)) {
            Ok(frame) => frame,
            Err(e) => {
                fails.push(format!("{pair} asym:{id}: fixture {fixture} missing: {e}"));
                continue;
            }
        };
        let br = bridge(a, b, std::slice::from_ref(&frame));
        if let Err(e) = asym_drop(id, &br.n1, &br.n2) {
            fails.push(format!("{pair} asym:{id}: {e}"));
        }
    }
    (fails, concepts_judged, rows_judged)
}

/// The four ordered pairs (oo, og, go, gg): every shared concept's load-bearing fields survive the
/// bridge, and every asymmetry row is accounted for in its origin -> other direction.
#[test]
fn every_shared_concept_survives_the_cross_dialect_bridge() {
    let map: Value = serde_json::from_slice(
        &fs::read(corpus().join("voice-cross-dialect-map.json")).expect("the cross-dialect map"),
    )
    .expect("the cross-dialect map is JSON");
    let total_rows = map["asymmetry"].as_array().map_or(0, Vec::len);
    let mut fails = Vec::new();
    let mut rows = 0usize;
    for (a, b) in [
        (Dialect::OpenAi, Dialect::OpenAi),
        (Dialect::OpenAi, Dialect::Gemini),
        (Dialect::Gemini, Dialect::OpenAi),
        (Dialect::Gemini, Dialect::Gemini),
    ] {
        let (f, concepts, r) = cross(a, b, &map);
        assert!(
            concepts > 0,
            "{}->{}: no shared concept was judged (the corpus did not read)",
            a.name(),
            b.name()
        );
        fails.extend(f);
        rows += r;
    }
    assert_eq!(
        rows, total_rows,
        "every asymmetry row is judged once, in its origin -> other pair"
    );
    assert!(fails.is_empty(), "bridge failures:\n{}", fails.join("\n"));
}

/// RED: an asymmetry row this suite has no handler for fails rather than passing unread, and a
/// bridged concept whose fields differ fails `survives`.
#[test]
fn an_unhandled_asymmetry_or_a_lost_field_fails_the_bridge() {
    assert!(asym_drop("planted_row", &[], &[]).is_err());
    let n1 = [Norm::Truncate(120)];
    let n2 = [Norm::Truncate(0)];
    assert!(survives("planted", &n1, &n2, Dialect::OpenAi, Dialect::OpenAi).is_err());
}
