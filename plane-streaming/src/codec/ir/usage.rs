// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Usage and rate-limit extraction.
//!
//! `response.done.usage` (audio vs text are SEPARATE token classes, audio dominates) and
//! `rate_limits.updated` are EXTRACTED, never client-translated. This is the metering/audit tap that
//! the plane folds into counts per class (whose labels core never interprets) for the kernel's
//! metering + `journal_append_scoped`. The same move the LLM reader makes with `IrUsage`.

/// The neutral usage carrier: token classes the plane extracts from a duplex turn. Folded into a
/// `CostBreakdown` whose top-level components sum to `total` (the one invariant core enforces), with
/// audio/text as labeled opaque components core never interprets.
///
/// A plain token-class tally. The plane meters it through its own declared classes
/// (`crate::session::class_counts`); `cached` is attribution only, a subset of the input it is part
/// of, and never billed on its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IrDuplexUsage {
    /// Audio input tokens consumed this turn.
    pub audio_in: u64,
    /// Audio output tokens produced this turn.
    pub audio_out: u64,
    /// Text input tokens consumed this turn.
    pub text_in: u64,
    /// Text output tokens produced this turn.
    pub text_out: u64,
    /// Image input tokens consumed this turn (OpenAI `input_token_details.image_tokens`, Gemini's
    /// IMAGE modality entries): a class of their own, priced on their own.
    pub image_in: u64,
    /// Video input tokens consumed this turn (Gemini's VIDEO modality entries): a class of their
    /// own, priced on their own.
    pub video_in: u64,
    /// Cached tokens billed at the cache rate this turn.
    pub cached: u64,
    /// The far end's report could not be read: these counts are the plane's FLOOR over the report's
    /// bytes, never the far end's own (the llm plane's floor).
    pub floor: bool,
}

/// The one seam every billed count read in this crate goes through.
///
/// [`serde_json::Value::as_u64`] returns `None` for any float-backed `Number`, including `27.0`,
/// which is exactly an integer; reading with `.unwrap_or_default()` would turn a provider's real
/// count of 27 into a recorded count of zero, invisible at every layer below the read.
///
/// The LLM codec crate carries the identical seam (`busbar_llm_codec::usage_count::read_count_u64`)
/// and this is deliberately a separate copy, not an import: the two crates are per-plane codecs and
/// a codec→codec edge is a lateral plugin edge. This function is the one place in this crate a
/// count is read.
///
/// # What is rejected, and why
///
/// A fractional value, a negative value, a non-finite value, and anything at or above 2^53 (past
/// which an `f64` no longer represents consecutive integers, so the integrality test stops carrying
/// information) all read as `None`. Rounding would invent a quantity the provider never reported.
///
/// A refused value is never read as zero: every billed count in this crate is read through the
/// codecs' `billed_count`, which locates a present-but-unreadable count: the report
/// then bills the plane's floor and says so in one log line, and the session goes on.
#[must_use]
pub fn read_count_u64(v: &serde_json::Value) -> Option<u64> {
    if let Some(u) = v.as_u64() {
        return Some(u);
    }
    let f = v.as_f64()?;
    // 2^53 — the largest magnitude at which an f64 still represents every integer distinctly, and
    // therefore the largest at which `fract() == 0.0` still proves anything. `contains` also
    // rejects NaN and both infinities, since neither compares inside any range.
    const EXACT_INTEGER_LIMIT: f64 = 9_007_199_254_740_992.0;
    if (0.0..EXACT_INTEGER_LIMIT).contains(&f) && f.fract() == 0.0 {
        Some(f as u64)
    } else {
        None
    }
}
