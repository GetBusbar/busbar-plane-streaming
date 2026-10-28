// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The meter's frame reads answer as the whole-frame reads they stand in for: the base64 length is
//! the decoded length with the decode's refusals, and the borrowed read accepts exactly the frames a
//! `serde_json::Value` read accepts.

use super::peek::{base64_len, Head, Lite};
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
            busbar_contract::media::base64_decode(b64).map(|b| bytes::Bytes::from_owner(b).len()),
            "{b64:?}"
        );
    }
}

#[test]
fn the_borrowed_read_accepts_what_a_whole_frame_read_accepts_and_keeps_the_last_member() {
    let frames: [&[u8]; 9] = [
        br#"{"type":"a","audio":"QU\/D","x":[1,{"y":null}],"z":1.5e3}"#,
        br#"{"type":"a","x":"\ud800"}"#,
        br#"{"a":1} x"#,
        br#"[1,2]"#,
        b"{\"a\":\"\xff\"}",
        br#"{"type":1,"type":"b"}"#,
        br#"{"type":"c"}"#,
        br#"not json"#,
        br#""#,
    ];
    for frame in frames {
        let shown = String::from_utf8_lossy(frame);
        assert_eq!(
            Lite::read(frame).is_some(),
            serde_json::from_slice::<Value>(frame).is_ok(),
            "{shown}"
        );
        if let (Some(lite), Ok(v)) = (Lite::read(frame), serde_json::from_slice::<Value>(frame)) {
            assert_eq!(lite.str_at("type"), str_at(&v, "type"), "{shown}");
            assert_eq!(lite.str_at("audio"), str_at(&v, "audio"), "{shown}");
        }
    }
    assert_eq!(
        Head::read(br#"{"type":"a","delta":"QUJD","type":"b"}"#).and_then(|h| h.ty),
        Some("b".into()),
        "the head names the last `type` written, as a whole-frame read does"
    );
}
