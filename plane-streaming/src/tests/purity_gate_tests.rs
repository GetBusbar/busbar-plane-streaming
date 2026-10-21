//! Purity gates (moved from `tests/purity.rs`; they reach the plane's own types, which only the door
//! leaves this crate): the plane is `Copy`, shareable and pure over its configuration, and is
//! registered under its own key, never "voice". The source scans stay in `tests/purity.rs`.

use crate::claims::Dialect;
use crate::StreamingPlane;
use crate::Upstream;
use busbar_contract::ids::LaneId;

/// `StreamingPlane` derives `Copy`. Every interior-mutable cell (`Cell`, `RefCell`, `Mutex`,
/// `OnceLock`, ...) is `!Copy`, so a type that IS `Copy` structurally cannot hold one: a compile-time
/// proof, not a convention, that the plane keeps no mutable state of its own across calls. Everything
/// that varies across a session lives in the kernel-held `PlaneSessionState` the adapter opens.
const fn assert_copy<T: Copy>() {}
const _: () = assert_copy::<StreamingPlane>();

/// A plane is shared across every concurrent connection the kernel drives, so it must be `Send + Sync`;
/// it is held for the life of the program, so it must be `'static`. Asserted structurally.
const fn assert_shareable<T: Send + Sync + 'static>() {}
const _: () = assert_shareable::<StreamingPlane>();

/// Two planes built from the same upstream list compare equal, and looking up the same dialect twice
/// gives the same answer — a pure function of `self.upstreams()` and the argument.
#[test]
fn same_configuration_answers_the_same_way_every_time() {
    static UPSTREAMS: &[Upstream] = &[Upstream {
        lane: LaneId::new("realtime"),
        host: "api.openai.example",
        dialect: Dialect::OpenaiRealtime,
    }];
    let a = StreamingPlane::new(UPSTREAMS);
    let b = StreamingPlane::new(UPSTREAMS);
    assert_eq!(a, b);
    assert_eq!(
        a.upstream_for_dialect(Dialect::OpenaiRealtime),
        b.upstream_for_dialect(Dialect::OpenaiRealtime)
    );
    assert_eq!(a.upstream_for_dialect(Dialect::GeminiLive), None);
}

/// A plane with nothing configured answers every dialect lookup with `None` rather than panicking or
/// fabricating a host — the honest answer for a plane with no upstream.
#[test]
fn empty_plane_names_no_upstream() {
    assert!(StreamingPlane::EMPTY
        .upstream_for_dialect(Dialect::OpenaiRealtime)
        .is_none());
    assert_eq!(StreamingPlane::EMPTY, StreamingPlane::default());
}

/// The plane is registered under its own KEY, and never under "voice": the plane is streaming and the
/// realtime voice dialect is one dialect under it.
#[test]
fn the_plane_key_is_streaming_never_voice() {
    use busbar_contract::plane::PlaneMeta;
    use busbar_contract::plugin::Plugin;
    assert_eq!(<StreamingPlane as PlaneMeta>::KEY, "streaming");
    assert_eq!(StreamingPlane::EMPTY.key(), "streaming");
    assert_ne!(<StreamingPlane as PlaneMeta>::KEY, "voice");
}
