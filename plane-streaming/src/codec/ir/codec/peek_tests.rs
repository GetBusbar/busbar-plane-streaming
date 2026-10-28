// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The meter's reads answer what the whole-frame reads answer: the same audio lengths, the same
//! refusals, and no far-end frame left unread that carries a usage report, an error or a tool
//! call's open.

use super::gemini::GeminiLiveCodec;
use super::peek::{base64_len, Lite};
use super::*;

#[test]
fn the_counted_length_is_the_decoded_length_and_refuses_what_the_decode_refuses() {
    for b64 in [
        "",
        "Q",
        "QQ",
        "QQ=",
        "QQ==",
        "QUI",
        "QUI=",
        "QUJD",
        "QUJDRA",
        "QUJDRA==",
        "QUJDREU",
        " QU JD\n",
        "QQ==QQ==",
        "QQ= =",
        "QQ=x",
        "====",
        "A===",
        "Q!==",
        "QUJD\u{e9}",
        "QU+/",
        "QU-_",
        "\tQ\rQ\n",
    ] {
        assert_eq!(
            base64_len(b64),
            decode_audio(b64).map(|b| b.len()),
            "{b64:?}"
        );
    }
}

/// Every frame either dialect is asked about below, both directions, well-formed and not.
fn frames() -> Vec<String> {
    let audio = busbar_contract::media::base64_encode(&[7_u8; 301]);
    [
        format!(r#"{{"type":"input_audio_buffer.append","audio":"{audio}"}}"#),
        format!(r#"{{"type":"input_audio_buffer.append","audio":"{audio}","event_id":"e1"}}"#),
        r#"{"type":"input_audio_buffer.append","audio":"QU\/D"}"#.to_string(),
        r#"{"type":"input_audio_buffer.append","audio":"QUJD","audio":"QQ=="}"#.to_string(),
        r#"{"type":"input_audio_buffer.append"}"#.to_string(),
        r#"{"type":"input_audio_buffer.append","audio":"Q"}"#.to_string(),
        r#"{"type":"input_audio_buffer.append","audio":7}"#.to_string(),
        r#"{"type":"input_audio_buffer.append","audio":"QUJD","x":"\ud800"}"#.to_string(),
        r#"{"type":"input_audio_buffer.append","audio":"QUJD"}"#.to_string(),
        r#"{"type":1,"type":"input_audio_buffer.append","audio":"QUJD"}"#.to_string(),
        r#"{"type":"input_audio_buffer.append","type":1,"audio":"QUJD"}"#.to_string(),
        r#"[{"type":"input_audio_buffer.append","audio":"QUJD"}]"#.to_string(),
        r#"{"type":"input_audio_buffer.append","audio":"QUJD"} x"#.to_string(),
        format!(r#"{{"type":"response.output_audio.delta","delta":"{audio}","item_id":"i"}}"#),
        r#"{"type":"response.audio.delta","delta":"!!"}"#.to_string(),
        r#"{"type":"response.done","response":{"usage":{"input_tokens":3,"output_tokens":4}}}"#
            .to_string(),
        r#"{"type":"response.done","response":{"usage":{"input_tokens":"x"}}}"#.to_string(),
        r#"{"type":"response.done","type":"response.output_audio.delta"}"#.to_string(),
        r#"{"type":"response.output_audio.delta","type":"response.done"}"#.to_string(),
        r#"{"type":"response.output_audio.delta","type":{"a":1}}"#.to_string(),
        r#"{"type":"error","error":{"code":"c","message":"m"}}"#.to_string(),
        r#"{"type":"error","error":{}}"#.to_string(),
        r#"{"type":"response.output_item.added","item":{"type":"function_call","call_id":"c","name":"n"}}"#
            .to_string(),
        r#"{"type":"session.update","session":{"instructions":"x"}}"#.to_string(),
        r#"{"type":"error","error":{"message":"\ud800"}}"#.to_string(),
        format!(r#"{{"realtimeInput":{{"audio":{{"mimeType":"audio/pcm;rate=16000","data":"{audio}"}}}}}}"#),
        r#"{"realtimeInput":{"audio":{"mimeType":"audio/pcm;rate=24000","data":"QUJD"},"mediaChunks":[{"mimeType":"audio/pcm","data":"QQ=="}]}}"#.to_string(),
        r#"{"realtimeInput":{"mediaChunks":[{"mimeType":"audio/pcm","data":"QQ=="},{"mimeType":"image/png","data":"QUJD"},{"mimeType":"audio/pcm;rate=24000","data":"QUJDRA"}]}}"#.to_string(),
        r#"{"realtimeInput":{"audio":null,"mediaChunks":[{"mimeType":"audio/pcm","data":"QQ=="}]}}"#.to_string(),
        r#"{"realtimeInput":{"audio":{"mimeType":"audio/pcm","data":"Q"}}}"#.to_string(),
        r#"{"realtimeInput":{"audio":{"mimeType":"audio/pcm","data":"QQ=="}},"setup":{}}"#
            .to_string(),
        r#"{"realtimeInput":{"audioStreamEnd":true}}"#.to_string(),
        format!(r#"{{"serverContent":{{"modelTurn":{{"parts":[{{"inlineData":{{"mimeType":"audio/pcm;rate=24000","data":"{audio}"}}}}]}}}}}}"#),
        r#"{"serverContent":{"turnComplete":true},"usageMetadata":{"promptTokenCount":5}}"#
            .to_string(),
        r#"{"usageMetadata":{"promptTokenCount":"x"}}"#.to_string(),
        r#"{"toolCall":{"functionCalls":[{"id":"c","name":"n","args":{}}]}}"#.to_string(),
        r#"{"usageMetadata":{"promptTokenCount":5}}"#.to_string(),
        r#"{"setupComplete":{},"usageMetadata":{"promptTokenCount":5}}"#.to_string(),
        "not json".to_string(),
        String::new(),
    ]
    .into()
}

fn metered(evs: &[IrServerEvent]) -> bool {
    evs.iter().any(|e| {
        matches!(
            e,
            IrServerEvent::Usage(_)
                | IrServerEvent::Error { .. }
                | IrServerEvent::Tool(IrDuplexTool::CallOpen { .. })
        )
    })
}

fn agrees<C: DuplexReader>(codec: &C) {
    let mut frames: Vec<Vec<u8>> = frames().into_iter().map(String::into_bytes).collect();
    frames.push(
        b"{\"type\":\"input_audio_buffer.append\",\"audio\":\"QUJD\",\"x\":\"\xff\"}".to_vec(),
    );
    frames.push(b"{\"type\":\"response.output_audio.delta\",\"delta\":\"\xff\"}".to_vec());
    for frame in &frames {
        let shown = String::from_utf8_lossy(frame);
        let down = codec.read_down_ref(WireRef(frame), &mut DecodeState::default());
        assert!(
            codec.meters_down(frame) || !metered(&down),
            "{shown}: left unread, yet it carries {down:?}"
        );
        let read = Lite::read(frame);
        assert_eq!(
            read.is_some(),
            serde_json::from_slice::<Value>(frame).is_ok(),
            "{shown}"
        );
        let mut counted = Vec::new();
        if let Some(top) = &read {
            codec.uplink_audio(top, &mut |format, len| counted.push((format, len)));
        }
        let whole: Vec<_> = codec
            .read_up_ref(WireRef(frame), &mut DecodeState::default())
            .into_iter()
            .filter_map(|e| match e {
                IrClientEvent::AudioFrame(f) => Some((f.format, f.media.len())),
                _ => None,
            })
            .collect();
        assert_eq!(counted, whole, "{shown}");
    }
}

#[test]
fn the_meters_reads_agree_with_the_whole_frame_reads_on_both_dialects() {
    agrees(&OpenAiRealtimeCodec);
    agrees(&GeminiLiveCodec);
}
