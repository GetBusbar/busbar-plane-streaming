// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE DECLARES RIDE THE STATEMENT (ARCHITECT ruling 2026-10-21; BUSBAR-1.6.0.md l.246): the door's
//! Statement, read back as the loader reads a linked row, states this plugin's whole `declares`
//! document under `EXT_DECLARES`, byte for byte the `declares.json` the release signs.

/// The one declares file: the one the door's Statement carries.
const DECLARES: &[u8] = include_bytes!("../../plane-streaming/declares.json");

#[test]
fn the_statement_states_the_whole_declares_document() {
    let row = busbar_plugin_loader::dispatch::LinkedRow::of(busbar_plane_streaming::door).unwrap();
    let read = busbar_contract::abi::mechanism::rendering::read(&row.statement).unwrap();
    assert_eq!(
        busbar_contract::abi::mechanism::extensions::declares(&read.extensions.bytes),
        Some(DECLARES),
    );
}
