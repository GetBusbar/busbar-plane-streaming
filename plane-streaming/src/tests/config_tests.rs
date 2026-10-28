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

/// RED-BEFORE-GREEN (THE DESIGN section 2, "Validation refusals, every kind"): a key the plane does
/// not model INSIDE `streams.session` is refused with its path. RED: `instruction:` (a typo of
/// `instructions:`) was dropped, every session opened with no locked instructions, and the governed
/// splice still cut the caller's own.
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

/// The strict key lists name exactly the members the session object serializes: a field added to
/// [`SessionConfig`] or [`IrVad`] must be added to them (each literal below names every field, so
/// a new one fails to compile here first).
#[test]
fn the_strict_key_lists_are_the_session_objects_own_members() {
    use crate::codec::ir::config::Eagerness;
    use crate::codec::ir::config::MaxOutputTokens;
    use crate::codec::ir::media::AudioFormat;
    let keys = |v: serde_json::Value| -> Vec<String> {
        let mut k: Vec<String> = v.as_object().expect("object").keys().cloned().collect();
        k.sort();
        k
    };
    let sorted = |list: &[&str]| -> Vec<String> {
        let mut k: Vec<String> = list.iter().map(|s| (*s).to_string()).collect();
        k.sort();
        k
    };
    let full = SessionConfig {
        model: Some("m".into()),
        modalities: vec!["audio".into()],
        instructions: Some("i".into()),
        voice: Some("v".into()),
        input_audio_format: Some(AudioFormat::Pcm16),
        output_audio_format: Some(AudioFormat::Pcm16),
        turn_detection: Some(None),
        tools: vec![serde_json::json!({})],
        tool_choice: Some(serde_json::json!("auto")),
        max_output_tokens: Some(MaxOutputTokens::Inf),
        context_window_tokens: Some(1),
    };
    assert_eq!(
        keys(serde_json::to_value(&full).expect("json")),
        sorted(SESSION_KEYS)
    );
    let server = IrVad::ServerVad {
        threshold: 0.5,
        prefix_padding_ms: 1,
        silence_duration_ms: 1,
        create_response: true,
        interrupt_response: true,
    };
    let semantic = IrVad::SemanticVad {
        eagerness: Eagerness::Auto,
    };
    for (vad, ty) in [(server, "server_vad"), (semantic, "semantic_vad")] {
        let (_, list) = VAD_KEYS.iter().find(|(t, _)| *t == ty).expect("listed");
        assert_eq!(
            keys(serde_json::to_value(&vad).expect("json")),
            sorted(list)
        );
    }
}

/// RED-BEFORE-GREEN (the section's two token ceilings): a per-response output cap at or above the
/// context window leaves the locked session no room for the conversation, so the section is refused
/// at load, the refusal naming both settings and their values; the defaults (4096 below 32768)
/// load and leave the conversation 28672 tokens. RED: such a section loaded and the locked
/// session's context limit after its output saturated to 0.
#[test]
fn an_output_cap_at_or_above_the_context_window_is_refused_naming_both_settings() {
    for (blob, words) in [
        (
            r#"{"max_output_tokens":4096,"context_window_tokens":4096}"#,
            "streams.max_output_tokens (4096) must be below streams.context_window_tokens (4096)",
        ),
        (
            r#"{"max_output_tokens":8192,"context_window_tokens":4096}"#,
            "streams.max_output_tokens (8192) must be below streams.context_window_tokens (4096)",
        ),
    ] {
        let refused = crate::door::read_settings(blob.as_bytes()).expect_err(blob);
        assert!(refused.contains(words), "{blob}: {refused}");
    }
    for blob in [&b""[..], &b"{}"[..]] {
        let cfg = crate::door::read_settings(blob).expect("the defaults load");
        assert_eq!(
            (cfg.max_output_tokens, cfg.context_window_tokens),
            (4096, 32_768)
        );
        let locked = crate::session_door::with_ceilings(cfg.session.clone(), &cfg);
        assert_eq!(
            locked.ga_session()["truncation"]["token_limits"]["post_instructions"],
            28_672
        );
    }
}
