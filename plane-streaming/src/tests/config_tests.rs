// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Tests for `crates/busbar-plane-streaming/src/config.rs`.

use super::*;

/// An empty `streams: {}` decodes to exactly [`StreamsCfg::default`]: the hand-written `Default`
/// and the serde field defaults agree, so an omitted section and an empty one are the same posture.
#[test]
fn an_empty_section_decodes_to_the_default() {
    let empty: StreamsCfg = serde_json::from_str("{}").expect("an empty section parses");
    assert_eq!(empty, StreamsCfg::default());
    assert_eq!(empty.session_max_secs, None, "no ceiling unless configured");
}

/// A typo'd key is refused by the grammar itself, not by a later pass.
#[test]
fn an_unknown_key_is_refused() {
    let err = serde_json::from_str::<StreamsCfg>(r#"{"nonsense_key": 1}"#)
        .expect_err("an unknown key is refused");
    assert!(err.to_string().contains("nonsense_key"), "{err}");
}

/// RED-BEFORE-GREEN (THE DESIGN §2 "Validation refusals, every kind"): a key the plane does not
/// model INSIDE `streams.session` is refused with its path. RED: `instruction:` (a typo of
/// `instructions:`) was dropped, every session opened with no locked instructions, and the
/// governed splice still cut the caller's own.
#[test]
fn an_unknown_key_inside_the_locked_session_is_refused_with_its_path() {
    for (blob, path) in [
        (
            r#"{"session":{"instruction":"be terse"}}"#,
            "streams.session.instruction",
        ),
        (
            r#"{"session":{"turn_detection":{"type":"server_vad","silence_ms":300}}}"#,
            "streams.session.turn_detection.silence_ms",
        ),
    ] {
        let err = serde_json::from_str::<StreamsCfg>(blob).expect_err("refused");
        assert!(err.to_string().contains(path), "{blob}: {err}");
    }
    let ok: StreamsCfg =
        serde_json::from_str(r#"{"session":{"instructions":"be terse","voice":"verse"}}"#)
            .expect("every modelled key reads");
    assert_eq!(ok.session.instructions.as_deref(), Some("be terse"));
}

