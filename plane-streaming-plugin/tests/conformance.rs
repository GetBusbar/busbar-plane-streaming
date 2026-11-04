// SPDX-License-Identifier: Apache-2.0
//! Admission and malformed metadata tests against the actual streaming plane.
use busbar_plugin_loader::dispatch::kinds::plane::Plane;
use busbar_plugin_loader::dispatch::{
    load_dropped, load_linked, rendering_of_library, Bind, ConnTable, DispatchConfig, Dispatcher,
    LinkedRow, NoSink, Plugin,
};
use std::sync::Arc;
#[path = "support/artifact.rs"]
mod artifact;
#[path = "support/door_refusals.rs"]
mod door_refusals;
#[path = "support/host_refusals.rs"]
mod host_refusals;
type Loaded = (Plugin<Plane>, Arc<Dispatcher>);
fn bind(d: &Dispatcher) -> Bind {
    Bind {
        instance: Arc::from("streaming-conformance"),
        max_inflight_cap: 64,
        sink: Arc::new(NoSink),
        dispatcher: d.adopter(),
        conns: ConnTable::Probe,
    }
}
fn cdylib() -> std::path::PathBuf {
    artifact::plugin_cdylib()
}
fn packed_rendering(lib: &std::path::Path) -> Vec<u8> {
    rendering_of_library(lib)
        .expect("actual artifact renders")
        .expect("actual artifact exports door")
}
fn linked() -> Loaded {
    let d = Arc::new(Dispatcher::new(DispatchConfig::default()));
    let row = LinkedRow::of(busbar_plane_streaming::door).unwrap();
    let p = load_linked::<Plane>(&row, bind(&d)).unwrap();
    (p, d)
}
fn dropped(lib: &std::path::Path, stated: &[u8]) -> Loaded {
    let d = Arc::new(Dispatcher::new(DispatchConfig::default()));
    let p = load_dropped::<Plane>(lib, stated, bind(&d)).unwrap();
    (p, d)
}
#[test]
fn actual_cdylib_and_linked_streaming_statements_match() {
    let lib = cdylib();
    let stated = packed_rendering(&lib);
    assert_eq!(
        stated,
        LinkedRow::of(busbar_plane_streaming::door)
            .unwrap()
            .statement
    );
    let (l, _) = linked();
    let (r, _) = dropped(&lib, &stated);
    assert_eq!(l.name(), r.name());
    assert_eq!(l.kind(), r.kind());
    assert_eq!(l.max_inflight(), r.max_inflight());
    #[cfg(debug_assertions)]
    assert!(
        std::env::var_os("BUSBAR_EXPECT_RELEASE").is_none(),
        "release profile requested"
    );
}

#[test]
fn declared_network_schemes_match_the_actual_streaming_statement() {
    let row = LinkedRow::of(busbar_plane_streaming::door).unwrap();
    let statement = busbar_contract::abi::mechanism::rendering::read(&row.statement).unwrap();
    let declares: serde_json::Value =
        serde_json::from_str(include_str!("../../plane-streaming/declares.json")).unwrap();
    let mut expected: Vec<_> = declares["needs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    expected.sort();
    expected.dedup();
    let mut actual: Vec<_> = statement
        .needs
        .iter()
        .filter(|n| !n.transport.is_empty())
        .map(|n| n.transport.as_str())
        .collect();
    actual.sort();
    actual.dedup();
    assert_eq!(actual, expected);
}
