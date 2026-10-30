// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE PUBLISHED CONFORMANCE SUITE, run over this plane (OWNER 2026-10-03: plugins test themselves
//! against busbar's published suite). The linked door (`busbar_plane_streaming::door`) and this
//! crate's dropped-in image (the `busbar-plane-streaming-plugin` cdylib, which `cargo test` builds
//! beside the test), each through the one loader, driven by busbar-plugin-loader's plane script
//! over `conformance.json`, every step at its pinned crossing count, the two folds equal. This is the
//! plugin's own both-ways witness at the path every plugin repo keeps it (`tests/conformance.rs`).
//! `plugin-ci.yml` runs it under `--release`.
//!
//! No host connector is named: the plane dials nothing itself (its needs are the sockets the kernel
//! carries for its units), and the suite reads a door bound over a host as one that answers PENDING
//! on the network.

busbar_test_kit::conformance_suite! {
    door: busbar_plane_streaming::door,
    cdylib: "busbar_plane_streaming_plugin",
    inputs: include_str!("conformance.json"),
}
