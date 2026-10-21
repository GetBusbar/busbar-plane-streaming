// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE GOVERNED SPLICE: the one edit a same-dialect relay makes to a caller's bytes.
//!
//! Law 5: "A RELAY (same dialect) runs the reader only as that read-only tap and sends the client's
//! bytes with governed byte-level splices; a WRITER runs only when translating." The governance a
//! live session splices is Part 2 #45's browser trust boundary: the caller "never authors tools or
//! instructions, and cannot override the session's locked instructions". So a caller frame that
//! configures the session, or asks for one response under its own settings, has its instructions
//! and tool set cut out and the session's locked keys put in their place; every other member it
//! wrote passes as the caller wrote it, byte for byte, and so does every frame that configures
//! nothing.
//!
//! The edit is made on the frame's own bytes: one object member is rebuilt from the members the
//! caller wrote (their bytes unchanged, in their order) and the locked members after them, and the
//! bytes around that object are copied as they arrived. Keys are compared as the far end reads
//! them, escapes decoded, and a key the caller wrote twice is cut every time it appears.

use serde_json::{Map, Value};

use crate::codec::ir::codec::gemini::{setup_locked, GeminiLiveCodec};
use crate::codec::ir::codec::OpenAiRealtimeCodec;
use crate::codec::ir::config::{MaxOutputTokens, SessionConfig};
use crate::codec::ir::media::AudioFormat;

/// Where in a dialect's caller frame the governed object sits, and what is done to it.
#[derive(Debug)]
pub struct Rule {
    /// The top-level member holding the governed object.
    pub at: &'static str,
    /// The keys a caller never authors there.
    pub cut: &'static [&'static str],
    /// The locked members put in, each over the caller's own (an object over an object is merged
    /// member by member).
    pub set: Map<String, Value>,
    /// The far end reads the object as protobuf JSON, which takes a field under its original
    /// snake_case name as well as its lowerCamelCase JSON name (`system_instruction` is
    /// `systemInstruction`): every key is compared under the lowerCamelCase name, so each spelling
    /// of a governed field is governed.
    pub proto_names: bool,
    /// The numeric members capped at the session's ceiling ([`Cap`]).
    pub caps: Vec<Cap>,
}

/// A numeric member a rule caps: written once, as the lower of the caller's own value and
/// `ceiling`, and as `ceiling` where the caller wrote one that is not a whole number (`"inf"`) or
/// one above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cap {
    /// The objects, from the governed one down, the member sits in (`[]`: the governed object's
    /// own member).
    pub path: &'static [&'static str],
    /// The member's key.
    pub key: &'static str,
    /// The ceiling.
    pub ceiling: u64,
    /// Whether the member is written at `ceiling` where the caller wrote none; otherwise a frame
    /// that does not name the key is left without it.
    pub when_absent: bool,
}

/// THE DIALECT'S GOVERNED KEYS: which caller frames a session's lock splices, and how. Every rule
/// is applied to EVERY caller frame, by the frame's bytes: a governed object is governed wherever
/// it sits at the top level, whatever the frame's `type` says (a far end may read a `type` the
/// caller wrote twice differently from this reader).
pub trait LockedKeys {
    /// The rule for the object that configures the session (`session.update`'s `session`, `setup`),
    /// locked to `locked`.
    fn configure_rule(&self, locked: &SessionConfig) -> Rule;
    /// The rule for the object that asks for one response under its own settings, capped at the
    /// output ceiling `locked` states; `None` for a dialect with no such object.
    fn response_rule(&self, locked: &SessionConfig) -> Option<Rule>;
    /// The frame that tells the far end the session is locked to `locked`, in the shape its
    /// socket reads, for a caller-side envelope that configures nothing itself (the telephony
    /// carrier); `None` for a dialect whose writer frames it.
    fn locked_frame(&self, locked: &SessionConfig) -> Option<Vec<u8>>;
    /// Whether `frame` carries an item the caller may never author: a conversation item, or a
    /// per-response input item, in a role that instructs the model (`system`, `developer`). Such a
    /// frame goes nowhere. A frame whose bytes cannot be read here is answered `true`.
    fn authors_instructions(&self, frame: &[u8]) -> bool;
    /// The uplink audio format `frame` configures the session to (`Ok(None)`: it names none), read
    /// from every key the dialect's far end reads one from. `Err` when it names one the plane
    /// cannot meter, or names two that disagree: such a frame goes nowhere, because the far end
    /// would read audio at a rate the plane does not count it at.
    fn uplink_format(&self, frame: &[u8]) -> Result<Option<AudioFormat>, Unmeterable>;
}

/// The OpenAI Realtime member a response's output ceiling is written in.
const MAX_OUTPUT_TOKENS: &str = "max_output_tokens";
/// The pre-GA spelling of a session's per-response output ceiling, which a far end may still read.
const MAX_RESPONSE_OUTPUT_TOKENS: &str = "max_response_output_tokens";

/// Gemini Live's `setup` member holding the output ceiling.
const GENERATION_CONFIG: &str = "generationConfig";
/// The path to Gemini Live's output ceiling under `setup`.
const GENERATION_CONFIG_PATH: &[&str] = &[GENERATION_CONFIG];
/// Gemini Live's output ceiling, under `setup.generationConfig`.
const MAX_OUTPUT_TOKENS_LIVE: &str = "maxOutputTokens";

/// The output ceiling `locked` states, as a whole number; `None` for none (or `"inf"`).
fn output_ceiling(locked: &SessionConfig) -> Option<u64> {
    match locked.max_output_tokens {
        Some(MaxOutputTokens::Limit(n)) => Some(u64::from(n)),
        _ => None,
    }
}

/// OpenAI Realtime: `session.update`'s `session`, and `response.create`'s `response`. A stored
/// `prompt` is instructions by reference, so it is the caller's to author no more than they are.
const REALTIME_CUT: &[&str] = &["instructions", "tools", "prompt"];
/// Gemini Live: `setup`'s system instruction and tools.
const LIVE_CUT: &[&str] = &["systemInstruction", "tools"];
/// The roles an item instructs the model in: never the caller's to author.
const INSTRUCTING_ROLES: &[&str] = &["system", "developer"];

impl LockedKeys for OpenAiRealtimeCodec {
    /// The locked keys in the GA `session` shape the GA socket reads
    /// ([`SessionConfig::ga_session`]): a caller's GA members are merged with them member by
    /// member (`audio.input`, `audio.output`).
    ///
    /// The output ceiling is not set over the caller's: the caller's GA `max_output_tokens` is
    /// held to the lower of its value and the ceiling, and is the ceiling where it names none; a
    /// pre-GA `max_response_output_tokens` the caller writes is held the same way, and is never
    /// added (the GA session object reads the GA key, which carries the ceiling).
    fn configure_rule(&self, locked: &SessionConfig) -> Rule {
        let mut set = locked.ga_session();
        set.remove(MAX_OUTPUT_TOKENS);
        let caps = output_ceiling(locked)
            .map(|ceiling| {
                vec![
                    Cap {
                        path: &[],
                        key: MAX_OUTPUT_TOKENS,
                        ceiling,
                        when_absent: true,
                    },
                    Cap {
                        path: &[],
                        key: MAX_RESPONSE_OUTPUT_TOKENS,
                        ceiling,
                        when_absent: false,
                    },
                ]
            })
            .unwrap_or_default();
        Rule {
            at: "session",
            cut: REALTIME_CUT,
            set,
            proto_names: false,
            caps,
        }
    }

    /// `response.create`'s `response`: a caller's own `max_output_tokens` never lifts the
    /// session's output ceiling ([`crate::session_door::with_ceilings`]); the response asks for
    /// the lower of the two, and for the ceiling where the caller names none.
    fn response_rule(&self, locked: &SessionConfig) -> Option<Rule> {
        Some(Rule {
            at: "response",
            cut: REALTIME_CUT,
            set: Map::new(),
            proto_names: false,
            caps: output_ceiling(locked)
                .map(|ceiling| Cap {
                    path: &[],
                    key: MAX_OUTPUT_TOKENS,
                    ceiling,
                    when_absent: true,
                })
                .into_iter()
                .collect(),
        })
    }

    /// A GA `session.update` carrying the locked session ([`SessionConfig::ga_session`]).
    fn locked_frame(&self, locked: &SessionConfig) -> Option<Vec<u8>> {
        serde_json::to_vec(&serde_json::json!({
            "type": "session.update",
            "session": Value::Object(locked.ga_session()),
        }))
        .ok()
    }

    /// `conversation.item.create`'s `item`, and `response.create`'s `response.input` items, read
    /// wherever they sit at the top level.
    fn authors_instructions(&self, frame: &[u8]) -> bool {
        let Some((top, _)) = members(frame, ws(frame, 0)) else {
            return true;
        };
        for m in &top {
            let verdict = match m.key.as_str() {
                "item" => instructs(frame, m.value.0),
                "response" => input_instructs(frame, m.value.0),
                _ => Some(false),
            };
            if verdict != Some(false) {
                return true;
            }
        }
        false
    }

    /// The legacy `session.input_audio_format` and the GA `session.audio.input.format`, every
    /// spelling of each the frame carries.
    fn uplink_format(&self, frame: &[u8]) -> Result<Option<AudioFormat>, Unmeterable> {
        let (top, _) = members(frame, ws(frame, 0)).ok_or(Unmeterable)?;
        let mut named = Named::default();
        for session in top.iter().filter(|m| m.key == "session") {
            if frame.get(session.value.0) != Some(&b'{') {
                continue;
            }
            let (keys, _) = members(frame, session.value.0).ok_or(Unmeterable)?;
            for m in &keys {
                match m.key.as_str() {
                    "input_audio_format" => {
                        named.add(legacy_format(&frame[m.value.0..m.value.1])?)?
                    }
                    "audio" => {
                        for input in nested(frame, m.value.0, "input")? {
                            for format in nested(frame, input, "format")? {
                                named.add(ga_format(frame, format)?)?;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(named.0)
    }
}

/// A caller frame names an uplink audio format the plane cannot meter, or two that disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unmeterable;

/// The one format a frame's keys name, or none; a second that disagrees is refused.
#[derive(Default)]
struct Named(Option<AudioFormat>);

impl Named {
    fn add(&mut self, format: Option<AudioFormat>) -> Result<(), Unmeterable> {
        match (self.0, format) {
            (_, None) => Ok(()),
            (None, Some(f)) => {
                self.0 = Some(f);
                Ok(())
            }
            (Some(a), Some(b)) if a == b => Ok(()),
            _ => Err(Unmeterable),
        }
    }
}

/// The objects at every `key` member of the object at `b[open]` (none when `b[open]` is not an
/// object: the far end refuses that shape itself).
fn nested(b: &[u8], open: usize, key: &str) -> Result<Vec<usize>, Unmeterable> {
    if b.get(open) != Some(&b'{') {
        return Ok(Vec::new());
    }
    let (keys, _) = members(b, open).ok_or(Unmeterable)?;
    Ok(keys
        .iter()
        .filter(|m| m.key == key)
        .map(|m| m.value.0)
        .collect())
}

/// A legacy format token (`"pcm16"`, `"g711_ulaw"`); `null` names none.
fn legacy_format(value: &[u8]) -> Result<Option<AudioFormat>, Unmeterable> {
    match serde_json::from_slice::<Option<String>>(value).map_err(|_| Unmeterable)? {
        None => Ok(None),
        Some(token) => AudioFormat::from_wire(&token).map(Some).ok_or(Unmeterable),
    }
}

/// A GA format object (`{"type":"audio/pcm","rate":24000}`, `{"type":"audio/pcmu"}`), or a legacy
/// token where one is written in its place; `null` names none.
fn ga_format(b: &[u8], at: usize) -> Result<Option<AudioFormat>, Unmeterable> {
    let end = value_end(b, at).ok_or(Unmeterable)?;
    if b.get(at) != Some(&b'{') {
        return legacy_format(&b[at..end]);
    }
    let (keys, _) = members(b, at).ok_or(Unmeterable)?;
    let mut named = Named::default();
    let mut pcm = false;
    for m in keys.iter().filter(|m| m.key == "type") {
        let token: String =
            serde_json::from_slice(&b[m.value.0..m.value.1]).map_err(|_| Unmeterable)?;
        match token.as_str() {
            "audio/pcm" => pcm = true,
            "audio/pcmu" => named.add(Some(AudioFormat::G711Ulaw))?,
            _ => return Err(Unmeterable),
        }
    }
    if pcm {
        // The GA PCM format is 24 kHz; a rate it states must be that one.
        for m in keys.iter().filter(|m| m.key == "rate") {
            let rate: u64 =
                serde_json::from_slice(&b[m.value.0..m.value.1]).map_err(|_| Unmeterable)?;
            if rate != 24_000 {
                return Err(Unmeterable);
            }
        }
        named.add(Some(AudioFormat::Pcm16))?;
    }
    match named.0 {
        Some(f) => Ok(Some(f)),
        // A format object that names no type is one the plane cannot meter.
        None => Err(Unmeterable),
    }
}

/// Whether the item at `b[at]` is in an instructing role: `Some(true)` when any `role` it carries
/// is one, `Some(false)` when none is (or it is not an object), `None` when it cannot be read.
fn instructs(b: &[u8], at: usize) -> Option<bool> {
    if b.get(at) != Some(&b'{') {
        return Some(false);
    }
    let (keys, _) = members(b, at)?;
    for m in keys.iter().filter(|m| m.key == "role") {
        if let Ok(role) = serde_json::from_slice::<String>(&b[m.value.0..m.value.1]) {
            if INSTRUCTING_ROLES
                .iter()
                .any(|r| r.eq_ignore_ascii_case(role.trim()))
            {
                return Some(true);
            }
        }
    }
    Some(false)
}

/// Whether the response object at `b[at]` carries, in any `input` array, an item in an
/// instructing role.
fn input_instructs(b: &[u8], at: usize) -> Option<bool> {
    if b.get(at) != Some(&b'{') {
        return Some(false);
    }
    let (keys, _) = members(b, at)?;
    for m in keys.iter().filter(|m| m.key == "input") {
        for item in elements(b, m.value.0)? {
            if instructs(b, item)? {
                return Some(true);
            }
        }
    }
    Some(false)
}

/// Where each element of the array at `b[open]` starts; none when it is not an array.
fn elements(b: &[u8], open: usize) -> Option<Vec<usize>> {
    if b.get(open) != Some(&b'[') {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    let mut i = ws(b, open + 1);
    if b.get(i) == Some(&b']') {
        return Some(out);
    }
    loop {
        let end = value_end(b, i)?;
        out.push(i);
        let next = ws(b, end);
        match b.get(next)? {
            b',' => i = ws(b, next + 1),
            b']' => return Some(out),
            _ => return None,
        }
    }
}

impl LockedKeys for GeminiLiveCodec {
    /// `setup`, locked to `locked`. The output ceiling is not set over the caller's: the
    /// caller's `generationConfig.maxOutputTokens` is held to the lower of its value and the
    /// ceiling, and is the ceiling where it names none.
    fn configure_rule(&self, locked: &SessionConfig) -> Rule {
        let mut set = setup_locked(locked);
        let emptied = match set.get_mut(GENERATION_CONFIG) {
            Some(Value::Object(config)) => {
                config.remove(MAX_OUTPUT_TOKENS_LIVE);
                config.is_empty()
            }
            _ => false,
        };
        if emptied {
            set.remove(GENERATION_CONFIG);
        }
        Rule {
            at: "setup",
            cut: LIVE_CUT,
            set,
            proto_names: true,
            caps: output_ceiling(locked)
                .map(|ceiling| Cap {
                    path: GENERATION_CONFIG_PATH,
                    key: MAX_OUTPUT_TOKENS_LIVE,
                    ceiling,
                    when_absent: true,
                })
                .into_iter()
                .collect(),
        }
    }

    fn response_rule(&self, _locked: &SessionConfig) -> Option<Rule> {
        None
    }

    fn locked_frame(&self, _locked: &SessionConfig) -> Option<Vec<u8>> {
        None
    }

    /// Gemini Live's conversation turns are `user` and `model`: no item instructs the model.
    fn authors_instructions(&self, _frame: &[u8]) -> bool {
        false
    }

    /// Gemini Live states each uplink blob's format on the blob itself (its `mimeType`), and each
    /// blob is metered at the format it states: `setup` configures none.
    fn uplink_format(&self, _frame: &[u8]) -> Result<Option<AudioFormat>, Unmeterable> {
        Ok(None)
    }
}

/// `frame` with `rule` applied to every top-level `rule.at` object; the frame unchanged when it
/// holds none. `None` when the frame is not one JSON object.
#[must_use]
pub fn splice(frame: &[u8], rule: &Rule) -> Option<Vec<u8>> {
    let open = ws(frame, 0);
    let (members, close) = members(frame, open)?;
    if ws(frame, close) != frame.len() {
        return None;
    }
    let mut out = Vec::with_capacity(frame.len() + 64);
    let mut at = 0;
    for m in members.iter().filter(|m| m.key == rule.at) {
        if frame.get(m.value.0) != Some(&b'{') {
            continue;
        }
        let edited = object(
            frame,
            m.value.0,
            rule.cut,
            &rule.set,
            rule.proto_names,
            &rule.caps,
        )?;
        out.extend_from_slice(&frame[at..m.value.0]);
        out.extend_from_slice(&edited);
        at = m.value.1;
    }
    out.extend_from_slice(&frame[at..]);
    Some(out)
}

/// One member of an object: its key as the far end reads it, where it starts and its value's span.
struct JsonMember {
    key: String,
    start: usize,
    value: (usize, usize),
}

/// `key` as a protobuf JSON reader resolves it when `proto` (its lowerCamelCase JSON name: an
/// underscore is dropped and the letter after it raised), else as written.
fn field_name(key: &str, proto: bool) -> std::borrow::Cow<'_, str> {
    if !proto || !key.contains('_') {
        return std::borrow::Cow::Borrowed(key);
    }
    let mut out = String::with_capacity(key.len());
    let mut raise = false;
    for c in key.chars() {
        if c == '_' {
            raise = true;
        } else if raise {
            out.extend(c.to_uppercase());
            raise = false;
        } else {
            out.push(c);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// The object at `b[open]`, rebuilt: the caller's members not cut, not set and not capped, in order
/// and byte for byte; then each `set` member (merged into the caller's when both are objects); then
/// each object a cap reaches into that no `set` member names (the caller's, rebuilt under its caps,
/// or a new one where a cap is written in the caller's absence); then each cap on this object's own
/// members ([`Cap`]). Keys are compared under [`field_name`].
fn object(
    b: &[u8],
    open: usize,
    cut: &[&str],
    set: &Map<String, Value>,
    proto: bool,
    caps: &[Cap],
) -> Option<Vec<u8>> {
    let (members, _) = members(b, open)?;
    let here: Vec<Cap> = caps.iter().filter(|c| c.path.is_empty()).copied().collect();
    let mut under: Vec<&str> = caps
        .iter()
        .filter_map(|c| c.path.first().copied())
        .collect();
    under.sort_unstable();
    under.dedup();
    let below = |parent: &str| -> Vec<Cap> {
        caps.iter()
            .filter(|c| c.path.first() == Some(&parent))
            .map(|c| {
                let path: &'static [&'static str] = c.path;
                Cap {
                    path: &path[1..],
                    ..*c
                }
            })
            .collect()
    };
    let capped = |name: &str| here.iter().any(|c| c.key == name);
    let reaches = |name: &str| under.contains(&name);
    let is_object = |m: &JsonMember| b.get(m.value.0) == Some(&b'{');
    let mut out = vec![b'{'];
    let mut first = true;
    let mut sep = |out: &mut Vec<u8>| {
        if !std::mem::take(&mut first) {
            out.push(b',');
        }
    };
    for m in &members {
        let name = field_name(&m.key, proto);
        if cut.contains(&&*name)
            || set.contains_key(&*name)
            || capped(&name)
            || (reaches(&name) && is_object(m))
        {
            continue;
        }
        sep(&mut out);
        out.extend_from_slice(&b[m.start..m.value.1]);
    }
    // The caller's last object under `key`, as the far end reads the last of a repeated key.
    let theirs = |key: &str| {
        members
            .iter()
            .rev()
            .find(|m| field_name(&m.key, proto) == key && is_object(m))
    };
    for (key, value) in set {
        sep(&mut out);
        out.extend_from_slice(&serde_json::to_vec(key).ok()?);
        out.push(b':');
        let sub = below(key);
        match (value, theirs(key)) {
            (Value::Object(locked), Some(m)) => {
                out.extend_from_slice(&object(b, m.value.0, &[], locked, proto, &sub)?);
            }
            (Value::Object(locked), None) => {
                out.extend_from_slice(&serde_json::to_vec(&fresh(locked, &sub)).ok()?);
            }
            _ => out.extend_from_slice(&serde_json::to_vec(value).ok()?),
        }
    }
    for parent in under.iter().copied().filter(|p| !set.contains_key(*p)) {
        let sub = below(parent);
        let rebuilt = match theirs(parent) {
            Some(m) => object(b, m.value.0, &[], &Map::new(), proto, &sub)?,
            None if sub.iter().any(|c| c.when_absent)
                && !members.iter().any(|m| field_name(&m.key, proto) == parent) =>
            {
                serde_json::to_vec(&fresh(&Map::new(), &sub)).ok()?
            }
            None => continue,
        };
        sep(&mut out);
        out.extend_from_slice(&serde_json::to_vec(parent).ok()?);
        out.push(b':');
        out.extend_from_slice(&rebuilt);
    }
    for Cap {
        key,
        ceiling,
        when_absent,
        ..
    } in here
    {
        // Every value the caller wrote under the key is read; one that is not a whole number
        // (`"inf"`, a fraction, a string) asks for no ceiling of its own, so the cap stands.
        let written: Vec<&JsonMember> = members
            .iter()
            .filter(|m| field_name(&m.key, proto) == key)
            .collect();
        if when_absent || !written.is_empty() {
            let asked = written
                .iter()
                .map(|m| serde_json::from_slice::<u64>(&b[m.value.0..m.value.1]).unwrap_or(ceiling))
                .fold(ceiling, u64::min);
            sep(&mut out);
            out.extend_from_slice(&serde_json::to_vec(key).ok()?);
            out.push(b':');
            out.extend_from_slice(asked.to_string().as_bytes());
        }
    }
    out.push(b'}');
    Some(out)
}

/// `locked` as a new object where the caller wrote none, with each cap that is written in the
/// caller's absence ([`Cap::when_absent`]) at its ceiling.
fn fresh(locked: &Map<String, Value>, caps: &[Cap]) -> Value {
    let mut m = locked.clone();
    'caps: for c in caps.iter().filter(|c| c.when_absent) {
        let mut at = &mut m;
        for p in c.path {
            let next = at
                .entry((*p).to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            let Value::Object(o) = next else {
                continue 'caps;
            };
            at = o;
        }
        at.insert(c.key.to_string(), Value::from(c.ceiling));
    }
    Value::Object(m)
}

/// The index of the first byte at or after `i` that is not JSON whitespace.
fn ws(b: &[u8], mut i: usize) -> usize {
    while matches!(b.get(i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        i += 1;
    }
    i
}

/// The index past the JSON string starting at `b[i]`.
fn string_end(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// The index past the JSON value starting at `b[i]`.
fn value_end(b: &[u8], i: usize) -> Option<usize> {
    match b.get(i)? {
        b'"' => string_end(b, i),
        b'{' | b'[' => {
            let mut depth = 0_usize;
            let mut j = i;
            while j < b.len() {
                match b[j] {
                    b'"' => {
                        j = string_end(b, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth = depth.checked_sub(1)?;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
            {
                j += 1;
            }
            (j > i).then_some(j)
        }
    }
}

/// The members of the object at `b[open]`, and the index past its closing brace.
fn members(b: &[u8], open: usize) -> Option<(Vec<JsonMember>, usize)> {
    if b.get(open) != Some(&b'{') {
        return None;
    }
    let mut out = Vec::new();
    let mut i = ws(b, open + 1);
    if b.get(i) == Some(&b'}') {
        return Some((out, i + 1));
    }
    loop {
        if b.get(i) != Some(&b'"') {
            return None;
        }
        let key_end = string_end(b, i)?;
        let key: String = serde_json::from_slice(&b[i..key_end]).ok()?;
        let colon = ws(b, key_end);
        if b.get(colon) != Some(&b':') {
            return None;
        }
        let start = ws(b, colon + 1);
        let end = value_end(b, start)?;
        out.push(JsonMember {
            key,
            start: i,
            value: (start, end),
        });
        let next = ws(b, end);
        match b.get(next)? {
            b',' => i = ws(b, next + 1),
            b'}' => return Some((out, next + 1)),
            _ => return None,
        }
    }
}

#[cfg(test)]
#[path = "tests/splice_tests.rs"]
mod tests;
