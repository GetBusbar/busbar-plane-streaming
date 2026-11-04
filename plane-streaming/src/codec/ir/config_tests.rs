// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

use super::*;

#[test]
fn audit_ga_limits_and_semantic_default() {
    for (input, expected) in [(0, 1), (8000, 4096), (17, 17)] {
        let cfg = SessionConfig {
            max_output_tokens: Some(MaxOutputTokens::Limit(input)),
            ..Default::default()
        };
        assert_eq!(cfg.ga_session()["max_output_tokens"], expected);
    }
    let vad: IrVad = serde_json::from_str(r#"{"type":"semantic_vad"}"#).unwrap();
    assert_eq!(
        vad,
        IrVad::SemanticVad {
            eagerness: Eagerness::Auto
        }
    );
}
