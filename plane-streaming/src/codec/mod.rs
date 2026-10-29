// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The voice duplex codecs: the pure half of the voice protocol plugin.
//!
//! They hold the session config, the audio formats and usage a session meters, the meter's reads
//! of each dialect and the Twilio Media Streams grammar. A plane is a pure kind whose whole
//! transitive closure is scanned, so these codecs link no runtime (no socket, HTTP client or
//! socket-capable async runtime).
//!
//! They name only the plugin contract (`busbar-contract`, for the base64 media transcode, the
//! billing carrier and its reserved unit names) plus serde and `bytes`.

/// The registry key the streaming plane is known by: the string the composition root flips
/// onto the unified kernel loop's session admit
/// (`busbar_kernel::plane_host::register_session_runner`) and the same string the voice session
/// gauntlet reports from its `GauntletPlane::capability_key`.
///
/// Named ONCE, here, on the pure side of the split, because the plane's `capability_key` and the
/// composition-root FLIP must reference the SAME literal or a swap could drift onto two.
pub const PLANE_KEY: &str = "streaming";

pub mod ir;

/// The one topology module that is a GRAMMAR rather than a dial: it keeps its `topology::` parent
/// so its in-crate path is the one the runtime half still spells, and the move is invisible to
/// every caller.
///
/// UNGATED. This crate declares NO features on purpose (a plane compiled two ways is two planes),
/// so there is no feature to gate this on — and none is needed: the grammar is a total function
/// from a frame to an IR event and back, pulling nothing beyond `serde`/`bytes` (both already
/// unconditional here), so making it unconditional adds no dependency and no I/O to this pure
/// kind's closure. The legacy engine's own `#[cfg(feature = "runtime")]` on the module that
/// re-exports it is what keeps that engine's Twilio path OFF by default — this module simply exists
/// for it to reach, always, the same way `codec::ir` always does.
pub mod topology {
    pub mod twilio;
}
