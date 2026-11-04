// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The voice duplex codecs: the pure half of the voice protocol plugin.

/// The registry key the streaming plane is known by: the string the composition root flips on when registering this plane.
pub const PLANE_KEY: &str = "streaming";

pub mod ir;

/// The one topology module that is a GRAMMAR rather than a dial: it keeps its `topology::` parent for the carrier envelope.
pub mod topology {
    pub mod twilio;
}
