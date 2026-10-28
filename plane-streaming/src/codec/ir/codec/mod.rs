// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The meter's reads: what a live session reads off each frame, in each dialect.
//!
//! A session is a relay: a same-dialect relay runs the reader only as a read-only tap and sends
//! the client's bytes with governed byte-level splices. So the reader answers only what
//! the session meters: from the far end, a usage report, an error, a tool call's open; from the
//! caller, the audio it sends. The plane writes no frame of its own beyond the two a session is
//! told, the locked session and the ceiling's error ([`crate::splice::LockedKeys`]).
//!
//! The dialect is OpenAI Realtime GA: every wire event is a JSON object tagged on a `type` field. A
//! frame whose head names no metered `type` is not read further ([`peek::Head`]); a metered one is
//! read as a `serde_json::Value` and hand-mapped by `type`. Uplink audio is counted off one borrowed
//! read of the frame ([`peek::Lite`]), its length taken from the base64 length
//! ([`peek::base64_len`]). A malformed or unrecognized frame meters nothing.

use crate::codec::ir::media::AudioFormat;
use crate::codec::ir::usage::IrDuplexUsage;
use serde_json::Value;

pub mod peek;
use peek::{base64_len, Head, Lite};

/// The dialect wire `type` tokens the meter reads — named once here.
mod wire {
    pub const INPUT_AUDIO_APPEND: &str = "input_audio_buffer.append";
    pub const OUTPUT_ITEM_ADDED: &str = "response.output_item.added";
    pub const RESPONSE_DONE: &str = "response.done";
    pub const ERROR: &str = "error";
    pub const ITEM_FN_CALL: &str = "function_call";
}

/// ONE WIRE EVENT — the opaque, dialect-shaped message a session relays: the JSON bytes of one
/// event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireEvent(pub bytes::Bytes);

/// WHAT A FAR-END FRAME SAYS THAT A SESSION METERS, in the order the frame says it.
#[derive(Debug, Clone, PartialEq)]
pub enum Metered {
    /// A usage report: the turn closes with it.
    Usage(IrDuplexUsage),
    /// An error: the turn closes.
    Error,
    /// The far end opened a tool call.
    ToolOpen,
}

// ── helpers ─────────────────────────────────────────────────────────────────────────────────────

fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// One output token per this many bytes of an unreadable usage report: the llm plane's floor
/// estimate (`TRUNCATED_TAIL_BYTES_PER_TOKEN`).
const FLOOR_BYTES_PER_TOKEN: u64 = 4;

/// Read one BILLED count: an absent object, an absent field or a JSON `null` is 0 (no such quantity
/// was reported), a readable count is the count (through the crate's one seam,
/// [`crate::codec::ir::usage::read_count_u64`]), and a present-but-UNREADABLE count is a refusal: a count the
/// provider really sent is never metered as zero. The error is the operator-facing message: the field and
/// a BOUNDED spelling (cut on a character, so a hostile multi-byte spelling cannot panic the cut).
fn billed_count(o: Option<&Value>, key: &str) -> Result<u64, String> {
    match o.and_then(|x| x.get(key)) {
        None => Ok(0),
        Some(v) if v.is_null() => Ok(0),
        Some(v) => crate::codec::ir::usage::read_count_u64(v).ok_or_else(|| {
            let spelling: String = v.to_string().chars().take(64).collect();
            format!("usage field `{key}` is present but is not a count: {spelling}")
        }),
    }
}

/// What an unreadable usage report bills (spend is always attributed and nothing is silent): the
/// llm plane's floor, one output token per [`FLOOR_BYTES_PER_TOKEN`] bytes
/// of the report and at least one, marked as the plane's floor. Its one log line names the field and
/// its spelling; the door's call capture carries it to the plugin's host log.
pub(crate) fn usage_floor(report_len: usize, message: &str) -> IrDuplexUsage {
    busbar_contract::diag_warn!(
        busbar_contract::diagnostic::USAGE_TAP_DECODE_FAILED,
        delivered = report_len,
        "{message}: usage is unreadable; billing the floor estimate, never 0"
    );
    IrDuplexUsage {
        text_out: (report_len as u64 / FLOOR_BYTES_PER_TOKEN).max(1),
        floor: true,
        ..IrDuplexUsage::default()
    }
}

// ── the reads ───────────────────────────────────────────────────────────────────────────────────

/// THE METER'S READS of one dialect, both directions. Stateless: nothing a frame says changes how
/// the next is read.
pub trait DuplexReader {
    /// What a far-end frame says that a session meters; empty for a frame that says none of it
    /// (audio, most control), which is not read past its head.
    fn meter_down(&self, frame: &[u8]) -> Vec<Metered>;

    /// The uplink audio of a caller frame read whole as `top`: each blob's stated format (`None`:
    /// it states none, and the session's governs) and the length its audio decodes to.
    fn uplink_audio(&self, top: &Lite<'_>, admit: &mut dyn FnMut(Option<AudioFormat>, usize));
}

/// THE OpenAI Realtime GA DIALECT CODEC. A unit struct: stateless and shareable.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenAiRealtimeCodec;

impl DuplexReader for OpenAiRealtimeCodec {
    /// A `response.done` (its usage), an `error`, and a `response.output_item.added` that adds a
    /// `function_call`.
    fn meter_down(&self, frame: &[u8]) -> Vec<Metered> {
        let metered = Head::read(frame).is_none_or(|h| {
            matches!(
                h.ty.as_deref(),
                Some(wire::RESPONSE_DONE | wire::ERROR | wire::OUTPUT_ITEM_ADDED)
            )
        });
        if !metered {
            return Vec::new();
        }
        let Ok(v) = serde_json::from_slice::<Value>(frame) else {
            return Vec::new();
        };
        match str_at(&v, "type") {
            wire::OUTPUT_ITEM_ADDED
                if v.get("item").map_or("", |item| str_at(item, "type")) == wire::ITEM_FN_CALL =>
            {
                vec![Metered::ToolOpen]
            }
            // An absent `usage` is no report (zero); a present-but-unreadable billed count bills
            // the plane's floor and says so (never metered at zero).
            wire::RESPONSE_DONE => vec![Metered::Usage(
                match v
                    .get("response")
                    .and_then(|r| r.get("usage"))
                    .map(extract_usage)
                    .transpose()
                {
                    Ok(usage) => usage.unwrap_or_default(),
                    Err(message) => usage_floor(frame.len(), &message),
                },
            )],
            wire::ERROR => vec![Metered::Error],
            _ => Vec::new(),
        }
    }

    /// An `input_audio_buffer.append`'s `audio`, which states no format.
    fn uplink_audio(&self, top: &Lite<'_>, admit: &mut dyn FnMut(Option<AudioFormat>, usize)) {
        if top.str_at("type") == wire::INPUT_AUDIO_APPEND {
            if let Some(len) = base64_len(top.str_at("audio")) {
                admit(None, len);
            }
        }
    }
}

/// Extract the split token classes from a `response.done.usage` object (audio vs text are
/// separate classes; extraction-only, never client-translated).
///
/// The stated `input_tokens`/`output_tokens` totals are what the far end bills ("Usage statistics
/// for the Response, this will correspond to billing", the Realtime API reference), and the
/// per-modality breakdown (`input_token_details`/`output_token_details`) only SPLITS them. So each
/// side bills the larger of its stated total and the sum of every entry its breakdown carries
/// (`audio_tokens`, `text_tokens`, and on input `image_tokens`), never the audio and text entries
/// alone: a turn that also carried image input billed its audio and text and dropped the rest.
/// Audio bills as audio and image tokens as image tokens (each its own declared class); every
/// other token the side bills is attributed to `text`. It is an integer passthrough of the wire
/// figures, no rate lookup or multiply.
///
/// This is the exact twin of the Gemini Live dialect's `usage_from_metadata` (`gemini/mod.rs`).
///
/// Every count is a BILLED count and is read through [`billed_count`]: absent is zero, and a
/// present-but-unreadable one is an `Err` the caller bills the floor for ([`usage_floor`]).
fn extract_usage(u: &Value) -> Result<IrDuplexUsage, String> {
    let ind = u.get("input_token_details");
    let outd = u.get("output_token_details");
    let audio_in = billed_count(ind, "audio_tokens")?;
    let image_in = billed_count(ind, "image_tokens")?;
    let input = billed_count(Some(u), "input_tokens")?.max(
        audio_in
            .saturating_add(billed_count(ind, "text_tokens")?)
            .saturating_add(image_in),
    );
    let audio_out = billed_count(outd, "audio_tokens")?;
    let output = billed_count(Some(u), "output_tokens")?
        .max(audio_out.saturating_add(billed_count(outd, "text_tokens")?));
    Ok(IrDuplexUsage {
        audio_in,
        audio_out,
        text_in: input - audio_in - image_in,
        text_out: output - audio_out,
        image_in,
        video_in: 0,
        cached: billed_count(ind, "cached_tokens")?,
        floor: false,
    })
}

/// The second dialect, Gemini Live (`BidiGenerateContent`). See [`gemini::GeminiLiveCodec`].
pub mod gemini;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "usage_refusal_tests.rs"]
mod usage_refusal_tests;

#[cfg(test)]
#[path = "peek_tests.rs"]
mod peek_tests;
