// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE LADDER RED for `crates/busbar-plane-streaming/src/plane_door.rs`: the plane's lines on a guest
//! list reproduce the served plane's dispatch ladder exactly (`BUSBAR-1.6.0.md` section 6, "Auth
//! points and guest lists", step 3). Every served route resolves to the same (dialect, door), and
//! a request no served route took resolves to no line. A request whose path a line matches under
//! another method is answered 405 with that line's Allow, as the served plane answered it.
//!
//! The match here is the spec's, stated in the test: a line matches when its method set holds the
//! method and its path does (exact bytes, or a pattern where `{…}` is exactly one non-empty
//! segment); an exact path ranks above a pattern, and two matching lines at equal rank are the
//! ambiguity boot refuses.

use super::*;
use crate::driven::Door;

/// The rank of a line whose path matches `path`; `None` when it does not match.
fn path_rank(line: &Route, path: &str) -> Option<u8> {
    if line.exact() {
        return (line.target == path).then_some(2);
    }
    let want: Vec<&str> = line.target.split('/').collect();
    let got: Vec<&str> = path.split('/').collect();
    if want.len() != got.len() {
        return None;
    }
    let all = want.iter().zip(&got).all(|(w, g)| {
        if w.starts_with('{') && w.ends_with('}') {
            !g.is_empty()
        } else {
            w == g
        }
    });
    all.then_some(1)
}

/// The rank of a line that matches (`method`, `path`); `None` when it does not match.
fn rank(line: &Route, method: &str, path: &str) -> Option<u8> {
    if line.verb != method {
        return None;
    }
    path_rank(line, path)
}

/// The methods the lines whose path matches `path` take: the 405's Allow, empty when none does.
fn allow(path: &str) -> Vec<&'static str> {
    ROUTES
        .iter()
        .filter(|l| path_rank(l, path).is_some())
        .map(|l| l.verb)
        .collect()
}

/// The line `(method, path)` resolves to, by index; `Err` on an equal-rank ambiguity.
fn resolve(method: &str, path: &str) -> Result<Option<usize>, String> {
    let mut best: Option<(u8, usize)> = None;
    let mut tied = false;
    for (i, line) in ROUTES.iter().enumerate() {
        let Some(r) = rank(line, method, path) else {
            continue;
        };
        match best {
            Some((b, _)) if r < b => {}
            Some((b, _)) if r == b => tied = true,
            _ => {
                best = Some((r, i));
                tied = false;
            }
        }
    }
    if tied {
        return Err(format!("{method} {path} matches two lines at equal rank"));
    }
    Ok(best.map(|(_, i)| i))
}

/// The served plane's routes, as its mount table and its one telephony claim answer them.
const SERVED: &[(&str, &str, Door, u32, &str)] = &[
    (
        "POST",
        "/v1/realtime/client_secrets",
        Door::Mint,
        0,
        KEY_AUTH,
    ),
    ("POST", "/v1/realtime/calls", Door::Sdp, 0, KEY_AUTH),
    (
        "GET",
        "/v1/realtime/sideband/rtc_abc",
        Door::Sideband,
        0,
        KEY_AUTH,
    ),
    (
        "GET",
        "/v1/realtime/gemini/call-1",
        Door::Gemini,
        1,
        KEY_AUTH,
    ),
    ("GET", "/twilio/CA123", Door::Twilio, 2, SIGNATURE_AUTH),
    ("GET", "/v1/realtime", Door::Realtime, 0, KEY_AUTH),
];

#[test]
fn every_served_route_resolves_to_the_same_dialect_and_door() {
    for &(method, path, door, dialect, auth) in SERVED {
        let i = resolve(method, path)
            .expect("no ambiguity")
            .unwrap_or_else(|| panic!("{method} {path} resolves to no line"));
        assert_eq!(Door::of(i as u32), Some(door), "{method} {path}");
        assert_eq!(ROUTES[i].dialect, dialect, "{method} {path}");
        assert_eq!(ROUTES[i].refusal_dialect, dialect, "{method} {path}");
        assert_eq!(ROUTES[i].auth, auth, "{method} {path}");
        assert_eq!(door.dialect(), dialect, "the door's dialect is its line's");
    }
    assert_eq!(SERVED.len(), ROUTES.len(), "every line is a served route");
}

#[test]
fn a_request_no_served_route_took_resolves_to_no_line() {
    for (method, path) in [
        ("GET", "/v1/realtime/"),
        ("GET", "/v1/realtime/sideband"),
        ("GET", "/v1/realtime/sideband/"),
        ("GET", "/v1/realtime/sideband/a/b"),
        ("GET", "/v1/realtime/telephony/CA123"),
        ("GET", "/twilio"),
        ("GET", "/twilio/"),
        ("GET", "/twilio/a/b"),
        ("GET", "/twiliofoo/x"),
        ("POST", "/v1/audio/transcriptions"),
    ] {
        assert_eq!(resolve(method, path), Ok(None), "{method} {path}");
        assert!(
            allow(path).is_empty(),
            "{method} {path} is no-route, not 405"
        );
    }
}

#[test]
fn a_served_path_under_another_method_is_405_with_the_lines_allow() {
    for (method, path, allowed) in [
        ("GET", "/v1/realtime/client_secrets", "POST"),
        ("GET", "/v1/realtime/calls", "POST"),
        ("POST", "/v1/realtime/sideband/rtc_abc", "GET"),
        ("POST", "/v1/realtime/gemini/call-1", "GET"),
        ("POST", "/twilio/CA123", "GET"),
        ("POST", "/v1/realtime", "GET"),
    ] {
        assert_eq!(
            resolve(method, path),
            Ok(None),
            "{method} {path} takes no line"
        );
        assert_eq!(
            allow(path),
            [allowed],
            "{method} {path} is 405, Allow: {allowed}"
        );
    }
}

#[test]
fn the_sockets_are_upgrade_lines_and_the_one_request_doors_are_not() {
    for (i, line) in ROUTES.iter().enumerate() {
        let door = Door::of(i as u32).expect("a door per line");
        // The SDP door opens a session over a datagram carrier (row 132): a session, not an
        // upgrade line.
        let datagram = door == Door::Sdp;
        assert_eq!(
            line.upgrade,
            door.is_session() && !datagram,
            "{}",
            line.target
        );
        assert_eq!(
            line.carrier,
            if line.upgrade {
                WS_TRANSPORT
            } else if datagram {
                crate::claims::WEBRTC_TRANSPORT
            } else {
                HTTP_TRANSPORT
            },
            "an upgrade line hands the connection to the socket transport; the SDP door's \
             session rides the datagram carrier"
        );
    }
}

#[test]
fn no_two_lines_tie_and_each_lines_own_witness_reaches_it() {
    for (i, line) in ROUTES.iter().enumerate() {
        let witness: String = line
            .target
            .split('/')
            .map(|seg| if seg.starts_with('{') { "w" } else { seg })
            .collect::<Vec<_>>()
            .join("/");
        assert_eq!(
            resolve(line.verb, &witness),
            Ok(Some(i)),
            "{} {}",
            line.verb,
            witness
        );
    }
}

#[test]
fn no_line_is_open_and_the_telephony_line_is_signed() {
    use busbar_contract::abi::plane::CLAIM_OPEN;
    let open: Vec<_> = ROUTES
        .iter()
        .filter(|r| {
            ![crate::plane_door::KEY_AUTH, SIGNATURE_AUTH].contains(&r.auth)
                || r.claim().flags & CLAIM_OPEN != 0
        })
        .map(|r| r.target)
        .collect();
    assert!(
        open.is_empty(),
        "no door is read without a credential: the metadata path is not claimed (1.5.5 parity)"
    );
    let signed: Vec<_> = ROUTES
        .iter()
        .filter(|r| r.auth == SIGNATURE_AUTH)
        .map(|r| r.target)
        .collect();
    assert_eq!(signed, ["/twilio/{call_id}"]);
}

/// A `{call_id}` route is claimed as a PATTERN, not as a one-level prefix. Without `CLAIM_PATTERN`
/// the published claim reads as the prefix `/twilio/{call_id}` (one segment under it), which also
/// catches a path the route was never meant to carry. RED arms: every pattern route carries
/// `CLAIM_PATTERN` and never `CLAIM_EXACT`; every exact route the reverse; and the neighbouring
/// path the prefix reading would have caught (`/twilio/{call_id}/extra`-shaped, one more level)
/// is not matched by the pattern.
#[test]
fn a_call_id_route_is_claimed_as_a_pattern_and_a_neighbour_is_not() {
    use busbar_contract::abi::plane::{CLAIM_EXACT, CLAIM_PATTERN};
    let mut patterns = 0;
    for r in ROUTES {
        let flags = r.claim().flags;
        if r.target.contains('{') {
            patterns += 1;
            assert_eq!(
                flags & CLAIM_PATTERN,
                CLAIM_PATTERN,
                "{} is a pattern",
                r.target
            );
            assert_eq!(flags & CLAIM_EXACT, 0, "{} is not exact", r.target);
        } else {
            assert_eq!(flags & CLAIM_EXACT, CLAIM_EXACT, "{} is exact", r.target);
            assert_eq!(flags & CLAIM_PATTERN, 0, "{} is not a pattern", r.target);
        }
    }
    assert_eq!(
        patterns, 3,
        "the sideband, Gemini and Twilio sockets are the pattern routes"
    );
    // The neighbour: one more segment than the pattern has is no match (a prefix reading of
    // `/twilio` + one level would not catch it either, but `/twilio/x/extra` would be caught by a
    // looser prefix; the pattern pins the segment count).
    let twilio = ROUTES
        .iter()
        .find(|r| r.target == "/twilio/{call_id}")
        .expect("twilio route");
    assert!(path_rank(twilio, "/twilio/abc").is_some());
    assert!(path_rank(twilio, "/twilio/abc/extra").is_none());
    assert!(
        path_rank(twilio, "/twilio/").is_none(),
        "an empty segment is no call id"
    );
}
