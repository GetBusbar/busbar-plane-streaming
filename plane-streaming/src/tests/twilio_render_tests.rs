//! Twilio media rendering preserves the serializer's wire bytes.

use crate::codec::topology::twilio::TwilioEnvelope;

/// The identifier a `start` event binds, in the shape this dialect mints them.
const SID: &str = "MZ18ad3ab5a668481ce02b83e7395059f0";

/// One 20 ms µ-law frame at eight kilohertz.
fn frame_bytes() -> Vec<u8> {
    (0..160u16).map(|i| (i % 251) as u8).collect()
}

/// The bytes the serializer produced, restated as the reference the renderer is held to.
fn reference(stream_sid: &str, mulaw: &[u8]) -> Vec<u8> {
    #[derive(serde::Serialize)]
    struct Payload {
        payload: String,
    }
    #[derive(serde::Serialize)]
    struct Media<'a> {
        event: &'static str,
        #[serde(rename = "streamSid")]
        stream_sid: &'a str,
        media: Payload,
    }
    serde_json::to_vec(&Media {
        event: "media",
        stream_sid,
        media: Payload {
            payload: base64(mulaw),
        },
    })
    .expect("the document serializes")
}

/// Standard base64, spelled out here so the reference owes the renderer nothing.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// The rendered envelope is the serializer's, byte for byte.
#[test]
fn the_rendered_envelope_is_the_one_the_serializer_produced() {
    let payloads: Vec<Vec<u8>> = vec![
        Vec::new(),
        vec![0x7f],
        vec![0x7f, 0x00],
        vec![0x7f, 0x00, 0xff],
        frame_bytes(),
    ];
    for sid in [SID, "", "a\"b\\c", "sid with spaces", "sid-ünïcode"] {
        for payload in &payloads {
            let mut out = Vec::new();
            TwilioEnvelope::encode_media_into(&mut out, sid, payload);
            assert_eq!(
                out,
                reference(sid, payload),
                "the envelope for {sid:?} with {} payload bytes moved",
                payload.len()
            );
            assert_eq!(
                TwilioEnvelope::encode_media(sid, payload),
                out,
                "the owning form and the rendering form disagree for {sid:?}"
            );
            let expected = out.clone();
            TwilioEnvelope::encode_media_into(&mut out, sid, payload);
            assert_eq!(
                out, expected,
                "a reused buffer renders the same bytes a fresh one does"
            );
        }
    }
}
