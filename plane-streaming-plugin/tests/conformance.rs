// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The published conformance suite, run by this streaming plane plugin.

busbar_test_kit::conformance_suite! {
    door: busbar_plane_streaming::door,
    cdylib: "busbar_plane_streaming_plugin",
    inputs: include_str!("conformance.json"),
}
