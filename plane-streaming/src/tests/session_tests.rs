// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Tests for `session.rs`: the per-turn audio counters and a closed turn's class counts.

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
