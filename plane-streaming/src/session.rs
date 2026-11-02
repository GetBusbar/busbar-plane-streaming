//! The plane's per-turn bookkeeping: the counters a turn keeps that no usage report carries, and a

use crate::codec::ir::{AudioFormat, IrDuplexUsage};
use busbar_contract::ids::MeterClassId;

use crate::meta;

/// The plane's own bookkeeping for the current, still-open turn.
///
/// Closed (its counts reset to zero) every time a turn is ANSWERED — a usage report, an upstream error or
/// a carrier `stop`, each of which states them on its own facts — and CARRIED, not reset, when a
/// barge-in supersedes the open turn, because a superseded turn is never answered and nothing else
/// would ever state what it served. These are the quantities [`crate::codec::ir::usage::IrDuplexUsage`] does not carry and this plane must
/// derive itself: `audio_seconds_in` from the byte counts of ingress audio frames, `tool_calls` from
/// counting the tool calls the far end opens as they are read.
///
/// **The session is one unit, so its uplink audio is converted once.** Two
/// positions are carried from turn to turn rather than reset with the turn's counts: the bytes of
/// admitted audio short of a whole millisecond (a frame's bytes do not divide into milliseconds
/// evenly, and flooring each frame lost the remainder every frame), and the milliseconds the
/// session's earlier turns already settled. A closed turn bills the DIFFERENCE of the session's
/// cumulative seconds figure ([`class_counts`]), so the sum over every turn is the session's total
/// milliseconds converted once — rounding each turn up on its own billed 20 turns of 1050 ms as 40 s
/// of audio rather than 21 s. Each frame is counted at its own format, and a session may carry more
/// than one (a Gemini caller may send 16 kHz and 24 kHz blobs), so the remainder is kept in
/// [`TICKS_PER_MS`] ticks, a unit every format's byte is a whole number of: a remainder carried from
/// a frame of one format onto a frame of another stays exact.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnCounters {
    /// Milliseconds of ingress audio admitted since the turn opened.
    pub audio_ms_in: u64,
    /// Tool calls the upstream opened since the turn opened.
    pub tool_calls: u64,
    /// Milliseconds of ingress audio the session's closed turns settled before this one (carried).
    audio_ms_before: u64,
    /// Admitted ingress audio short of a whole millisecond, in [`TICKS_PER_MS`] ticks (carried).
    audio_ticks_rem: u64,
    /// The carried remainder when this turn opened: it is not new audio by itself.
    audio_ticks_at_open: u64,
    /// The rounded cumulative seconds already settled by earlier turns.
    audio_seconds_before: u64,
}

/// The ticks a millisecond of audio divides into: the least common multiple of every format's bytes
const TICKS_PER_MS: u64 = 96;

const _: () = {
    assert!(TICKS_PER_MS.is_multiple_of(AudioFormat::Pcm16.bytes_per_ms()));
    assert!(TICKS_PER_MS.is_multiple_of(AudioFormat::Pcm16At16k.bytes_per_ms()));
    assert!(TICKS_PER_MS.is_multiple_of(AudioFormat::G711Ulaw.bytes_per_ms()));
};

impl TurnCounters {
    /// Count `bytes` of admitted uplink audio, read under `format`. The part of a millisecond the
    pub fn admit_audio(&mut self, format: AudioFormat, bytes: usize) {
        let ticks = (bytes as u64).saturating_mul(TICKS_PER_MS / format.bytes_per_ms());
        let total = self.audio_ticks_rem.saturating_add(ticks);
        self.audio_ms_in = self.audio_ms_in.saturating_add(total / TICKS_PER_MS);
        self.audio_ticks_rem = total % TICKS_PER_MS;
    }

    /// Count one tool call the upstream opened.
    pub fn open_tool_call(&mut self) {
        self.tool_calls = self.tool_calls.saturating_add(1);
    }

    /// Whether nothing has been counted since the turn opened.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.audio_ms_in == 0
            && self.audio_ticks_rem == self.audio_ticks_at_open
            && self.tool_calls == 0
    }

    /// Close the turn: answer its counters as they stood, and leave in their place the next turn's,
    pub fn close(&mut self) -> TurnCounters {
        let closed = *self;
        *self = TurnCounters {
            audio_ms_before: closed.audio_ms_before.saturating_add(closed.audio_ms_in),
            audio_ticks_rem: closed.audio_ticks_rem,
            audio_ticks_at_open: closed.audio_ticks_rem,
            audio_seconds_before: closed.cumulative_audio_seconds(),
            ..TurnCounters::default()
        };
        closed
    }

    /// The `audio_seconds_in` this turn bills: the session's cumulative seconds after it less the
    #[must_use]
    pub fn audio_seconds_in(&self) -> u64 {
        self.cumulative_audio_seconds()
            .saturating_sub(self.audio_seconds_before)
    }

    fn cumulative_audio_seconds(&self) -> u64 {
        let ms = self.audio_ms_before.saturating_add(self.audio_ms_in);

        meta::audio_seconds_in(ms).saturating_add(u64::from(
            ms.is_multiple_of(1_000) && self.audio_ticks_rem != 0,
        ))
    }
}

/// A CLOSED TURN'S RAW COUNTS PER DECLARED CLASS — the lines this plane's `meter` emits for a turn
#[must_use]
pub fn class_counts(
    usage: Option<&IrDuplexUsage>,
    counters: TurnCounters,
) -> Vec<(MeterClassId, u64)> {
    let reported = usage.copied().unwrap_or_default();

    [
        (meta::CLASS_AUDIO_TOKENS_IN, reported.audio_in),
        (meta::CLASS_AUDIO_TOKENS_OUT, reported.audio_out),
        (meta::CLASS_TEXT_TOKENS_IN, reported.text_in),
        (meta::CLASS_TEXT_TOKENS_OUT, reported.text_out),
        (meta::CLASS_AUDIO_SECONDS_IN, counters.audio_seconds_in()),
        (meta::CLASS_TOOL_CALLS, counters.tool_calls),
        (meta::CLASS_IMAGE_TOKENS_IN, reported.image_in),
        (meta::CLASS_VIDEO_TOKENS_IN, reported.video_in),
    ]
    .into_iter()
    .filter(|(_, n)| *n != 0)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_audio_is_nonempty_and_rounded_once_across_turns() {
        let mut counters = TurnCounters::default();
        counters.admit_audio(AudioFormat::Pcm16, 2);
        assert!(!counters.is_empty());
        assert_eq!(counters.audio_seconds_in(), 1);
        assert_eq!(counters.close().audio_seconds_in(), 1);
        assert!(
            counters.is_empty(),
            "a carried remainder is not a second admission"
        );
        assert_eq!(counters.audio_seconds_in(), 0);
        counters.admit_audio(AudioFormat::Pcm16, 47_998);
        assert_eq!(
            counters.close().audio_seconds_in(),
            0,
            "the first exact second is already settled"
        );
        counters.admit_audio(AudioFormat::Pcm16At16k, 2);
        assert_eq!(
            counters.close().audio_seconds_in(),
            1,
            "a sample after an exact second enters the next one"
        );
        assert!(counters.is_empty());
        assert_eq!(
            counters.close().audio_seconds_in(),
            0,
            "close is idempotent"
        );
    }

    #[test]
    fn fractional_audio_after_one_second_survives_frame_splitting() {
        for parts in [vec![48_002], vec![48_000, 2], vec![2; 24_001]] {
            let mut counters = TurnCounters::default();
            for bytes in parts {
                counters.admit_audio(AudioFormat::Pcm16, bytes);
            }
            assert_eq!(counters.audio_seconds_in(), 2);
            assert_eq!(
                class_counts(None, counters),
                vec![(meta::CLASS_AUDIO_SECONDS_IN, 2)]
            );
        }
    }
}
