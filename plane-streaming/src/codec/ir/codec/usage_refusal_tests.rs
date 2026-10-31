//! ITEM 133 (the duplex twin) — A PRESENT-BUT-UNREADABLE BILLED COUNT BILLS THE FLOOR (#42; owner
//! ruling 2026-10-27).
//!
//! Both duplex dialects read every billed count as `read_count_u64(..).unwrap_or_default()`, so a
//! stringified count metered a real turn at ZERO. Each test drives the real `meter_down` path: an
//! unreadable count now bills the llm plane's floor over the report's bytes (one output token per
//! four bytes, at least one), marked as the floor, never a usage report carrying zero; an ABSENT
//! count keeps meaning zero.
use super::gemini::GeminiLiveCodec;
use super::*;
use serde_json::json;

fn wire(v: &Value) -> Vec<u8> {
    v.to_string().into_bytes()
}

/// Whether `ir` is one usage report billing the floor over a report of `src`'s bytes.
fn floored(ir: &[Metered], src: &Value) -> bool {
    let floor = (src.to_string().len() as u64 / 4).max(1);
    matches!(ir, [Metered::Usage(u)] if u.floor && u.text_out == floor
        && (u.audio_in, u.audio_out, u.text_in, u.image_in, u.video_in, u.cached) == (0, 0, 0, 0, 0, 0))
}

#[test]
fn openai_realtime_bills_the_floor_for_a_stringified_count() {
    for usage in [
        json!({"input_tokens": "1500", "output_tokens": 9}),
        json!({"input_tokens": 10, "output_tokens": 9, "input_token_details": {"audio_tokens": "8"}}),
        json!({"input_tokens": 10, "output_tokens": 9, "input_token_details": {"cached_tokens": "3"}}),
    ] {
        let src = json!({"type": "response.done", "response": {"usage": usage}});
        let ir = OpenAiRealtimeCodec.meter_down(&wire(&src));
        assert!(
            floored(&ir, &src),
            "{usage} bills the floor, never zero: {ir:?}"
        );
    }
}

#[test]
fn openai_realtime_absent_counts_still_read_as_zero() {
    let src = json!({"type": "response.done", "response": {"usage": {"output_tokens": 9}}});
    let ir = OpenAiRealtimeCodec.meter_down(&wire(&src));
    let [Metered::Usage(u)] = ir.as_slice() else {
        panic!("expected Usage, got {ir:?}")
    };
    assert_eq!((u.text_in, u.text_out, u.cached), (0, 9, 0));
}

#[test]
fn gemini_live_bills_the_floor_for_a_stringified_count() {
    for um in [
        json!({"promptTokenCount": "95", "responseTokenCount": 50}),
        json!({"promptTokenCount": 95, "responseTokenCount": 50, "cachedContentTokenCount": "5"}),
        json!({"promptTokensDetails": [{"modality": "AUDIO", "tokenCount": "80"}]}),
    ] {
        let src = json!({"usageMetadata": um});
        let ir = GeminiLiveCodec.meter_down(&wire(&src));
        assert!(
            floored(&ir, &src),
            "{um} bills the floor, never zero: {ir:?}"
        );
    }
}

#[test]
fn gemini_live_absent_counts_still_read_as_zero() {
    let src = json!({"usageMetadata": {"responseTokenCount": 50}});
    let ir = GeminiLiveCodec.meter_down(&wire(&src));
    let [Metered::Usage(u)] = ir.as_slice() else {
        panic!("expected Usage, got {ir:?}")
    };
    assert_eq!((u.text_in, u.text_out, u.cached), (0, 50, 0));
}

/// RED (audit plane-streaming round 52b0f93ac9, S57): a usage report that is PRESENT but is not an
/// object (a string, a number, an array) is unreadable usage on a delivered turn: it bills the
/// floor and says so, never an all-zero report. A JSON `null` is no report, as an absent one is.
#[test]
fn a_present_usage_that_is_not_an_object_bills_the_floor_in_both_dialects() {
    for usage in [json!("n/a"), json!(42), json!([1, 2])] {
        let src = json!({"type": "response.done", "response": {"usage": usage}});
        let ir = OpenAiRealtimeCodec.meter_down(&wire(&src));
        assert!(
            floored(&ir, &src),
            "OpenAI usage {usage} bills the floor: {ir:?}"
        );
        let src = json!({"usageMetadata": usage});
        let ir = GeminiLiveCodec.meter_down(&wire(&src));
        assert!(
            floored(&ir, &src),
            "Gemini usageMetadata {usage} bills the floor: {ir:?}"
        );
    }
    let src = json!({"type": "response.done", "response": {"usage": null}});
    let ir = OpenAiRealtimeCodec.meter_down(&wire(&src));
    assert!(
        matches!(ir.as_slice(), [Metered::Usage(u)] if !u.floor && *u == IrDuplexUsage::default()),
        "a null OpenAI usage is no report: {ir:?}"
    );
    let ir = GeminiLiveCodec.meter_down(&wire(&json!({"usageMetadata": null})));
    assert!(
        ir.iter()
            .all(|m| matches!(m, Metered::Usage(u) if !u.floor)),
        "a null Gemini usageMetadata bills no floor: {ir:?}"
    );
}
