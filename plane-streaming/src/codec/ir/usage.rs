// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Usage extraction.

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
#[must_use]
pub fn read_count_u64(v: &serde_json::Value) -> Option<u64> {
    if let Some(u) = v.as_u64() {
        return Some(u);
    }
    let f = v.as_f64()?;

    const EXACT_INTEGER_LIMIT: f64 = 9_007_199_254_740_992.0;
    if (0.0..EXACT_INTEGER_LIMIT).contains(&f) && f.fract() == 0.0 {
        Some(f as u64)
    } else {
        None
    }
}
