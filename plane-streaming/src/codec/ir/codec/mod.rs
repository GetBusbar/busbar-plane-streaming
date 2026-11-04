// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The meter's reads: what a live session reads off each frame, in each dialect.

use crate::codec::ir::media::AudioFormat;
use crate::codec::ir::usage::IrDuplexUsage;
use serde_json::Value;

pub mod peek;
use peek::{base64_len, Head, Lite};

/// The dialect wire `type` tokens the meter reads — named once here.
pub(crate) mod wire {
    pub const INPUT_AUDIO_APPEND: &str = "input_audio_buffer.append";
    pub const ITEM_CREATE: &str = "conversation.item.create";
    pub const RESPONSE_CREATE: &str = "response.create";
    pub const INPUT_AUDIO_PART: &str = "input_audio";
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

fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// One output token per this many bytes of an unreadable usage report: the llm plane's floor conversion.
const FLOOR_BYTES_PER_TOKEN: u64 = 4;

/// Read one BILLED count: an absent object, an absent field or a JSON `null` is 0 (no such quantity was reported).
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

/// A present usage report `u` that is not an object: unreadable, never an all-zero report. The error is handled by the caller through the billing floor.
pub(crate) fn usage_object(u: &Value, member: &str) -> Result<(), String> {
    if u.is_object() {
        return Ok(());
    }
    let spelling: String = u.to_string().chars().take(64).collect();
    Err(format!(
        "usage `{member}` is present but is not an object: {spelling}"
    ))
}

/// What an unreadable usage report bills (spend is always attributed and nothing is silent): the byte-derived floor is attributed to output text.
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

/// THE METER'S READS of one dialect, both directions. Stateless: nothing a frame says changes how another frame is interpreted.
pub trait DuplexReader {
    /// What a far-end frame says that a session meters; empty for a frame that says none of it .
    fn meter_down(&self, frame: &[u8]) -> Vec<Metered>;

    /// The uplink audio of a caller frame read whole as `top`: each blob's stated format (`None`:
    fn uplink_audio(
        &self,
        top: &Lite<'_>,
        admit: &mut dyn FnMut(Option<AudioFormat>, usize),
    ) -> Result<(), crate::splice::Unmeterable>;
}

/// THE OpenAI Realtime GA DIALECT CODEC. A unit struct: stateless and shareable.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenAiRealtimeCodec;

impl DuplexReader for OpenAiRealtimeCodec {
    /// A `response.done` (its usage), an `error`, and a `response.output_item.added` that adds a function call are the billable events.
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
        if Lite::read(frame).is_none() {
            return if Head::read(frame)
                .is_some_and(|h| h.ty.as_deref() == Some(wire::RESPONSE_DONE))
            {
                vec![Metered::Usage(usage_floor(
                    frame.len(),
                    "the response exceeds the bounded JSON reader",
                ))]
            } else {
                Vec::new()
            };
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

            wire::RESPONSE_DONE => vec![Metered::Usage(
                match v
                    .get("response")
                    .and_then(|r| r.get("usage"))
                    .filter(|u| !u.is_null())
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

    /// An `input_audio_buffer.append`'s `audio`, each `input_audio` content part of a
    /// `conversation.item.create`'s item, and of every item a `response.create` carries in its
    /// `response.input`. Audio that cannot be counted (not a string, not base64, or a member the
    /// frame names twice) is `Err`: the frame is refused, never relayed uncounted.
    fn uplink_audio(
        &self,
        top: &Lite<'_>,
        admit: &mut dyn FnMut(Option<AudioFormat>, usize),
    ) -> Result<(), crate::splice::Unmeterable> {
        match frame_type(top)? {
            wire::INPUT_AUDIO_APPEND => admit(None, audio_len(top)?),
            wire::ITEM_CREATE => item_audio(only(top, "item")?, admit)?,
            wire::RESPONSE_CREATE => {
                let input = only(top, "response")?
                    .map(|response| only(response, "input"))
                    .transpose()?
                    .flatten();
                if let Some(Lite::Arr(items)) = input {
                    for item in items {
                        item_audio(Some(item), admit)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// The member `key` of `holder`, named at most once.
fn only<'l, 'a>(
    holder: &'l Lite<'a>,
    key: &str,
) -> Result<Option<&'l Lite<'a>>, crate::splice::Unmeterable> {
    holder.only(key).map_err(|_| crate::splice::Unmeterable)
}

/// The frame's `type`, the last one written. A frame naming more than one `type` where any of
/// them carries uplink audio is `Err`: a far end that reads the first would read another frame.
fn frame_type<'l>(top: &'l Lite<'_>) -> Result<&'l str, crate::splice::Unmeterable> {
    let Lite::Obj(members) = top else {
        return Ok("");
    };
    let mut types = members.iter().filter(|(k, _)| k == "type").map(|(_, v)| v);
    let audio_bearing = |v: &Lite<'_>| matches!(v, Lite::Str(t) if matches!(t.as_ref(), wire::INPUT_AUDIO_APPEND | wire::ITEM_CREATE | wire::RESPONSE_CREATE | wire::INPUT_AUDIO_PART));
    if types.clone().nth(1).is_some() && types.any(audio_bearing) {
        return Err(crate::splice::Unmeterable);
    }
    Ok(top.str_at("type"))
}

/// The decoded length of `holder`'s `audio`: `0` where it names none, `Err` where it is not one
/// base64 string.
fn audio_len(holder: &Lite<'_>) -> Result<usize, crate::splice::Unmeterable> {
    match only(holder, "audio")? {
        None | Some(Lite::Null) => Ok(0),
        Some(Lite::Str(audio)) => base64_len(audio).ok_or(crate::splice::Unmeterable),
        Some(_) => Err(crate::splice::Unmeterable),
    }
}

/// Each `input_audio` content part of `item`.
fn item_audio(
    item: Option<&Lite<'_>>,
    admit: &mut dyn FnMut(Option<AudioFormat>, usize),
) -> Result<(), crate::splice::Unmeterable> {
    let Some(item) = item else {
        return Ok(());
    };
    if let Some(Lite::Arr(parts)) = only(item, "content")? {
        for part in parts {
            if frame_type(part)? == wire::INPUT_AUDIO_PART {
                admit(None, audio_len(part)?);
            }
        }
    }
    Ok(())
}

/// Extract the split token classes from a `response.done.usage` object (audio vs text are attributed separately).
fn extract_usage(u: &Value) -> Result<IrDuplexUsage, String> {
    usage_object(u, "usage")?;
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
        text_in: input.saturating_sub(audio_in).saturating_sub(image_in),
        text_out: output.saturating_sub(audio_out),
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
