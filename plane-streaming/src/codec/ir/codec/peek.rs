// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! A FRAME READ FOR THE METER, NOT FOR ITS PAYLOAD.

use serde::de::{
    Deserialize, DeserializeSeed, Deserializer, Error, IgnoredAny, MapAccess, SeqAccess, Visitor,
};
use std::borrow::Cow;
use std::cell::Cell;
use std::fmt;

/// The most values (every string, number, member name, object and array) one frame is read into:
/// a frame of more is not read at all, so a hostile frame of tiny members cannot be multiplied in
/// memory before anything judges it. Far above any frame a dialect writes.
pub const MAX_LITE_NODES: usize = 1 << 16;

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
    /// JSON `null` (ProtoJSON reads a null field as unset).
    Null,
    /// A number or a boolean.
    Other,
}

impl<'a> Lite<'a> {
    /// `frame` read whole; `None` where a `Value` read of it fails, or where it holds more than
    /// [`MAX_LITE_NODES`] values.
    #[must_use]
    pub fn read(frame: &'a [u8]) -> Option<Self> {
        let budget = Cell::new(MAX_LITE_NODES);
        let mut de = serde_json::Deserializer::from_slice(frame);
        let lite = LiteSeed(&budget).deserialize(&mut de).ok()?;
        de.end().ok()?;
        Some(lite)
    }

    /// The one member `key` of an object: `Err` where the object names it more than once (a reader
    /// that keeps the first and one that keeps the last would read the frame differently).
    pub fn only(&self, key: &str) -> Result<Option<&Lite<'a>>, Ambiguous> {
        let Lite::Obj(members) = self else {
            return Ok(None);
        };
        let mut named = members.iter().filter(|(k, _)| k == key).map(|(_, v)| v);
        let first = named.next();
        if named.next().is_some() {
            return Err(Ambiguous);
        }
        Ok(first)
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

/// An object names a member more than once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ambiguous;

impl<'de> Deserialize<'de> for Lite<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let budget = Cell::new(MAX_LITE_NODES);
        LiteSeed(&budget).deserialize(d)
    }
}

/// One value read under a frame's shared budget of values ([`MAX_LITE_NODES`]).
struct LiteSeed<'b>(&'b Cell<usize>);

impl<'de> DeserializeSeed<'de> for LiteSeed<'_> {
    type Value = Lite<'de>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(LiteVisitor(self.0))
    }
}

struct LiteVisitor<'b>(&'b Cell<usize>);

impl LiteVisitor<'_> {
    /// Spend one value of the frame's budget, or refuse the frame once it is spent.
    fn spend<E: Error>(&self) -> Result<(), E> {
        match self.0.get().checked_sub(1) {
            Some(left) => {
                self.0.set(left);
                Ok(())
            }
            None => Err(E::custom("a frame of more values than the meter reads")),
        }
    }
}

impl<'de> Visitor<'de> for LiteVisitor<'_> {
    type Value = Lite<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_borrowed_str<E: Error>(self, v: &'de str) -> Result<Self::Value, E> {
        self.spend::<E>()?;
        Ok(Lite::Str(Cow::Borrowed(v)))
    }

    fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
        self.spend::<E>()?;
        Ok(Lite::Str(Cow::Owned(v.to_owned())))
    }

    fn visit_string<E: Error>(self, v: String) -> Result<Self::Value, E> {
        self.spend::<E>()?;
        Ok(Lite::Str(Cow::Owned(v)))
    }

    fn visit_bool<E: Error>(self, _: bool) -> Result<Self::Value, E> {
        self.spend::<E>()?;
        Ok(Lite::Other)
    }

    fn visit_i64<E: Error>(self, _: i64) -> Result<Self::Value, E> {
        self.spend::<E>()?;
        Ok(Lite::Other)
    }

    fn visit_u64<E: Error>(self, _: u64) -> Result<Self::Value, E> {
        self.spend::<E>()?;
        Ok(Lite::Other)
    }

    fn visit_f64<E: Error>(self, _: f64) -> Result<Self::Value, E> {
        self.spend::<E>()?;
        Ok(Lite::Other)
    }

    fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
        self.spend::<E>()?;
        Ok(Lite::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        self.spend::<A::Error>()?;
        let mut out = Vec::new();
        while let Some(v) = seq.next_element_seed(LiteSeed(self.0))? {
            out.push(v);
        }
        Ok(Lite::Arr(out))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        self.spend::<A::Error>()?;
        let mut out = Vec::new();
        while let Some(k) = map.next_key_seed(LiteSeed(self.0))? {
            let Lite::Str(k) = k else {
                return Err(A::Error::custom("a member name that is not a string"));
            };
            let v = map.next_value_seed(LiteSeed(self.0))?;
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
            if head.keys.len() >= MAX_LITE_NODES {
                return Err(A::Error::custom("too many frame members"));
            }
            head.keys.push(k);
        }
        Ok(head)
    }
}

/// The length [`busbar_contract::media::base64_decode`] decodes `b64` to, counted without decoding or allocating the payload.
#[must_use]
pub fn base64_len(b64: &str) -> Option<usize> {
    digits_len(b64, b"+/")
}

/// [`base64_len`] over either base64 alphabet, the standard one (`+`, `/`) and the URL-safe one (`-`, `_`).
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
