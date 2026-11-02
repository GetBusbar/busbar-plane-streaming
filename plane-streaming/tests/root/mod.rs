// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE STREAMING PLANE DOOR'S ROOT CELLS, as the composition root's door-cells row reads them: this













/// THIS PLANE'S CAPABILITY-EQUALITY COLUMNS (`qa/capability-equality.json`): the cells its root leg
pub(crate) const COLUMNS: &[&str] = &["streaming-client", "streaming-server"];

/// `(capability, column, reason)`: the root-leg cells this plane is NOT owed, each the ledger's own
pub(crate) const NOT_APPLICABLE: &[(&str, &str, &str)] = &[
    (
        "breaker-trip",
        "streaming-server",
        "the voice front door is an inbound role; opening a session records no upstream target availability, so there is no core breaker cell to trip here -- the outbound provider dial a session triggers is the streaming-client cell's subject, the same shape every other inbound server column's breaker-trip is N/A under.",
    ),
    (
        "breaker-fastfail",
        "streaming-server",
        "the inbound session-open has no dispatch against an upstream to make fast; admission-before-dial against a tripped provider cell belongs to the streaming-client dial leg, not the front door that merely accepts the session.",
    ),
    (
        "failover-reroute",
        "streaming-client",
        "a duplex voice session is pinned to one provider socket for its whole lifetime, so there is no pre-first-byte candidate set to walk on the dial leg; a dropped session re-dials as a NEW session, never a mid-session reroute, so failover::walk has nothing to select over here.",
    ),
    (
        "failover-reroute",
        "streaming-server",
        "the front door is an inbound role with no candidate set to walk; failover selection over interchangeable provider members is the outbound streaming-client leg's decision, exactly as an inbound server column's reroute is not-applicable on the receiving side.",
    ),
    (
        "disposition",
        "streaming-client",
        "disposition classifies a discrete upstream ANSWER (proto Stage 1 -> breaker::classify); a duplex frame stream has no per-hop answer to normalize -- a fatal provider frame closes the session rather than resolving into a Disposition, so Stage 2 has nothing to classify on the dial leg.",
    ),
    (
        "disposition",
        "streaming-server",
        "disposition classifies an upstream's ANSWER; the inbound session-open has no upstream answer to normalize, so Stage-1/Stage-2 classification has no subject here -- the same argument the other inbound server columns' disposition is not-applicable under.",
    ),
    (
        "trust-pinning",
        "streaming-client",
        "voice providers (OpenAI Realtime, Gemini Live) are operator-declared config endpoints, not peer-published artifacts; there is no upstream-authored schema digest or card to pin -- exactly the argument an operator-declared provider dial cell is not-applicable under.",
    ),
    (
        "trust-pinning",
        "streaming-server",
        "the browser-sideband and telephony callers that reach the front door publish no artifact (no schema digest, agent card, or SPKI) for the trust lifecycle to pin; there is no peer-authored object whose drift could be detected, unlike a server column whose peers publish a card for it to fingerprint.",
    ),
    (
        "net-guard",
        "streaming-client",
        "the provider WSS is an operator-configured endpoint and no request-authored URL reaches the dial, so there is no SSRF surface on the outbound leg (the net_guard resolve->pin->guard at topology::dial_provider is defense-in-depth, not a request-URL guard); revisit if a T3 request-authored callback URL is ever dialed, like a server column's push delivery.",
    ),
    (
        "net-guard",
        "streaming-server",
        "the inbound session listener derives no outbound destination of its own; the one outbound a session triggers is the provider dial, guarded (defense-in-depth) on the streaming-client leg, so the front door has no request-authored egress to SSRF-guard.",
    ),
    (
        "egress-auth",
        "streaming-server",
        "the front door's only outbound is the provider dial, whose credential injection is the streaming-client egress-auth cell; the inbound caller's own bearer is authenticated by the Auth plugin, which is not egress credential planning -- the same split another inbound server column's egress-auth is not-applicable under.",
    ),
    (
        "catalogue",
        "streaming-client",
        "the voice plane's config is the SINGULAR streams: section, not a named-definition map like tools:/agents:, so there are no per-item registrations for catalogue::entitled to walk and trust-gate; named_def_list and registry_contains are None by construction, so no caller-visible catalogue exists on this plane.",
    ),
    (
        "catalogue",
        "streaming-server",
        "the voice plane declares the SINGULAR streams: section, not a named-definition map, so the front door exposes no per-item registry for catalogue::entitled to walk; named_def_list/registry_contains are None, so there is no caller-visible catalogue surface to trust-gate.",
    ),
];

/// THE SECRET-SOURCE SCAN'S DRIVEN DOOR (the composition root's `secret_source.rs`, which holds
pub(crate) fn secret_source_driven() -> Vec<String> {
    Vec::new()
}
