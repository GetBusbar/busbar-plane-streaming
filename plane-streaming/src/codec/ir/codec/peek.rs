// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! A FRAME READ FOR THE METER, NOT FOR ITS PAYLOAD.
//!
//! A live session meters every frame in both directions, and nearly every frame is audio: a
//! whole-frame read builds the frame as a `serde_json::Value`, copying its base64 audio into a
//! string of its own, and then decodes that audio, only for the meter to take its length. These
//! reads answer the meter's questions off the frame's own bytes:
//!
//! - [`Head`]: a frame's top-level member names and its `type`, read leniently (nothing below the
//!   top level is checked or kept), which is enough to tell that a far-end frame cannot carry a
//!   usage report, an error or a tool call's open, and so needs no whole-frame read;
//! - [`Lite`]: the whole frame, read exactly as strictly as a `Value` read (every string checked, a
//!   repeated member's last value the one that counts), with its strings borrowed from the frame;
//! - [`base64_len`]: the length a base64 payload decodes to, without decoding it.

use serde::de::{Deserialize, Deserializer, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::borrow::Cow;
use std::fmt;

/// One JSON value, read as a whole-frame `serde_json::Value` read takes it, with its strings
/// borrowed from the frame where they hold no escape.
#[derive(Debug)]
pub enum Lite<'a> {
    /// A string.
    Str(Cow<'a, str>),
    /// An object's members, in the order written.
    Obj(Vec<(Cow<'a, str>, Lite<'a>)>),
    /// An array.
    Arr(Vec<Lite<'a>>),
    /// A number, a boolean or `null`.
    Other,
}

impl<'a> Lite<'a> {
    /// `frame` read whole; `None` where a `Value` read of it fails.
    #[must_use]
    pub fn read(frame: &'a [u8]) -> Option<Self> {
        serde_json::from_slice(frame).ok()
    }

    /// The member `key` of an object, the last one written, as `Value::get` answers.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Lite<'a>> {
        match self {
            Lite::Obj(members) => members.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The string member `key` of an object; empty where there is none.
    #[must_use]
    pub fn str_at(&self, key: &str) -> &str {
        match self.get(key) {
            Some(Lite::Str(s)) => s,
            _ => "",
        }
    }

    /// An object's member names; `None` when this is not an object.
    #[must_use]
    pub fn keys(&self) -> Option<impl Iterator<Item = &str>> {
        match self {
            Lite::Obj(members) => Some(members.iter().map(|(k, _)| &**k)),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for Lite<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(LiteVisitor)
    }
}

struct LiteVisitor;

impl<'de> Visitor<'de> for LiteVisitor {
    type Value = Lite<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E> {
        Ok(Lite::Str(Cow::Borrowed(v)))
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
        Ok(Lite::Str(Cow::Owned(v.to_owned())))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
        Ok(Lite::Str(Cow::Owned(v)))
    }

    fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
        Ok(Lite::Other)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
        Ok(Lite::Other)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
        Ok(Lite::Other)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
        Ok(Lite::Other)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Lite::Other)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some(v) = seq.next_element()? {
            out.push(v);
        }
        Ok(Lite::Arr(out))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some((k, v)) = map.next_entry::<Lite<'de>, Lite<'de>>()? {
            let Lite::Str(k) = k else {
                return Err(A::Error::custom("a member name that is not a string"));
            };
            out.push((k, v));
        }
        Ok(Lite::Obj(out))
    }
}

/// A frame's top-level member names and its `type` (the last one written, where that is a
/// string), read leniently: what a far-end frame says it is before anything reads what it carries.
#[derive(Debug, Default)]
pub struct Head<'a> {
    /// The top-level member names, in the order written.
    pub keys: Vec<Cow<'a, str>>,
    /// The `type` member, where the last one written is a string.
    pub ty: Option<Cow<'a, str>>,
}

impl<'a> Head<'a> {
    /// `frame`'s head; `None` when it is not a JSON object.
    #[must_use]
    pub fn read(frame: &'a [u8]) -> Option<Self> {
        serde_json::from_slice(frame).ok()
    }
}

impl<'de> Deserialize<'de> for Head<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_map(HeadVisitor)
    }
}

struct HeadVisitor;

impl<'de> Visitor<'de> for HeadVisitor {
    type Value = Head<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut head = Head::default();
        while let Some(k) = map.next_key::<Lite<'de>>()? {
            let Lite::Str(k) = k else {
                return Err(A::Error::custom("a member name that is not a string"));
            };
            if k == "type" {
                head.ty = match map.next_value::<Lite<'de>>()? {
                    Lite::Str(t) => Some(t),
                    _ => None,
                };
            } else {
                map.next_value::<IgnoredAny>()?;
            }
            head.keys.push(k);
        }
        Ok(head)
    }
}

/// The length [`busbar_contract::media::base64_decode`] decodes `b64` to, counted without decoding
/// it: every base64 digit is six bits, so `n` digits are `n * 6 / 8` bytes. `None` exactly where the
/// decode refuses: a byte that is neither a digit, `=` nor whitespace, a digit after the padding,
/// or a lone trailing digit.
#[must_use]
pub fn base64_len(b64: &str) -> Option<usize> {
    digits_len(b64, b"+/")
}

/// [`base64_len`] over either base64 alphabet, the standard one (`+`, `/`) and the URL-safe one
/// (`-`, `_`), as a protobuf JSON far end (Gemini Live) reads a `bytes` field: the length a decode
/// in the blob's own alphabet yields. `None` where neither alphabet decodes it.
#[must_use]
pub fn base64_len_any_alphabet(b64: &str) -> Option<usize> {
    digits_len(b64, b"+/-_")
}

/// The decoded length of `b64` whose non-alphanumeric digits are `extra`.
fn digits_len(b64: &str, extra: &[u8]) -> Option<usize> {
    let mut digits = 0_usize;
    let mut padded = false;
    for &b in b64.as_bytes() {
        if b.is_ascii_whitespace() {
            continue;
        }
        if b == b'=' {
            padded = true;
            continue;
        }
        if padded || !(b.is_ascii_alphanumeric() || extra.contains(&b)) {
            return None;
        }
        digits += 1;
    }
    (digits % 4 != 1).then_some(digits / 4 * 3 + (digits % 4).saturating_sub(1))
}
