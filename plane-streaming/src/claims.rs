//! The transports the door's routes are carried on ([`WS_TRANSPORT`], [`HTTP_TRANSPORT`],
//! [`WEBRTC_TRANSPORT`]), as the plane's needs and its routes (`crate::plane_door::ROUTES`) name
//! them.

/// The transport the three WebSocket dialects (`openai-realtime`, `gemini-live`,
/// `twilio-media-streams`) are carried on: every socket door, and the far end's sockets.
pub const WS_TRANSPORT: &str = "ws";

/// The transport the one plain-request door is claimed against: the ephemeral-secret mint
/// (`POST /v1/realtime/client_secrets`).
pub const HTTP_TRANSPORT: &str = "http";

/// The transport the SDP door's WebRTC call is carried on: the browser's offer, the call busbar
/// places with the provider, and the media listener the browser's association arrives on.
pub const WEBRTC_TRANSPORT: &str = "webrtc";
