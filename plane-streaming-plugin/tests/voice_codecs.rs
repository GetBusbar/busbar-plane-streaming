// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Fixture bytes and captured transcripts through the actual streaming doors.

use busbar_contract::abi::mechanism::call::Outcome;
use busbar_contract::abi::plane::{
    FROM_CALLER, FROM_FAR_END, FROM_KERNEL, PIECE_HAS_STATUS, UNITS_REPORTED,
};
use busbar_plugin_loader::dispatch::{DispatchConfig, Dispatcher};
use serde_json::Value;

#[path = "support/voice_abi.rs"]
pub mod abi;
#[path = "support/voice_codec_oracles.rs"]
mod oracles;

// These checks deliberately read the actual ABI answers directly. There is no session
// simulator or expected-answer interpreter between the test and the product.
fn assert_answer(a: &abi::Answer, claim: u32) {
    assert_eq!(a.outcome, Outcome::Ready);
    assert_eq!(a.more, 0);
    assert_eq!(
        a.records, 0,
        "a live session must not write request records"
    );
    if !a.emitted.is_empty() && a.to_far {
        assert_eq!(a.need, if claim == 5 { 4 } else { 5 });
        assert_eq!(a.verb, "GET");
        assert!(abi::dials_socket(
            &a.target,
            if claim == 5 {
                "/v1/realtime"
            } else {
                "/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent"
            }
        ));
    }
    assert!(a.units.iter().all(|u| u.1 == UNITS_REPORTED));
}

fn fixture(claim: u32, bytes: &[u8], from: u32, expected: &[u8], tokens: &[(u32, u64)]) {
    let value: Value = serde_json::from_slice(bytes).expect("unchanged fixture is valid JSON");
    let wire = serde_json::to_vec(&value).unwrap();
    for dropped in [false, true] {
        let dispatcher = Dispatcher::new(DispatchConfig::default());
        let plugin = abi::load(&dispatcher, "voice-codec-fixture", dropped);
        assert_eq!(
            abi::open(&plugin, abi::SESSION, Some(abi::PUBLIC)).outcome,
            Outcome::Ready
        );
        let initial = abi::piece(&plugin, &abi::session(77, claim, FROM_CALLER, 0, &[]));
        assert_answer(&initial, claim);
        if claim == 5 {
            assert!(initial.to_far);
            assert_eq!(initial.target, "/v1/realtime?model=m-cap");
            let lock: Value = serde_json::from_slice(&initial.emitted).unwrap();
            assert_eq!(lock["type"], "session.update");
            assert_eq!(lock["session"]["type"], "realtime");
            assert_eq!(lock["session"]["max_output_tokens"], 4096);
            assert_eq!(
                lock["session"]["truncation"]["token_limits"]["post_instructions"],
                28672
            );
        } else {
            assert!(initial.emitted.is_empty());
        }
        let mut answer = abi::piece(
            &plugin,
            &abi::Piece {
                status: if from == FROM_FAR_END { 101 } else { 0 },
                ..abi::session(
                    77,
                    claim,
                    from,
                    if from == FROM_FAR_END {
                        PIECE_HAS_STATUS
                    } else {
                        0
                    },
                    &wire,
                )
            },
        );
        let mut emissions = Vec::new();
        let mut drained = false;
        for drain_index in 0..=64 {
            assert_answer(&answer, claim);
            if !answer.emitted.is_empty() {
                emissions.push((answer.to_far, answer.emitted.clone()));
            }
            if answer.done {
                drained = true;
                break;
            }
            if drain_index == 64 {
                break;
            }
            let next = abi::piece(&plugin, &abi::session(77, claim, FROM_KERNEL, 0, &[]));
            let quiet = next.emitted.is_empty() && !next.done;
            assert_answer(&next, claim);
            answer = next;
            if quiet {
                drained = true;
                break;
            }
        }
        assert!(
            drained,
            "the real door must run dry within the original drain bound"
        );
        assert_eq!(emissions, vec![(from == FROM_CALLER, expected.to_vec())]);
        assert_eq!(
            answer
                .units
                .iter()
                .filter(|u| u.0 < 4)
                .map(|u| (u.0, u.2))
                .collect::<Vec<_>>(),
            tokens
        );
        assert_eq!(
            answer.units.iter().any(|u| u.0 == 8 && u.2 == 1),
            from == FROM_FAR_END
        );
    }
}

#[test]
fn openai_conversation_item_create_function_call_output() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/openai/conversation.item.create.function_call_output.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("conversation.item.create")
    );
    assert_eq!(value["item"]["type"], "function_call_output");
    assert_eq!(value["item"]["call_id"], "call_weather_abc");
    assert_eq!(
        serde_json::from_str::<Value>(value["item"]["output"].as_str().unwrap()).unwrap(),
        serde_json::json!({"temp_c":18,"condition":"partly cloudy"})
    );
    fixture(
        5,
        bytes,
        FROM_CALLER,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_conversation_item_create_message() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/openai/conversation.item.create.message.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("conversation.item.create")
    );
    assert_eq!(value["item"]["type"], "message");
    assert_eq!(value["item"]["role"], "user");
    assert_eq!(
        value["item"]["content"][0]["text"],
        "What's the weather in Paris?"
    );
    fixture(
        5,
        bytes,
        FROM_CALLER,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_conversation_item_truncate() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/openai/conversation.item.truncate.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("conversation.item.truncate")
    );
    assert_eq!(value["item_id"], "item_asst_002");
    assert_eq!(value["audio_end_ms"], 1200);
    fixture(
        5,
        bytes,
        FROM_CALLER,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_error() {
    let bytes = include_bytes!("../../testing/voice-conformance/fixtures/openai/error.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(value.pointer("/type").unwrap(), &serde_json::json!("error"));
    assert_eq!(value["error"]["code"], "invalid_value");
    assert_eq!(
        value["error"]["message"],
        "Unknown parameter: 'session.foo'."
    );
    fixture(
        5,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_input_audio_buffer_append() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/openai/input_audio_buffer.append.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("input_audio_buffer.append")
    );
    assert_eq!(
        value["audio"],
        "UklGRhwAAABXQVZFZm10IBAAAAABAAEAgD4AAAB9AAACABAAZGF0YQAAAAA="
    );
    fixture(
        5,
        bytes,
        FROM_CALLER,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_input_audio_buffer_clear() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/openai/input_audio_buffer.clear.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("input_audio_buffer.clear")
    );
    fixture(
        5,
        bytes,
        FROM_CALLER,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_input_audio_buffer_commit() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/openai/input_audio_buffer.commit.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("input_audio_buffer.commit")
    );
    fixture(
        5,
        bytes,
        FROM_CALLER,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_input_audio_buffer_speech_started() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/openai/input_audio_buffer.speech_started.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("input_audio_buffer.speech_started")
    );
    assert_eq!(value["audio_start_ms"], 210);
    assert_eq!(value["item_id"], "item_user_002");
    fixture(
        5,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_input_audio_buffer_speech_stopped() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/openai/input_audio_buffer.speech_stopped.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("input_audio_buffer.speech_stopped")
    );
    assert_eq!(value["audio_end_ms"], 980);
    assert_eq!(value["item_id"], "item_user_002");
    fixture(
        5,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_response_audio_delta() {
    let bytes =
        include_bytes!("../../testing/voice-conformance/fixtures/openai/response.audio.delta.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("response.audio.delta")
    );
    assert_eq!(
        value["delta"],
        "UklGRhwAAABXQVZFZm10IBAAAAABAAEAgD4AAAB9AAACABAAZGF0YQAAAAA="
    );
    fixture(
        5,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_response_audio_done() {
    let bytes =
        include_bytes!("../../testing/voice-conformance/fixtures/openai/response.audio.done.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("response.audio.done")
    );
    assert_eq!(value["item_id"], "item_asst_002");
    fixture(
        5,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_response_cancel() {
    let bytes =
        include_bytes!("../../testing/voice-conformance/fixtures/openai/response.cancel.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("response.cancel")
    );
    fixture(
        5,
        bytes,
        FROM_CALLER,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_response_create() {
    let bytes =
        include_bytes!("../../testing/voice-conformance/fixtures/openai/response.create.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("response.create")
    );
    fixture(5, bytes, FROM_CALLER, oracles::OPENAI_RESPONSE_CREATE, &[]);
}

#[test]
fn openai_response_done() {
    let bytes =
        include_bytes!("../../testing/voice-conformance/fixtures/openai/response.done.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("response.done")
    );
    fixture(
        5,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[(0, 100), (1, 60), (2, 40), (3, 20)],
    );
}

#[test]
fn openai_response_function_call_arguments_delta() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/openai/response.function_call_arguments.delta.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("response.function_call_arguments.delta")
    );
    assert_eq!(value["call_id"], "call_weather_abc");
    assert_eq!(value["delta"], "{\"city\":\"Pa");
    fixture(
        5,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_response_function_call_arguments_done() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/openai/response.function_call_arguments.done.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("response.function_call_arguments.done")
    );
    assert_eq!(value["call_id"], "call_weather_abc");
    assert_eq!(
        serde_json::from_str::<Value>(value["arguments"].as_str().unwrap()).unwrap(),
        serde_json::json!({"city":"Paris"})
    );
    fixture(
        5,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_session_created() {
    let bytes =
        include_bytes!("../../testing/voice-conformance/fixtures/openai/session.created.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("session.created")
    );
    fixture(
        5,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn openai_session_update_ga_nested() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/openai/session.update.ga_nested.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("session.update")
    );
    fixture(
        5,
        bytes,
        FROM_CALLER,
        oracles::OPENAI_SESSION_UPDATE_GA_NESTED,
        &[],
    );
}

#[test]
fn openai_session_update() {
    let bytes =
        include_bytes!("../../testing/voice-conformance/fixtures/openai/session.update.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("session.update")
    );
    fixture(5, bytes, FROM_CALLER, oracles::OPENAI_SESSION_UPDATE, &[]);
}

#[test]
fn openai_session_update_semantic_vad() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/openai/session.update.semantic_vad.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/type").unwrap(),
        &serde_json::json!("session.update")
    );
    fixture(
        5,
        bytes,
        FROM_CALLER,
        oracles::OPENAI_SESSION_UPDATE_SEMANTIC_VAD,
        &[],
    );
}

#[test]
fn gemini_client_content() {
    let bytes =
        include_bytes!("../../testing/voice-conformance/fixtures/gemini/clientContent.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/clientContent").unwrap(),
        &serde_json::json!({"turns":[{"role":"user","parts":[{"text":"What's the weather in Paris?"}]}],"turnComplete":true})
    );
    fixture(
        3,
        bytes,
        FROM_CALLER,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn gemini_go_away() {
    // The old IR had no advance-disconnect twin. That documented IR drop never drops the opaque wire frame.
    let bytes = include_bytes!("../../testing/voice-conformance/fixtures/gemini/goAway.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/goAway").unwrap(),
        &serde_json::json!({"timeLeft":"10s"})
    );
    fixture(
        3,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn gemini_realtime_input_audio_stream_end() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/gemini/realtimeInput.audioStreamEnd.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/realtimeInput").unwrap(),
        &serde_json::json!({"audioStreamEnd":true})
    );
    fixture(
        3,
        bytes,
        FROM_CALLER,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn gemini_realtime_input() {
    let bytes =
        include_bytes!("../../testing/voice-conformance/fixtures/gemini/realtimeInput.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/realtimeInput").unwrap(),
        &serde_json::json!({"audio":{"mimeType":"audio/pcm;rate=16000","data":"UklGRhwAAABXQVZFZm10IBAAAAABAAEAgD4AAAB9AAACABAAZGF0YQAAAAA="}})
    );
    fixture(
        3,
        bytes,
        FROM_CALLER,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn gemini_realtime_input_media_chunks() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/gemini/realtimeInput.mediaChunks.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/realtimeInput").unwrap(),
        &serde_json::json!({"mediaChunks":[{"mimeType":"audio/pcm;rate=16000","data":"UklGRhwAAABXQVZFZm10IBAAAAABAAEAgD4AAAB9AAACABAAZGF0YQAAAAA="}]})
    );
    fixture(
        3,
        bytes,
        FROM_CALLER,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn gemini_server_content_input_transcription() {
    // The old shared IR had no transcription side-channel. The opaque input transcript must still reach the caller.
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/gemini/serverContent.inputTranscription.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/serverContent").unwrap(),
        &serde_json::json!({"inputTranscription":{"text":"What's the weather in Paris?"}})
    );
    fixture(
        3,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn gemini_server_content_interrupted() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/gemini/serverContent.interrupted.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/serverContent").unwrap(),
        &serde_json::json!({"interrupted":true})
    );
    fixture(
        3,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn gemini_server_content_model_turn() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/gemini/serverContent.modelTurn.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/serverContent").unwrap(),
        &serde_json::json!({"modelTurn":{"role":"model","parts":[{"inlineData":{"mimeType":"audio/pcm;rate=24000","data":"UklGRhwAAABXQVZFZm10IBAAAAABAAEAgD4AAAB9AAACABAAZGF0YQAAAAA="}}]}})
    );
    fixture(
        3,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn gemini_server_content_output_transcription() {
    // The old shared IR had no transcription side-channel. The opaque output transcript must still reach the caller.
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/gemini/serverContent.outputTranscription.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/serverContent").unwrap(),
        &serde_json::json!({"outputTranscription":{"text":"It's 18 degrees and partly cloudy in Paris."}})
    );
    fixture(
        3,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn gemini_server_content_turn_complete() {
    let bytes = include_bytes!(
        "../../testing/voice-conformance/fixtures/gemini/serverContent.turnComplete.json"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/serverContent").unwrap(),
        &serde_json::json!({"turnComplete":true,"generationComplete":true})
    );
    fixture(
        3,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn gemini_setup() {
    let bytes = include_bytes!("../../testing/voice-conformance/fixtures/gemini/setup.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/setup").unwrap(),
        &serde_json::json!({"model":"models/gemini-2.0-flash-exp","generationConfig":{"responseModalities":["AUDIO"],"temperature":0.8,"maxOutputTokens":512,"speechConfig":{"voiceConfig":{"prebuiltVoiceConfig":{"voiceName":"Puck"}}}},"systemInstruction":{"parts":[{"text":"You are a concise, friendly voice assistant. Keep answers short."}]},"tools":[{"functionDeclarations":[{"name":"get_weather","description":"Get the current weather for a city.","parameters":{"type":"OBJECT","properties":{"city":{"type":"STRING","description":"City name"}},"required":["city"]}}]}],"realtimeInputConfig":{"automaticActivityDetection":{"disabled":false,"startOfSpeechSensitivity":"START_SENSITIVITY_HIGH","endOfSpeechSensitivity":"END_SENSITIVITY_HIGH","prefixPaddingMs":300,"silenceDurationMs":500}},"inputAudioTranscription":{},"outputAudioTranscription":{}})
    );
    fixture(3, bytes, FROM_CALLER, oracles::GEMINI_SETUP, &[]);
}

#[test]
fn gemini_setup_complete() {
    let bytes =
        include_bytes!("../../testing/voice-conformance/fixtures/gemini/setupComplete.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/setupComplete").unwrap(),
        &serde_json::json!({})
    );
    fixture(
        3,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn gemini_tool_call() {
    let bytes = include_bytes!("../../testing/voice-conformance/fixtures/gemini/toolCall.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/toolCall").unwrap(),
        &serde_json::json!({"functionCalls":[{"id":"function-call-14582213604383","name":"get_weather","args":{"city":"Paris"}}]})
    );
    fixture(
        3,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn gemini_tool_call_cancellation() {
    // The old IR had no server-driven cancellation twin. The ordinary ABI oracle still requires the complete wire frame.
    let bytes =
        include_bytes!("../../testing/voice-conformance/fixtures/gemini/toolCallCancellation.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/toolCallCancellation").unwrap(),
        &serde_json::json!({"ids":["function-call-14582213604383"]})
    );
    fixture(
        3,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn gemini_tool_response() {
    let bytes = include_bytes!("../../testing/voice-conformance/fixtures/gemini/toolResponse.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/toolResponse").unwrap(),
        &serde_json::json!({"functionResponses":[{"id":"function-call-14582213604383","name":"get_weather","response":{"temp_c":18,"condition":"partly cloudy"}}]})
    );
    fixture(
        3,
        bytes,
        FROM_CALLER,
        &serde_json::to_vec(&value).unwrap(),
        &[],
    );
}

#[test]
fn gemini_usage_metadata() {
    let bytes =
        include_bytes!("../../testing/voice-conformance/fixtures/gemini/usageMetadata.json");
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value.pointer("/usageMetadata").unwrap(),
        &serde_json::json!({"promptTokenCount":140,"responseTokenCount":80,"totalTokenCount":220,"promptTokensDetails":[{"modality":"TEXT","tokenCount":40},{"modality":"AUDIO","tokenCount":100}],"responseTokensDetails":[{"modality":"AUDIO","tokenCount":80}]})
    );
    fixture(
        3,
        bytes,
        FROM_FAR_END,
        &serde_json::to_vec(&value).unwrap(),
        &[(0, 100), (1, 80), (2, 40)],
    );
}

#[test]
fn openai_captured_transcript_preserves_order_and_semantics() {
    let text = include_str!("../../testing/voice-conformance/fixtures/openai/transcript.jsonl");
    let transcript: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let frames: Vec<_> = transcript
        .iter()
        .filter(|line| matches!(line["dir"].as_str(), Some("client" | "server")))
        .collect();
    assert_eq!(frames.len(), 24);
    let input: Vec<Value> = frames.iter().map(|line| line["event"].clone()).collect();
    for dropped in [false, true] {
        let dispatcher = Dispatcher::new(DispatchConfig::default());
        let plugin = abi::load(&dispatcher, "openai-transcript", dropped);
        assert_eq!(
            abi::open(&plugin, abi::SESSION, Some(abi::PUBLIC)).outcome,
            Outcome::Ready
        );
        let initial = abi::piece(&plugin, &abi::session(77, 5, FROM_CALLER, 0, &[]));
        assert_answer(&initial, 5);
        assert!(initial.to_far);
        assert_eq!(initial.target, "/v1/realtime?model=m-cap");
        let lock: Value = serde_json::from_slice(&initial.emitted).unwrap();
        assert_eq!(lock["type"], "session.update");
        assert_eq!(lock["session"]["type"], "realtime");
        assert_eq!(lock["session"]["max_output_tokens"], 4096);
        assert_eq!(
            lock["session"]["truncation"]["token_limits"]["post_instructions"],
            28672
        );
        let mut emitted = Vec::new();
        let mut far_seen = false;
        let mut final_units = Vec::new();
        for (index, frame) in frames.iter().enumerate() {
            let from = if frame["dir"] == "client" {
                FROM_CALLER
            } else {
                FROM_FAR_END
            };
            let first_far = from == FROM_FAR_END && !std::mem::replace(&mut far_seen, true);
            let bytes = serde_json::to_vec(&frame["event"]).unwrap();
            let expected = match index {
                0 => oracles::OPENAI_TRANSCRIPT_0.to_vec(),
                14 => oracles::OPENAI_TRANSCRIPT_14.to_vec(),
                _ => bytes.clone(),
            };
            let mut answer = abi::piece(
                &plugin,
                &abi::Piece {
                    status: if first_far { 101 } else { 0 },
                    ..abi::session(
                        77,
                        5,
                        from,
                        if first_far { PIECE_HAS_STATUS } else { 0 },
                        &bytes,
                    )
                },
            );
            let mut answers = Vec::new();
            let mut drained = false;
            for drain_index in 0..=64 {
                assert_answer(&answer, 5);
                if !answer.emitted.is_empty() {
                    answers.push((answer.to_far, answer.emitted.clone()));
                }
                if answer.done {
                    drained = true;
                    break;
                }
                if drain_index == 64 {
                    break;
                }
                answer = abi::piece(&plugin, &abi::session(77, 5, FROM_KERNEL, 0, &[]));
                assert_answer(&answer, 5);
                if answer.emitted.is_empty() && !answer.done {
                    drained = true;
                    break;
                }
            }
            assert!(drained);
            assert_eq!(answers, vec![(from == FROM_CALLER, expected)]);
            emitted.extend(
                answers
                    .into_iter()
                    .map(|(_, bytes)| serde_json::from_slice::<Value>(&bytes).unwrap()),
            );
            final_units = answer.units;
        }
        assert_eq!(
            final_units
                .iter()
                .filter(|u| u.0 < 4)
                .map(|u| (u.0, u.2))
                .collect::<Vec<_>>(),
            &[]
        );
        assert!(final_units.iter().any(|u| u.0 == 8 && u.2 == 1));
        // Fixed indices preserve the original semantic skeleton's temporal order,
        // including two speech-start events and matching tool call/result identity.
        for events in [&input, &emitted] {
            assert_eq!(
                events[0].pointer("/type").unwrap(),
                &serde_json::json!("session.update")
            );
            assert_eq!(
                events[1].pointer("/type").unwrap(),
                &serde_json::json!("session.created")
            );
            assert_eq!(
                events[3].pointer("/type").unwrap(),
                &serde_json::json!("input_audio_buffer.append")
            );
            assert_eq!(
                events[4].pointer("/type").unwrap(),
                &serde_json::json!("input_audio_buffer.speech_started")
            );
            assert_eq!(
                events[10].pointer("/type").unwrap(),
                &serde_json::json!("response.function_call_arguments.delta")
            );
            assert_eq!(
                events[11].pointer("/type").unwrap(),
                &serde_json::json!("response.function_call_arguments.done")
            );
            assert_eq!(
                events[13].pointer("/item/type").unwrap(),
                &serde_json::json!("function_call_output")
            );
            assert_eq!(
                events[16].pointer("/type").unwrap(),
                &serde_json::json!("response.audio.delta")
            );
            assert_eq!(
                events[19].pointer("/type").unwrap(),
                &serde_json::json!("input_audio_buffer.speech_started")
            );
            assert_eq!(
                events[21].pointer("/type").unwrap(),
                &serde_json::json!("response.cancel")
            );
            assert_eq!(events[10]["call_id"], "call_weather_abc");
            assert_eq!(events[11]["arguments"], "{\"city\":\"Paris\"}");
            assert_eq!(events[13]["item"]["call_id"], "call_weather_abc");
            assert_eq!(events[16]["delta"], "UklGRhwAAABXQVZFZm10IBAAAAABAAEA");
        }
    }
}

#[test]
fn gemini_captured_transcript_preserves_order_and_semantics() {
    let text = include_str!("../../testing/voice-conformance/fixtures/gemini/transcript.jsonl");
    let transcript: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let frames: Vec<_> = transcript
        .iter()
        .filter(|line| matches!(line["dir"].as_str(), Some("client" | "server")))
        .collect();
    assert_eq!(frames.len(), 14);
    let input: Vec<Value> = frames.iter().map(|line| line["event"].clone()).collect();
    for dropped in [false, true] {
        let dispatcher = Dispatcher::new(DispatchConfig::default());
        let plugin = abi::load(&dispatcher, "gemini-transcript", dropped);
        assert_eq!(
            abi::open(&plugin, abi::SESSION, Some(abi::PUBLIC)).outcome,
            Outcome::Ready
        );
        let initial = abi::piece(&plugin, &abi::session(77, 3, FROM_CALLER, 0, &[]));
        assert_answer(&initial, 3);
        assert!(initial.emitted.is_empty());
        let mut emitted = Vec::new();
        let mut far_seen = false;
        let mut final_units = Vec::new();
        for (index, frame) in frames.iter().enumerate() {
            let from = if frame["dir"] == "client" {
                FROM_CALLER
            } else {
                FROM_FAR_END
            };
            let first_far = from == FROM_FAR_END && !std::mem::replace(&mut far_seen, true);
            let bytes = serde_json::to_vec(&frame["event"]).unwrap();
            let expected = match index {
                0 => oracles::GEMINI_TRANSCRIPT_0.to_vec(),
                _ => bytes.clone(),
            };
            let mut answer = abi::piece(
                &plugin,
                &abi::Piece {
                    status: if first_far { 101 } else { 0 },
                    ..abi::session(
                        77,
                        3,
                        from,
                        if first_far { PIECE_HAS_STATUS } else { 0 },
                        &bytes,
                    )
                },
            );
            let mut answers = Vec::new();
            let mut drained = false;
            for drain_index in 0..=64 {
                assert_answer(&answer, 3);
                if !answer.emitted.is_empty() {
                    answers.push((answer.to_far, answer.emitted.clone()));
                }
                if answer.done {
                    drained = true;
                    break;
                }
                if drain_index == 64 {
                    break;
                }
                answer = abi::piece(&plugin, &abi::session(77, 3, FROM_KERNEL, 0, &[]));
                assert_answer(&answer, 3);
                if answer.emitted.is_empty() && !answer.done {
                    drained = true;
                    break;
                }
            }
            assert!(drained);
            assert_eq!(answers, vec![(from == FROM_CALLER, expected)]);
            emitted.extend(
                answers
                    .into_iter()
                    .map(|(_, bytes)| serde_json::from_slice::<Value>(&bytes).unwrap()),
            );
            final_units = answer.units;
        }
        assert_eq!(
            final_units
                .iter()
                .filter(|u| u.0 < 4)
                .map(|u| (u.0, u.2))
                .collect::<Vec<_>>(),
            &[(2, 140), (3, 80)]
        );
        assert!(final_units.iter().any(|u| u.0 == 8 && u.2 == 1));
        // Fixed indices preserve config/connect/audio/tool-open/args/close/result,
        // interrupted (speech-start), turn-complete (audio-done), then usage.
        for events in [&input, &emitted] {
            assert!(events[0]["setup"].is_object());
            assert_eq!(
                events[1].pointer("/setupComplete").unwrap(),
                &serde_json::json!({})
            );
            assert_eq!(
                events[2].pointer("/realtimeInput/audio/mimeType").unwrap(),
                &serde_json::json!("audio/pcm;rate=16000")
            );
            assert_eq!(
                events[5].pointer("/toolCall/functionCalls/0/name").unwrap(),
                &serde_json::json!("get_weather")
            );
            assert_eq!(
                events[5].pointer("/toolCall/functionCalls/0/args").unwrap(),
                &serde_json::json!({"city":"Paris"})
            );
            assert_eq!(
                events[6]
                    .pointer("/toolResponse/functionResponses/0/name")
                    .unwrap(),
                &serde_json::json!("get_weather")
            );
            assert_eq!(
                events[7]
                    .pointer("/serverContent/modelTurn/parts/0/inlineData/mimeType")
                    .unwrap(),
                &serde_json::json!("audio/pcm;rate=24000")
            );
            assert_eq!(
                events[11].pointer("/serverContent/interrupted").unwrap(),
                &serde_json::json!(true)
            );
            assert_eq!(
                events[12].pointer("/serverContent/turnComplete").unwrap(),
                &serde_json::json!(true)
            );
            assert_eq!(
                events[13]
                    .pointer("/usageMetadata/totalTokenCount")
                    .unwrap(),
                &serde_json::json!(220)
            );
            assert_eq!(
                events[5]["toolCall"]["functionCalls"][0]["id"],
                "function-call-14582213604383"
            );
            assert_eq!(
                events[6]["toolResponse"]["functionResponses"][0]["id"],
                "function-call-14582213604383"
            );
        }
    }
}
