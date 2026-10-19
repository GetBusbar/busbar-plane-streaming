//! The names this plane's door declares its meter classes and its session-open operation under, and
//! the one conversion between the milliseconds the plane counts and the seconds its duration class
//! is denominated in. The door's Statement tail ([`crate::door`]) is the plane's one declaration:
//! these are the spellings it reads, never a second declaration beside it.
//!
//! The meter-class list is the one `docs/design/BUSBAR-1.6.0.md`'s protocol-inventory table names
//! for the streaming row. In particular the table names `audio_seconds_in` and no outbound twin:
//! metering the downlink by duration would be a second reading of the same audio the
//! `audio_tokens_out` class already prices.

use busbar_contract::ids::{MeterClassId, OpClassId};

/// Milliseconds in the second the `audio_seconds_in` class is denominated in.
const MS_PER_SECOND: u64 = 1_000;

/// The seconds figure the `audio_seconds_in` class carries, from the milliseconds this plane counts.
///
/// The class the design names is a duration in SECONDS. This plane's own bookkeeping is in
/// milliseconds, because a frame's byte count under the declared format assumption is what produces
/// it, and that division has a resolution finer than a second. Every reader of the counter that
/// writes a meter line goes through here, so the two units meet at one boundary rather than at each
/// of them — reported verbatim, a turn of audio settled at a thousand times its duration.
///
/// A part-second rounds UP. Audio that arrived is not audio that cost nothing, and a line reporting
/// zero seconds for it settles exactly as an absent line would.
#[must_use]
pub const fn audio_seconds_in(ms: u64) -> u64 {
    ms.div_ceil(MS_PER_SECOND)
}

/// The class key the audio a turn RECEIVED is counted under.
///
/// Exported for the same reason as [`CLASS_AUDIO_TOKENS_OUT`]: the class label is what selects a
/// unit price, and a root that spelled it out inline would be one more chance for that spelling to
/// drift from the declaration the door's tail makes.
pub const CLASS_AUDIO_TOKENS_IN: MeterClassId = MeterClassId::new("audio_tokens_in");

/// The class key the audio a turn EMITTED is counted under.
///
/// Exported because the class label is what selects a unit price, and the composition root names
/// this one three times: the estimate a turn is admitted against, the class its evidence reports,
/// and the class the settled line prices under. Three spellings of one wire string is three chances
/// for a turn to be admitted against one class and priced under another, which a rate card with two
/// different rates settles as money. This is the one spelling; the door's tail and the session's
/// counts read it.
pub const CLASS_AUDIO_TOKENS_OUT: MeterClassId = MeterClassId::new("audio_tokens_out");

/// The class key the text that PROMPTED a turn is counted under.
///
/// Exported for the same reason as [`CLASS_AUDIO_TOKENS_OUT`]: the class label is what selects a
/// unit price, and a root that spelled it out inline would be one more chance for that spelling to
/// drift from the declaration the door's tail makes.
pub const CLASS_TEXT_TOKENS_IN: MeterClassId = MeterClassId::new("text_tokens_in");

/// The class key the text a turn EMITTED is counted under.
///
/// Exported for the same reason as [`CLASS_AUDIO_TOKENS_OUT`]: the class label is what selects a
/// unit price, and a root that spelled it out inline would be one more chance for that spelling to
/// drift from the declaration the door's tail makes.
pub const CLASS_TEXT_TOKENS_OUT: MeterClassId = MeterClassId::new("text_tokens_out");

/// The key that names a cache-read token count — NOT a billable class (architect ruling #71,
/// money-model integrity: a plane's billable classes are pairwise disjoint, and both duplex
/// dialects report `cached_tokens` as a SUBSET of the input token classes above, so pricing it
/// beside them would bill the same cached input twice). The door's tail declares no such class, so
/// no rate card can configure it and no `class_counts` line ledgers it. Kept as a `MeterClassId`
/// because the codecs that read the figure identify it by the same typed key every class uses.
pub const CLASS_CACHED_TOKENS: MeterClassId = MeterClassId::new("cached_tokens");

/// The class key the seconds of inbound audio a turn consumed are counted under.
///
/// Exported for the same reason as [`CLASS_AUDIO_TOKENS_OUT`]: the class label is what selects a
/// unit price, and a root that spelled it out inline would be one more chance for that spelling to
/// drift from the declaration the door's tail makes.
pub const CLASS_AUDIO_SECONDS_IN: MeterClassId = MeterClassId::new("audio_seconds_in");

/// The class key a tool call is counted under.
///
/// Exported for the same reason as [`CLASS_AUDIO_TOKENS_OUT`]: the class label is what selects a
/// unit price, and a root that spelled it out inline would be one more chance for that spelling to
/// drift from the declaration the door's tail makes.
pub const CLASS_TOOL_CALLS: MeterClassId = MeterClassId::new("tool_calls");

/// The name a session's opening unit is audited and priced under.
///
/// The audit record's shape is fixed for every plane and a plane contributes exactly two ids to it:
/// an operation class and a finish class. So "a session opened" is not an event kind of its own — it
/// is *this* operation class on the unit that opened it, which is why the name is declared here
/// beside the other four rather than invented at the point a record is sealed.
pub const OP_SESSION_OPEN: OpClassId = OpClassId::new("streaming.session.open");
